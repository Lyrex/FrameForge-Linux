import {
  createExecutionContext,
  createScheduledController,
  env,
  waitOnExecutionContext,
} from "cloudflare:test";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import worker from "../src/index";
import type { Snapshot } from "../src/snapshot";
import { UNAVAILABLE_HEADER, workerUnavailable } from "../src/unavailable";

const WORKER = "https://worker.test";

type UpstreamReply = { status?: number; body?: unknown; throws?: boolean; etag?: string };

// Stands in for every upstream. Each entry is consumed by one call, so a
// request the worker was not supposed to make runs the queue dry and fails
// loudly instead of silently hitting the network.
function upstream(...replies: UpstreamReply[]) {
  const calls: string[] = [];
  const queue = [...replies];

  vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    calls.push(url);
    const headers = new Headers(init?.headers ?? {});
    seenHeaders.push(headers);

    const reply = queue.shift();
    if (!reply) throw new Error(`unexpected upstream call: ${url}`);
    if (reply.throws) throw new Error("upstream unreachable");
    return new Response(JSON.stringify(reply.body ?? {}), {
      status: reply.status ?? 200,
      headers: {
        "Content-Type": "application/json",
        ...(reply.etag ? { ETag: reply.etag } : {}),
      },
    });
  });

  return { calls, remaining: () => queue.length };
}

let seenHeaders: Headers[] = [];

// The edge cache and KV outlive a single test, so every test starts a week
// after the one before it: nothing a previous test cached is still fresh.
let clock = Date.now();

beforeEach(async () => {
  vi.useFakeTimers({ toFake: ["Date"] });
  advance(7 * 24 * 60 * 60);
  // The stored snapshot and cursor outlive a test too.
  for (const key of (await env.SNAPSHOT.list()).keys) await env.SNAPSHOT.delete(key.name);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
  seenHeaders = [];
});

function advance(seconds: number) {
  clock += seconds * 1000;
  vi.setSystemTime(clock);
}

const get = (path: string, headers: HeadersInit = {}) =>
  worker.fetch(new Request(`${WORKER}${path}`, { headers }), env);

type LogLine = Record<string, unknown>;

// Every line the worker logged while `run` ran, parsed. Asserting on the parsed
// object rather than the text keeps a test from passing on a field that merely
// contains the value it looks for.
async function logsOf(run: () => Promise<unknown>): Promise<LogLine[]> {
  const lines: string[] = [];
  const spy = vi.spyOn(console, "log").mockImplementation((line: string) => {
    lines.push(line);
  });
  try {
    await run();
  } finally {
    spy.mockRestore();
  }
  return lines.map((line) => JSON.parse(line) as LogLine);
}

const requestLines = (lines: LogLine[]) => lines.filter((line) => "route" in line);
const eventLines = (lines: LogLine[], event: string) =>
  lines.filter((line) => line["event"] === event);

// Each cache class is exercised through its own route, so the TTL that route
// was given is the one under test. `ttl` is repeated from the source rather
// than imported: a test that read the same constant as the code would agree
// with any value the code was changed to.
const CLASSES = [
  {
    name: "order books",
    route: "/v1/wfm/items/mirage_prime_set/orders",
    upstream: "https://api.warframe.market/v2/orders/item/mirage_prime_set",
    body: { data: [{ platinum: 120, order_type: "sell" }] },
    ttl: 30,
  },
  {
    name: "prices and statistics",
    route: "/v1/wfm/items/mirage_prime_set/statistics",
    upstream: "https://api.warframe.market/v1/items/mirage_prime_set/statistics",
    body: { payload: { statistics_closed: { "48hours": [{ avg_price: 118 }] } } },
    ttl: 300,
  },
  {
    name: "worldstate",
    route: "/v1/worldstate",
    upstream: "https://api.warframe.com/cdn/worldState.php",
    body: { WorldSeed: "seed", ActiveMissions: [] },
    ttl: 45,
  },
  {
    name: "static catalog",
    route: "/v1/catalog/drops",
    upstream:
      "https://raw.githubusercontent.com/WFCD/warframe-drop-data/gh-pages/data/all.json",
    body: { missionRewards: {} },
    ttl: 21_600,
  },
];

describe.each(CLASSES)("$name", (dataClass) => {
  it("fetches once on a miss, then serves the hit without an upstream call", async () => {
    const mock = upstream({ body: dataClass.body });

    const miss = await get(dataClass.route);
    expect(miss.status).toBe(200);
    expect(miss.headers.get("X-FrameForge-Cache")).toBe("miss");
    await expect(miss.json()).resolves.toEqual(dataClass.body);

    const hit = await get(dataClass.route);
    expect(hit.headers.get("X-FrameForge-Cache")).toBe("hit");
    await expect(hit.json()).resolves.toEqual(dataClass.body);

    expect(mock.calls).toEqual([dataClass.upstream]);
  });

  it("still serves the cached body one second inside its own TTL", async () => {
    const mock = upstream({ body: dataClass.body });
    await get(dataClass.route);

    advance(dataClass.ttl - 1);
    const hit = await get(dataClass.route);

    expect(hit.headers.get("X-FrameForge-Cache")).toBe("hit");
    expect(mock.calls).toEqual([dataClass.upstream]);
  });

  it("fetches again one second past its own TTL", async () => {
    const mock = upstream({ body: dataClass.body }, { body: dataClass.body });
    await get(dataClass.route);

    advance(dataClass.ttl + 1);
    const refetched = await get(dataClass.route);

    expect(refetched.headers.get("X-FrameForge-Cache")).toBe("miss");
    expect(mock.calls).toEqual([dataClass.upstream, dataClass.upstream]);
  });
});

describe("stale-if-error", () => {
  const route = "/v1/wfm/items/nikana_prime_set/statistics";
  const body = { payload: { statistics_closed: { "48hours": [{ avg_price: 200 }] } } };

  it("serves the last cached body when the upstream 5xxes", async () => {
    upstream({ body }, { status: 503, body: { error: "upstream down" } });
    await get(route);

    expireCache();
    const stale = await get(route);

    expect(stale.status).toBe(200);
    expect(stale.headers.get("X-FrameForge-Cache")).toBe("stale");
    await expect(stale.json()).resolves.toEqual(body);
  });

  it("serves the last cached body when the upstream never answers", async () => {
    upstream({ body }, { throws: true });
    await get(route);

    expireCache();
    const stale = await get(route);

    expect(stale.status).toBe(200);
    await expect(stale.json()).resolves.toEqual(body);
  });

  it("passes the upstream error through when nothing is cached", async () => {
    upstream({ status: 500, body: { error: "boom" } });

    const response = await get("/v1/wfm/items/soma_prime_set/statistics");

    expect(response.status).toBe(500);
    expect(response.headers.get("X-FrameForge-Cache")).toBeNull();
  });
});

// The drop catalog is the one that makes this worth having: ~30 MB a client
// otherwise re-downloads on every refresh.
describe("client revalidation", () => {
  const route = "/v1/catalog/drops";
  const dropData =
    "https://raw.githubusercontent.com/WFCD/warframe-drop-data/gh-pages/data/all.json";
  const body = { missionRewards: { Earth: {} } };

  it("answers 304 with no body when the client holds the ETag it was given", async () => {
    const mock = upstream({ body, etag: '"drops-v2"' });

    const first = await get(route);
    expect(first.headers.get("ETag")).toBe('"drops-v2"');

    const revalidated = await get(route, { "If-None-Match": '"drops-v2"' });

    expect(revalidated.status).toBe(304);
    await expect(revalidated.text()).resolves.toBe("");
    expect(mock.calls).toEqual([dropData]);
  });

  it("serves the body when the client's validator is stale", async () => {
    upstream({ body, etag: '"drops-v2"' });
    await get(route);

    const response = await get(route, { "If-None-Match": '"drops-v1"' });

    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toEqual(body);
  });
});

// Jump past every TTL so the next request has to consult the upstream.
function expireCache() {
  advance(24 * 60 * 60);
}

describe("contract", () => {
  it("answers an unknown route with a JSON error", async () => {
    const response = await get("/v1/nonsense");

    expect(response.status).toBe(404);
    expect(response.headers.get("Content-Type")).toContain("application/json");
    await expect(response.json()).resolves.toEqual({ error: "not_found" });
  });

  it("rejects anything that is not a warframe.market slug", async () => {
    const mock = upstream();

    const response = await get("/v1/wfm/items/..%2Fauth%2Fsignin/orders");

    expect(response.status).toBe(400);
    expect(mock.calls).toEqual([]);
  });

  it("forwards no client headers to the upstream", async () => {
    upstream({ body: { data: [] } });

    await get("/v1/wfm/items/loki_prime_set/orders", {
      Authorization: "JWT secret",
      Cookie: "JWT=secret",
      "X-FrameForge-Version": "3.9.0",
    });

    const names = [...seenHeaders[0]!.keys()].map((name) => name.toLowerCase());
    expect(names).not.toContain("authorization");
    expect(names).not.toContain("cookie");
    expect(names).not.toContain("x-frameforge-version");
  });

  it("signals worker-unavailable with a header and a JSON body", async () => {
    const response = workerUnavailable();

    expect(response.status).toBe(503);
    expect(response.headers.get(UNAVAILABLE_HEADER)).toBe("unavailable");
    await expect(response.json()).resolves.toEqual({ error: "worker_unavailable" });
  });
});

const CATALOG_UPSTREAM = "https://api.warframe.market/v2/items";
const statisticsUpstream = (slug: string) =>
  `https://api.warframe.market/v1/items/${slug}/statistics`;

const catalogBody = (...slugs: string[]) => ({
  data: [
    ...slugs.map((slug) => ({ slug, id: `id_${slug}`, i18n: { en: { name: slug.toUpperCase() } } })),
    // Neither addressable nor searchable, so the catalog must drop it.
    { id: "id_nameless" },
  ],
});

// A traded item: three sales in the last two days, so the 48-hour window decides.
const priced = (platinum: number) => ({
  payload: {
    statistics_closed: {
      "48hours": [{ median: platinum, volume: 3 }],
      "90days": [{ median: platinum * 2, volume: 90 }],
    },
  },
});

const runPrewarm = async (batchSize: number) => {
  const ctx = createExecutionContext();
  const tickEnv = { ...env, PREWARM_BATCH_SIZE: batchSize } as unknown as Env;
  await worker.scheduled?.(createScheduledController(), tickEnv, ctx);
  await waitOnExecutionContext(ctx);
};

const readSnapshot = async (): Promise<Snapshot> => (await get("/v1/snapshot")).json();

describe("item catalog", () => {
  it("serves slugs with display metadata, then serves it again without an upstream call", async () => {
    const mock = upstream({ body: catalogBody("mirage_prime_set") });

    const miss = await get("/v1/wfm-items");
    expect(miss.status).toBe(200);
    await expect(miss.json()).resolves.toEqual({
      items: [{ slug: "mirage_prime_set", name: "MIRAGE_PRIME_SET" }],
    });

    const hit = await get("/v1/wfm-items");
    expect(hit.headers.get("X-FrameForge-Cache")).toBe("hit");

    expect(mock.calls).toEqual([CATALOG_UPSTREAM]);
  });
});

describe("price snapshot", () => {
  it("says so when prewarm has never run, without asking an upstream", async () => {
    const mock = upstream();

    const body = await readSnapshot();

    expect(body).toEqual({ generation: null, items: {} });
    expect(mock.calls).toEqual([]);
  });

  it("carries a generation marker and a price with its own freshness per item", async () => {
    upstream({ body: catalogBody("ash_prime_set") }, { body: priced(120) });
    await runPrewarm(10);

    const body = await readSnapshot();

    expect(body.generation).toBe(Math.floor(clock / 1000));
    expect(body.items["ash_prime_set"]).toEqual({ plat: 120, at: Math.floor(clock / 1000) });
  });

  // Pins the price the app must agree with, since the same trimmed median is
  // implemented twice. The 48-hour window holds one sale, under the three it
  // takes to be trusted, so the 90-day window decides: 15% of six prices trims
  // nothing, and the median of 40 and 42 is 41 — the 10 and the 300 pull it
  // nowhere.
  it("derives the app's price from the 90-day window when the recent one is thin", async () => {
    upstream(
      { body: catalogBody("nova_prime_set") },
      {
        body: {
          payload: {
            statistics_closed: {
              "48hours": [{ median: 40, volume: 1 }],
              "90days": [
                { median: 10, volume: 5 },
                { median: 38, volume: 5 },
                { median: 40, volume: 5 },
                { median: 42, volume: 5 },
                { median: 44, volume: 5 },
                { median: 300, volume: 5 },
              ],
            },
          },
        },
      },
    );
    await runPrewarm(1);

    const body = await readSnapshot();

    expect(body.items["nova_prime_set"]?.plat).toBe(41);
  });

  it("costs no upstream call to serve", async () => {
    upstream({ body: catalogBody("volt_prime_set") }, { body: priced(45) });
    await runPrewarm(10);

    const mock = upstream();
    const body = await readSnapshot();

    expect(body.items["volt_prime_set"]?.plat).toBe(45);
    expect(mock.calls).toEqual([]);
  });
});

describe("prewarm", () => {
  it("refreshes a bounded batch per tick and covers the catalog over a full pass", async () => {
    const slugs = ["a_prime_set", "b_prime_set", "c_prime_set", "d_prime_set", "e_prime_set"];
    const mock = upstream(
      { body: catalogBody(...slugs) },
      ...slugs.map((slug) => ({ body: priced(slug.length) })),
      // The cursor wraps past the end of the catalog, so the sixth item
      // refreshed is the first one again.
      { body: priced(11) },
    );

    for (let tick = 0; tick < 3; tick++) {
      await runPrewarm(2);
      // Past the statistics freshness, so the next tick's fetches are real ones.
      advance(400);
    }

    expect(mock.calls).toEqual([
      CATALOG_UPSTREAM,
      ...slugs.map(statisticsUpstream),
      statisticsUpstream("a_prime_set"),
    ]);

    const body = await readSnapshot();
    expect(Object.keys(body.items).sort()).toEqual(slugs);
  });

  it("leaves an item's previous entry alone when its upstream fetch fails", async () => {
    upstream(
      { body: catalogBody("saryn_prime_set", "trinity_prime_set") },
      { body: priced(150) },
      { body: priced(60) },
      { throws: true },
    );

    await runPrewarm(1);
    advance(400);
    await runPrewarm(1);
    advance(400);
    // Back round to the first item, which now fails.
    await runPrewarm(1);

    const body = await readSnapshot();
    expect(body.items["saryn_prime_set"]?.plat).toBe(150);
    expect(body.items["trinity_prime_set"]?.plat).toBe(60);
  });
});

describe("health", () => {
  it("answers without an upstream call", async () => {
    const mock = upstream();

    const response = await get("/v1/health");

    expect(response.status).toBe(200);
    await expect(response.json()).resolves.toEqual({ status: "ok" });
    expect(mock.calls).toEqual([]);
  });
});

describe("daily budget", () => {
  const route = "/v1/worldstate";
  const body = { WorldSeed: "seed" };

  // Small enough that a test can cross it, given the way the deployed worker
  // takes it: a binding, not a constant in the code.
  const capped = (limit: number) => ({ ...env, DAILY_REQUEST_BUDGET: limit }) as unknown as Env;

  const getCapped = (path: string, limit: number) =>
    worker.fetch(new Request(`${WORKER}${path}`), capped(limit));

  it("serves normally under budget", async () => {
    upstream({ body });

    const response = await getCapped(route, 5);

    expect(response.status).toBe(200);
    expect(response.headers.get(UNAVAILABLE_HEADER)).toBeNull();
  });

  it("flips every route to the unavailable signal once the threshold is crossed", async () => {
    const mock = upstream({ body });
    await getCapped(route, 1);

    for (const path of ["/v1/worldstate", "/v1/snapshot", "/v1/wfm-items", "/v1/nonsense"]) {
      const response = await getCapped(path, 1);
      expect(response.status).toBe(503);
      expect(response.headers.get(UNAVAILABLE_HEADER)).toBe("unavailable");
      await expect(response.json()).resolves.toEqual({ error: "worker_unavailable" });
    }

    expect(mock.remaining()).toBe(0);
  });

  it("keeps answering health while standing down", async () => {
    upstream();
    await getCapped(route, 0);

    const response = await getCapped("/v1/health", 0);

    expect(response.status).toBe(200);
  });

  it("restores service at the daily reset", async () => {
    upstream({ body }, { body });
    await getCapped(route, 1);
    expect((await getCapped(route, 1)).status).toBe(503);

    advance(24 * 60 * 60);

    expect((await getCapped(route, 1)).status).toBe(200);
  });

  it("skips the cron prewarm over budget", async () => {
    const mock = upstream();
    const ctx = createExecutionContext();

    await worker.scheduled?.(createScheduledController(), capped(0), ctx);
    await waitOnExecutionContext(ctx);

    expect(mock.calls).toEqual([]);
  });
});

describe("request log", () => {
  it("carries route, method, status, latency, app version and cache outcome — and nothing else", async () => {
    upstream({ body: { data: [] } });

    const lines = await logsOf(() =>
      get("/v1/wfm/items/mirage_prime_set/orders", { "X-FrameForge-Version": "3.9.0" }),
    );

    const entry = requestLines(lines).at(-1)!;
    expect(Object.keys(entry).sort()).toEqual([
      "cache",
      "latency_ms",
      "method",
      "route",
      "status",
      "version",
    ]);
    // The slug is what someone looked up, so no log line may carry it.
    expect(entry["route"]).toBe("/v1/wfm/items/:slug/orders");
    expect(JSON.stringify(lines)).not.toContain("mirage_prime_set");
    expect(entry).toMatchObject({ method: "GET", status: 200, version: "3.9.0" });
  });

  it("keeps the slug out of the log when the path matches no route", async () => {
    // A trailing slash is enough to miss every pattern, and the path is then
    // client text that would otherwise be written out verbatim.
    let response: Response;
    const lines = await logsOf(async () => {
      response = await get("/v1/wfm/items/mirage_prime_set/orders/");
    });

    expect(response!.status).toBe(404);
    expect(JSON.stringify(lines)).not.toContain("mirage_prime_set");
    expect(requestLines(lines).at(-1)!["route"]).toBe("unmatched");
  });

  it("reports the miss, then the hit it was serving from", async () => {
    upstream({ body: { WorldSeed: "seed" } });

    const first = await logsOf(() => get("/v1/worldstate"));
    const second = await logsOf(() => get("/v1/worldstate"));

    expect(requestLines(first).at(-1)!["cache"]).toBe("miss");
    expect(requestLines(second).at(-1)!["cache"]).toBe("hit");
  });

  it("reports a body the caller already held as neither", async () => {
    upstream({ body: { missionRewards: {} }, etag: '"drops-v3"' });
    await get("/v1/catalog/drops");

    const lines = await logsOf(() =>
      get("/v1/catalog/drops", { "If-None-Match": '"drops-v3"' }),
    );

    expect(requestLines(lines).at(-1)).toMatchObject({ status: 304, cache: "not_modified" });
  });

  it("reports no cache outcome for a route that never consults the cache", async () => {
    const lines = await logsOf(() => get("/v1/health"));

    expect(requestLines(lines).at(-1)!["cache"]).toBeNull();
  });
});

describe("operator log", () => {
  it("reports the prewarm tick's counts, cursor and snapshot size", async () => {
    upstream(
      { body: catalogBody("ember_prime_set", "frost_prime_set") },
      { body: priced(90) },
      { throws: true },
    );

    const lines = await logsOf(() => runPrewarm(2));

    expect(eventLines(lines, "prewarm")).toEqual([
      {
        event: "prewarm",
        attempted: 2,
        refreshed: 1,
        failed: 1,
        cursor: 0,
        catalog_size: 2,
        entries: 1,
        duration_ms: expect.any(Number),
      },
    ]);
  });

  it("reports a tick that found no catalog to walk", async () => {
    upstream({ body: { data: [] } });

    const lines = await logsOf(() => runPrewarm(2));

    expect(eventLines(lines, "prewarm")).toEqual([
      {
        event: "prewarm",
        attempted: 0,
        refreshed: 0,
        failed: 0,
        cursor: 0,
        catalog_size: 0,
        entries: 0,
        duration_ms: expect.any(Number),
      },
    ]);
  });

  it("names the upstream and what the caller got when a stale body is served", async () => {
    const route = "/v1/wfm/items/rhino_prime_set/statistics";
    upstream({ body: priced(70) }, { status: 503, body: { error: "down" } });
    await get(route);
    expireCache();

    const lines = await logsOf(() => get(route));

    expect(eventLines(lines, "upstream_down")).toEqual([
      {
        event: "upstream_down",
        upstream: "api.warframe.market",
        status: 503,
        served: "stale",
      },
    ]);
    // The host is ours; the path that would carry the slug is not logged.
    expect(JSON.stringify(lines)).not.toContain("rhino_prime_set");
  });

  it("reports an unreachable upstream with no status of its own", async () => {
    upstream({ throws: true });

    const lines = await logsOf(() => get("/v1/wfm/items/wisp_prime_set/orders"));

    expect(eventLines(lines, "upstream_down")).toEqual([
      {
        event: "upstream_down",
        upstream: "api.warframe.market",
        status: null,
        served: "unreachable",
      },
    ]);
  });

  it("marks the budget crossing once, not on every request after it", async () => {
    upstream({ body: { WorldSeed: "seed" } });
    const capped = { ...env, DAILY_REQUEST_BUDGET: 1 } as unknown as Env;
    const request = () => worker.fetch(new Request(`${WORKER}/v1/worldstate`), capped);

    const under = await logsOf(request);
    const crossing = await logsOf(request);
    const after = await logsOf(request);

    expect(eventLines(under, "budget_exceeded")).toEqual([]);
    expect(eventLines(crossing, "budget_exceeded")).toEqual([
      { event: "budget_exceeded", threshold: 1, count: 2 },
    ]);
    expect(eventLines(after, "budget_exceeded")).toEqual([]);
  });
});
