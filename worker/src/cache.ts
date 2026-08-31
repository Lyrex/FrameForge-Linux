// Every route funnels through `readThrough`, so cache policy is decided in one
// place rather than per route.

import { jsonError } from "./unavailable";
import { USER_AGENT, UPSTREAM_TIMEOUT_MS } from "./upstream";

// How long a cached body counts as fresh, per data class. Order books are what
// a trader acts on, so they are the shortest; catalogs change on the timescale
// of game updates.
export const TTL = {
  orders: 30,
  statistics: 300,
  worldstate: 45,
  catalog: 21_600,
  // The snapshot only changes when a prewarm tick writes it, so a client
  // holding one for a few minutes is never far behind.
  snapshot: 300,
} as const;

// A body stays in the edge cache long past its freshness so it is still there
// to serve when an upstream is down. Bounded so an upstream that dies
// permanently eventually stops being answered from a fossil.
const STALE_WINDOW_SECONDS = 86_400;

// Cache keys are built against this fixed origin. It is never fetched.
export const CACHE_ORIGIN = "https://frameforge.cache";

const FRESH_UNTIL_HEADER = "X-FrameForge-Fresh-Until";
export const CACHE_STATUS_HEADER = "X-FrameForge-Cache";

type CacheStatus = "hit" | "miss" | "stale" | "revalidated";

export async function readThrough(
  request: Request,
  upstreamUrl: string,
  ttlSeconds: number,
): Promise<Response> {
  const cache = caches.default;
  // Keyed on the path alone: no request header and no hostname take part, so
  // two users asking for the same thing — and the cron prewarm asking for it
  // off any hostname at all — always collapse into one entry.
  const key = new Request(new URL(new URL(request.url).pathname, CACHE_ORIGIN), { method: "GET" });

  const cached = await cache.match(key);
  if (cached && Date.now() < Number(cached.headers.get(FRESH_UNTIL_HEADER) ?? 0)) {
    return clientResponse(request, cached, "hit", ttlSeconds);
  }

  // Only the headers we choose reach the upstream — nothing from the client is
  // relayed, so an Authorization or Cookie header sent to us dies here. The
  // client's own If-None-Match is read (see `clientResponse`) but never
  // forwarded: the validator sent upstream is always the one this cache holds.
  const headers: Record<string, string> = { "User-Agent": USER_AGENT, Accept: "application/json" };
  const etag = cached?.headers.get("ETag");
  if (etag) headers["If-None-Match"] = etag;

  let upstream: Response;
  try {
    upstream = await fetch(upstreamUrl, {
      headers,
      signal: AbortSignal.timeout(UPSTREAM_TIMEOUT_MS),
    });
  } catch {
    logUpstreamDown(upstreamUrl, null, cached ? "stale" : "unreachable");
    if (cached) return clientResponse(request, cached, "stale", ttlSeconds);
    return jsonError(502, "upstream_unreachable");
  }

  if (upstream.status === 304 && cached) {
    const refreshed = await store(cache, key, cached, ttlSeconds, cached.headers.get("ETag"));
    return clientResponse(request, refreshed, "revalidated", ttlSeconds);
  }

  if (!upstream.ok) {
    if (upstream.status >= 500) {
      logUpstreamDown(upstreamUrl, upstream.status, cached ? "stale" : "passthrough");
      if (cached) return clientResponse(request, cached, "stale", ttlSeconds);
    }
    // A 4xx is the upstream's answer about this item, not a failure to reach
    // it — the client needs to see it. It is deliberately not cached.
    return new Response(upstream.body, {
      status: upstream.status,
      headers: { "Content-Type": contentTypeOf(upstream) },
    });
  }

  const stored = await store(cache, key, upstream, ttlSeconds, upstream.headers.get("ETag"));
  return clientResponse(request, stored, "miss", ttlSeconds);
}

async function store(
  cache: Cache,
  key: Request,
  source: Response,
  ttlSeconds: number,
  etag: string | null,
): Promise<Response> {
  const body = await source.arrayBuffer();
  const headers = new Headers({
    "Content-Type": contentTypeOf(source),
    "Cache-Control": `max-age=${ttlSeconds + STALE_WINDOW_SECONDS}`,
    [FRESH_UNTIL_HEADER]: String(Date.now() + ttlSeconds * 1000),
  });
  if (etag) headers.set("ETag", etag);

  const entry = new Response(body, { headers });
  // Awaited rather than deferred to waitUntil: the next request must see the
  // entry, and the put is a local write.
  await cache.put(key, entry.clone());
  return entry;
}

// The stored ETag is handed to the client so it can skip a body it already
// holds. `If-None-Match` is the one client header the worker reads: a validator
// says which bytes the caller has, not who the caller is, so it identifies
// nobody. It is still never forwarded — the validator the upstream sees is
// always this cache's own.
function clientResponse(
  request: Request,
  source: Response,
  status: CacheStatus,
  ttlSeconds: number,
): Response {
  const etag = source.headers.get("ETag");
  const headers = new Headers({
    "Content-Type": contentTypeOf(source),
    "Cache-Control": `public, max-age=${ttlSeconds}`,
    [CACHE_STATUS_HEADER]: status,
  });
  if (etag) headers.set("ETag", etag);

  if (etag && request.headers.get("If-None-Match") === etag) {
    return new Response(null, { status: 304, headers });
  }
  return new Response(source.body, { headers });
}

// An upstream that failed, and what the caller got instead. The host is ours to
// name; the URL is not, because a per-item path carries the slug that was asked
// for. `status` is null when the fetch never produced one — a timeout or a
// refused connection.
function logUpstreamDown(
  upstreamUrl: string,
  status: number | null,
  served: "stale" | "unreachable" | "passthrough",
): void {
  console.log(
    JSON.stringify({
      event: "upstream_down",
      upstream: new URL(upstreamUrl).host,
      status,
      served,
    }),
  );
}

function contentTypeOf(response: Response): string {
  return response.headers.get("Content-Type") ?? "application/json";
}
