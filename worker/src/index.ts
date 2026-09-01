// FrameForge public-data cache worker.
//
// The contract lives under /v1/. It carries public data only — the same bytes
// for every caller — so nothing here reads a request header beyond the method
// and path, and nothing a client sends is relayed to an upstream.
//
// Clients identify their release with X-FrameForge-Version. That is the only
// client-identifying header the contract has, and no route requires it.

import { overBudget } from "./budget";
import { CACHE_STATUS_HEADER } from "./cache";
import { drops } from "./routes/catalog";
import { items, orders, statistics } from "./routes/wfm";
import { worldstate } from "./routes/worldstate";
import { prewarm, snapshot } from "./snapshot";
import { jsonError, workerUnavailable } from "./unavailable";

export { DailyBudget } from "./budget";

// `ctx` is carried down to the cache so a route can hand back a body it already
// holds and refresh it after the response has gone.
type Handler = (
  request: Request,
  groups: Record<string, string | undefined>,
  env: Env,
  ctx: ExecutionContext,
) => Response | Promise<Response>;

const routes: { path: string; pattern: URLPattern; handle: Handler }[] = [
  route("/v1/wfm/items/:slug/statistics", (request, groups, _env, ctx) =>
    statistics(request, groups.slug ?? "", ctx),
  ),
  route("/v1/wfm/items/:slug/orders", (request, groups) => orders(request, groups.slug ?? "")),
  route("/v1/worldstate", (request) => worldstate(request)),
  route("/v1/catalog/drops", (request, _groups, _env, ctx) => drops(request, ctx)),
  route("/v1/wfm-items", (request, _groups, _env, ctx) => items(request, ctx)),
  route("/v1/snapshot", (_request, _groups, env) => snapshot(env)),
];

function route(path: string, handle: Handler) {
  return { path, pattern: new URLPattern({ pathname: path }), handle };
}

const HEALTH_PATH = "/v1/health";

// Logged in place of the path whenever no route pattern matched. The path is
// then arbitrary client text — `/v1/wfm/items/<slug>/orders/` misses on the
// trailing slash alone — and writing it out would record the lookup that the
// pattern logging exists to keep out of the logs.
const UNMATCHED_ROUTE = "unmatched";

export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const started = Date.now();
    const path = new URL(request.url).pathname;

    // Health answers before the budget is consulted, and without counting
    // against it: an operator has to be able to see that the worker is alive
    // precisely when it is standing down.
    if (path === HEALTH_PATH && (request.method === "GET" || request.method === "HEAD")) {
      return log(request, HEALTH_PATH, started, health());
    }

    if (request.method !== "GET" && request.method !== "HEAD") {
      return log(request, UNMATCHED_ROUTE, started, jsonError(405, "method_not_allowed"));
    }

    const match = routes
      .map((candidate) => ({ candidate, result: candidate.pattern.exec(request.url) }))
      .find((attempt) => attempt.result);

    if (await overBudget(env)) {
      return log(request, match?.candidate.path ?? UNMATCHED_ROUTE, started, workerUnavailable());
    }

    if (!match) return log(request, UNMATCHED_ROUTE, started, jsonError(404, "not_found"));

    return log(
      request,
      match.candidate.path,
      started,
      await match.candidate.handle(request, match.result!.pathname.groups, env, ctx),
    );
  },

  async scheduled(_controller: ScheduledController, env: Env, ctx: ExecutionContext): Promise<void> {
    ctx.waitUntil(
      (async () => {
        // The prewarm is by far the worker's heaviest upstream consumer, so
        // over budget it does nothing at all rather than a smaller batch.
        if (await overBudget(env)) return;
        await prewarm(env);
      })(),
    );
  },
} satisfies ExportedHandler<Env>;

// A liveness answer, not a status page: no upstream call, no cached state, and
// nothing an operator could mistake for a report on the data being served.
function health(): Response {
  return new Response(JSON.stringify({ status: "ok" }), {
    headers: { "Content-Type": "application/json", "Cache-Control": "no-store" },
  });
}

// One line per request, and only these six fields. A field is admitted when it
// describes what the worker did and refused when it describes who asked: the
// route pattern rather than the path, because a line naming the slug someone
// asked for is a record of what that person was looking up, and no address,
// agent, query or body at all. `cache` passes the same test — whether the edge
// answered from its own copy is the worker's own behaviour.
function log(request: Request, route: string, started: number, response: Response): Response {
  console.log(
    JSON.stringify({
      route,
      method: request.method,
      status: response.status,
      latency_ms: Date.now() - started,
      version: request.headers.get("X-FrameForge-Version"),
      cache: cacheOutcome(response),
    }),
  );
  return response;
}

// The `X-FrameForge-Cache` outcome as `readThrough` decided it, and null for a
// route that never consults the edge cache. A 304 is its own outcome: the body
// was served from neither, because the caller already held it.
function cacheOutcome(response: Response): string | null {
  if (response.status === 304) return "not_modified";
  return response.headers.get(CACHE_STATUS_HEADER);
}
