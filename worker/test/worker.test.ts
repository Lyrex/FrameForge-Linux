import {
  createExecutionContext,
  createScheduledController,
  env,
  waitOnExecutionContext,
} from "cloudflare:test";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resetBudgetState } from "../src/budget";
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
    notePath(url);
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

// KV outlives a single test, so every test starts a week after the one before
// it and nothing it stored reads as recent.
let clock = Date.now();

// The edge cache outlives a test too, and it expires entries on the real clock
// rather than the fake one — so a body a previous test cached would be handed
// to this one as a stale hit instead of being refetched. Every path a test
// reaches through the worker is dropped before the next test runs.
const cached = new Set<string>();

// The worker path a given upstream URL is cached under. Only the per-item
// statistics entry needs the translation: a prewarm tick warms it without any
// test having asked for that path.
function notePath(url: string) {
  const statistics = /api\.warframe\.market\/v1\/items\/([a-z0-9_]+)\/statistics$/.exec(url);
  if (statistics) cached.add(`/v1/wfm/items/${statistics[1]}/statistics`);
}

async function clearCache() {
  for (const path of cached) {
    await caches.default.delete(new Request(new URL(path, "https://frameforge.cache")));
  }
  cached.clear();
}

beforeEach(async () => {
  vi.useFakeTimers({ toFake: ["Date"] });
  advance(7 * 24 * 60 * 60);
  await clearCache();
  // The isolate's cached budget verdict and its uncounted requests outlive a
  // test, and either would decide the next one.
  resetBudgetState();
  // The stored snapshot and cursors outlive a test too.
  for (const key of (await env.SNAPSHOT.list()).keys) await env.SNAPSHOT.delete(key.name);
});

afterEach(async () => {
  await settle();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  seenHeaders = [];
});

function advance(seconds: number) {
  clock += seconds * 1000;
  vi.setSystemTime(clock);
}

// Every request gets its own execution context, and `settle` waits for the work
// each one deferred past its response — which is where a background refresh
// runs, so a test can tell "answered without waiting" from "refreshed".
const contexts: ExecutionContext[] = [];

const get = (path: string, headers: HeadersInit = {}) => {
  cached.add(path);
  const ctx = createExecutionContext();
  contexts.push(ctx);
  return worker.fetch(new Request(`${WORKER}${path}`, { headers }), env, ctx);
};

async function settle() {
  for (const ctx of contexts.splice(0)) await waitOnExecutionContext(ctx);
}

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
    servesStale: false,
  },
  {
    name: "prices and statistics",
    route: "/v1/wfm/items/mirage_prime_set/statistics",
    upstream: "https://api.warframe.market/v1/items/mirage_prime_set/statistics",
    body: { payload: { statistics_closed: { "48hours": [{ avg_price: 118 }] } } },
    ttl: 3600,
    servesStale: true,
  },
  {
    name: "worldstate",
    route: "/v1/worldstate",
    upstream: "https://api.warframe.com/cdn/worldState.php",
    body: { WorldSeed: "seed", ActiveMissions: [] },
    ttl: 45,
    servesStale: false,
  },
  {
    name: "static catalog",
    route: "/v1/catalog/drops",
    upstream:
      "https://raw.githubusercontent.com/WFCD/warframe-drop-data/gh-pages/data/all.json",
    body: { missionRewards: {} },
    ttl: 21_600,
    servesStale: true,
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

  it("goes back to the upstream one second past its own TTL", async () => {
    const mock = upstream({ body: dataClass.body }, { body: dataClass.body });
    await get(dataClass.route);

    advance(dataClass.ttl + 1);
    const refetched = await get(dataClass.route);
    await settle();

    expect(refetched.headers.get("X-FrameForge-Cache")).toBe(
      dataClass.servesStale ? "revalidating" : "miss",
    );
    expect(mock.calls).toEqual([dataClass.upstream, dataClass.upstream]);
  });
});

// The order book is the class that still blocks, so a failure there is the one
// that has to fall back to the last body rather than to a background refresh.
describe("stale-if-error", () => {
  const route = "/v1/wfm/items/nikana_prime_set/orders";
  const body = { data: [{ platinum: 200, order_type: "sell" }] };

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

  it("keeps serving the last body when a background refresh fails", async () => {
    upstream({ body: priced(70) }, { status: 503, body: { error: "down" } });
    const statistics = "/v1/wfm/items/nikana_prime_set/statistics";
    await get(statistics);

    expireCache();
    await get(statistics);
    await settle();

    upstream({ status: 503, body: { error: "down" } });
    const again = await get(statistics);

    expect(again.status).toBe(200);
    await expect(again.json()).resolves.toEqual(priced(70));
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

describe("stale-while-revalidate", () => {
  const route = "/v1/wfm/items/octavia_prime_set/statistics";

  // One second past the hour a statistics body stays fresh.
  const expire = () => advance(3601);

  it("answers a stale price from cache and refreshes behind the response", async () => {
    const mock = upstream({ body: priced(100) }, { body: priced(140) });
    await get(route);

    expire();
    const stale = await get(route);

    // The caller got the body we already held, not the one the upstream is
    // about to give: nothing waited on warframe.market.
    expect(stale.headers.get("X-FrameForge-Cache")).toBe("revalidating");
    await expect(stale.json()).resolves.toEqual(priced(100));

    await settle();
    expect(mock.calls.length).toBe(2);

    const next = await get(route);
    expect(next.headers.get("X-FrameForge-Cache")).toBe("hit");
    await expect(next.json()).resolves.toEqual(priced(140));
  });

  it("refreshes once for a crowd of callers on the same stale price", async () => {
    // Deliberately more replies than the test expects to be used, so a second
    // refresh shows up as a count rather than as a queue that ran dry.
    const mock = upstream({ body: priced(100) }, ...Array(6).fill({ body: priced(140) }));
    await get(route);

    expire();
    const crowd = await Promise.all(Array.from({ length: 5 }, () => get(route)));

    // Every one of them was answered out of the cache — the crowd arriving on
    // an expired price is exactly when nobody should be waiting on an upstream.
    for (const response of crowd) {
      expect(response.headers.get("X-FrameForge-Cache")).not.toBe("miss");
    }

    await settle();
    expect(mock.calls.length).toBe(2);
  });

  it("makes an expired order book wait for the current one", async () => {
    const book = (platinum: number) => ({ data: [{ platinum, order_type: "sell" }] });
    const mock = upstream({ body: book(120) }, { body: book(95) });
    const orders = "/v1/wfm/items/octavia_prime_set/orders";
    await get(orders);

    advance(31);
    const response = await get(orders);

    expect(response.headers.get("X-FrameForge-Cache")).toBe("miss");
    await expect(response.json()).resolves.toEqual(book(95));
    expect(mock.calls.length).toBe(2);
  });
});

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

// A tick's batch, split and ceiling are bindings on the deployed worker, so a
// test sets them the same way. The hot lane is off unless a test asks for it:
// most of these are about the walk over the whole catalog.
type TickConfig = {
  PREWARM_HOT_BATCH_SIZE?: number;
  PREWARM_HOT_SIZE?: number;
  PREWARM_SUBREQUEST_CEILING?: number;
};

// Sales over the last two days are what ranks an item, so this one varies the
// volume and holds the price still.
const traded = (volume: number) => ({
  payload: {
    statistics_closed: {
      "48hours": [{ median: 50, volume }],
      "90days": [{ median: 50, volume }],
    },
  },
});

const runPrewarm = async (batchSize: number, config: TickConfig = {}) => {
  // The tick reads the item catalog through the same cache a request would.
  cached.add("/v1/wfm-items");
  const ctx = createExecutionContext();
  const tickEnv = {
    ...env,
    PREWARM_BATCH_SIZE: batchSize,
    PREWARM_HOT_BATCH_SIZE: 0,
    PREWARM_HOT_SIZE: 0,
    PREWARM_SUBREQUEST_CEILING: 1000,
    ...config,
  } as unknown as Env;
  await worker.scheduled?.(createScheduledController(), tickEnv, ctx);
  await waitOnExecutionContext(ctx);
};

// Answers by URL instead of in order. A tick's walk is about which items were
// fetched and how often, which an ordered queue cannot express.
function upstreamByUrl(bodies: Record<string, unknown>) {
  const calls: string[] = [];

  vi.stubGlobal("fetch", async (input: RequestInfo | URL) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    calls.push(url);
    notePath(url);

    const body = bodies[url];
    if (body === undefined) throw new Error(`unexpected upstream call: ${url}`);
    return new Response(JSON.stringify(body), {
      headers: { "Content-Type": "application/json" },
    });
  });

  return { calls, countOf: (url: string) => calls.filter((call) => call === url).length };
}

// A catalog of items with the trade volumes that decide which of them are hot.
function tradedCatalog(volumes: Record<string, number>) {
  const bodies: Record<string, unknown> = {
    [CATALOG_UPSTREAM]: catalogBody(...Object.keys(volumes)),
  };
  for (const [slug, volume] of Object.entries(volumes)) {
    bodies[statisticsUpstream(slug)] = traded(volume);
  }
  return upstreamByUrl(bodies);
}

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
    expect(body.items["ash_prime_set"]).toEqual({
      plat: 120,
      at: Math.floor(clock / 1000),
      vol: 3,
    });
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
      advance(3601);
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
    advance(3601);
    await runPrewarm(1);
    advance(3601);
    // Back round to the first item, which now fails.
    await runPrewarm(1);

    const body = await readSnapshot();
    expect(body.items["saryn_prime_set"]?.plat).toBe(150);
    expect(body.items["trinity_prime_set"]?.plat).toBe(60);
  });
});

describe("hot set", () => {
  // Descending volume, so the ranking is the order they are written in.
  const VOLUMES = { a_prime_set: 100, b_prime_set: 50, c_prime_set: 10, d_prime_set: 1 };
  const slugs = Object.keys(VOLUMES);

  // Volume has to be measured before it can rank anything, so every test here
  // starts from a pass that has priced the whole catalog once.
  const seed = () => runPrewarm(slugs.length);

  it("records each item's recent trade volume", async () => {
    tradedCatalog(VOLUMES);
    await seed();

    const body = await readSnapshot();

    expect(Object.fromEntries(slugs.map((slug) => [slug, body.items[slug]?.vol]))).toEqual(VOLUMES);
  });

  it("refreshes a busy item far more often than a quiet one", async () => {
    const mock = tradedCatalog(VOLUMES);
    await seed();

    // One hot item and one cold item a tick, with only the busiest counted hot.
    for (let tick = 0; tick < 6; tick++) {
      advance(3601);
      await runPrewarm(2, { PREWARM_HOT_BATCH_SIZE: 1, PREWARM_HOT_SIZE: 1 });
    }

    const busiest = mock.countOf(statisticsUpstream("a_prime_set"));
    const quietest = mock.countOf(statisticsUpstream("d_prime_set"));

    expect(busiest).toBeGreaterThanOrEqual(7);
    expect(busiest).toBeGreaterThan(quietest * 3);
  });

  it("still reaches an item the hot lane never touches", async () => {
    const mock = tradedCatalog(VOLUMES);
    await seed();
    const seeded = mock.countOf(statisticsUpstream("d_prime_set"));

    for (let tick = 0; tick < 6; tick++) {
      advance(3601);
      await runPrewarm(2, { PREWARM_HOT_BATCH_SIZE: 1, PREWARM_HOT_SIZE: 1 });
    }

    expect(mock.countOf(statisticsUpstream("d_prime_set"))).toBeGreaterThan(seeded);
  });

  it("keeps an item nobody has priced yet out of the hot lane", async () => {
    const mock = tradedCatalog(VOLUMES);
    // One item measured, the rest of the catalog still unknown.
    await runPrewarm(1);

    const before = mock.calls.length;
    advance(3601);
    await runPrewarm(1, { PREWARM_HOT_BATCH_SIZE: 1, PREWARM_HOT_SIZE: 4 });

    // The hot lane has exactly one candidate, so the tick's one hot fetch goes
    // to it rather than to an item whose volume nothing has measured.
    expect(mock.calls.slice(before)).toEqual([statisticsUpstream("a_prime_set")]);
  });
});

describe("subrequest ceiling", () => {
  const VOLUMES = { a_prime_set: 5, b_prime_set: 4, c_prime_set: 3, d_prime_set: 2 };

  // Four items asked for, and a ceiling that pays for two of them.
  const truncated = () => runPrewarm(4, { PREWARM_SUBREQUEST_CEILING: 10 });

  it("keeps what a cut-short tick managed and starts the next one after it", async () => {
    tradedCatalog(VOLUMES);

    await truncated();
    expect(Object.keys((await readSnapshot()).items)).toEqual(["a_prime_set", "b_prime_set"]);

    advance(3601);
    await truncated();
    expect(Object.keys((await readSnapshot()).items).sort()).toEqual(Object.keys(VOLUMES));
  });

  it("counts the batch it never got to rather than reporting a clean tick", async () => {
    tradedCatalog(VOLUMES);

    const lines = await logsOf(truncated);

    expect(eventLines(lines, "prewarm")[0]).toMatchObject({
      attempted: 4,
      refreshed: 2,
      failed: 0,
      skipped: 2,
    });
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

  // A path no route matches: counted against the budget like any other request,
  // and answered without an upstream call, so a test can run the counter up
  // without also having to feed the upstream queue.
  const counted = "/v1/nonsense";

  // Small enough that a test can cross it, given the way the deployed worker
  // takes it: a binding, not a constant in the code.
  const capped = (limit: number) => ({ ...env, DAILY_REQUEST_BUDGET: limit }) as unknown as Env;

  const getCapped = (path: string, limit: number, budgetEnv = capped(limit)) => {
    const ctx = createExecutionContext();
    contexts.push(ctx);
    return worker.fetch(new Request(`${WORKER}${path}`), budgetEnv, ctx);
  };

  // Requests are counted in the isolate and reported behind the response, at
  // most once every few seconds. So a test drives the brake by letting the
  // interval elapse and then making the requests: the first of them carries
  // everything counted since the last report, and the object's answer is in
  // place by the time `settle` returns.
  const FLUSH_SECONDS = 5;
  async function report(limit: number, requests = 1) {
    advance(FLUSH_SECONDS);
    for (let i = 0; i < requests; i++) await getCapped(counted, limit);
    await settle();
  }

  // Wraps the binding so a test can see how many times the object was actually
  // written to, and with what batch each time.
  function countingBudget(limit: number) {
    const batches: number[] = [];
    const BUDGET = {
      idFromName: (name: string) => env.BUDGET.idFromName(name),
      get: (id: DurableObjectId) => {
        const stub = env.BUDGET.get(id);
        return {
          spend: (threshold: number, requests: number) => {
            batches.push(requests);
            return stub.spend(threshold, requests);
          },
        };
      },
    };
    return { budgetEnv: { ...capped(limit), BUDGET } as unknown as Env, batches };
  }

  it("serves normally under budget", async () => {
    upstream({ body });

    const response = await getCapped(route, 5);

    expect(response.status).toBe(200);
    expect(response.headers.get(UNAVAILABLE_HEADER)).toBeNull();
  });

  it("answers without waiting for the durable object to hear about the request", async () => {
    upstream({ body });

    // A budget of zero: the object will call this request over the threshold,
    // and it is still served, because the report goes out behind the response.
    const response = await getCapped(route, 0);
    expect(response.status).toBe(200);

    await settle();
    expect((await getCapped(counted, 0)).status).toBe(503);
  });

  it("reports a run of requests as one write rather than one per request", async () => {
    const { budgetEnv, batches } = countingBudget(100);

    for (let i = 0; i < 3; i++) await getCapped(counted, 100, budgetEnv);
    await settle();
    advance(FLUSH_SECONDS);
    await getCapped(counted, 100, budgetEnv);
    await settle();

    // Four requests, two writes: the first opens the interval carrying itself,
    // the second carries everything counted since.
    expect(batches).toEqual([1, 3]);
  });

  it("flips every route to the unavailable signal once the threshold is crossed", async () => {
    const mock = upstream();
    await report(1);
    await report(1);

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
    await report(0);

    const response = await getCapped("/v1/health", 0);

    expect(response.status).toBe(200);
  });

  it("restores service at the daily reset", async () => {
    upstream({ body });
    const limit = 2;
    for (let i = 0; i <= limit; i++) await report(limit);
    expect((await getCapped(route, limit)).status).toBe(503);

    advance(24 * 60 * 60);
    // The first request of the new day is what carries the report that finds
    // the counter reset; the one after it is served.
    await report(limit);

    expect((await getCapped(route, limit)).status).toBe(200);
  });

  it("marks the crossing once, however far past the threshold the batch carried it", async () => {
    upstream();
    const limit = 3;

    const under = await logsOf(() => report(limit));
    // Three more counted while the isolate held its last verdict, so the report
    // that follows steps the total from 2 straight to 5.
    const crossing = await logsOf(async () => {
      await report(limit, 3);
      await report(limit);
    });
    const after = await logsOf(() => report(limit));

    expect(eventLines(under, "budget_exceeded")).toEqual([]);
    expect(eventLines(crossing, "budget_exceeded")).toEqual([
      { event: "budget_exceeded", threshold: limit, count: 5 },
    ]);
    expect(eventLines(after, "budget_exceeded")).toEqual([]);
  });

  it("keeps the last verdict when the durable object cannot be reached", async () => {
    upstream({ body });
    const broken = {
      ...capped(1),
      BUDGET: {
        idFromName: () => {
          throw new Error("durable object unreachable");
        },
      },
    } as unknown as Env;

    // Nothing known yet, so the request is served: an unreachable counter is an
    // outage of the brake, not evidence the budget was spent.
    expect((await getCapped(route, 1, broken)).status).toBe(200);
    await settle();

    await report(1);
    await report(1);
    advance(FLUSH_SECONDS);

    // And once the brake is on, an unreachable object does not lift it.
    expect((await getCapped(counted, 1, broken)).status).toBe(503);
  });

  it("skips the cron prewarm over budget", async () => {
    const mock = upstream();
    const ctx = createExecutionContext();

    await worker.scheduled?.(createScheduledController(), capped(0), ctx);
    await waitOnExecutionContext(ctx);

    expect(mock.calls).toEqual([]);
  });

  it("asks the object itself on a cron tick rather than trusting a stale verdict", async () => {
    const mock = upstream();
    const limit = 5;
    await report(limit);

    // Spent elsewhere — another isolate, in the deployed worker. This one still
    // believes it is under budget.
    await env.BUDGET.get(env.BUDGET.idFromName("daily")).spend(limit, 10);
    expect((await getCapped(counted, limit)).status).toBe(404);

    const ctx = createExecutionContext();
    await worker.scheduled?.(createScheduledController(), capped(limit), ctx);
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
  it("reports the prewarm tick's counts, both cursors and snapshot size", async () => {
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
        skipped: 0,
        hot_attempted: 0,
        hot_refreshed: 0,
        hot_cursor: 0,
        hot_size: 0,
        cold_attempted: 2,
        cold_refreshed: 1,
        cold_cursor: 0,
        catalog_size: 2,
        entries: 1,
        duration_ms: expect.any(Number),
      },
    ]);
  });

  it("tells a hot tick's progress apart from the cold walk's", async () => {
    tradedCatalog({ a_prime_set: 9, b_prime_set: 8, c_prime_set: 7 });
    await runPrewarm(3);

    const lines = await logsOf(() =>
      runPrewarm(2, { PREWARM_HOT_BATCH_SIZE: 1, PREWARM_HOT_SIZE: 2 }),
    );

    expect(eventLines(lines, "prewarm")[0]).toMatchObject({
      hot_attempted: 1,
      hot_refreshed: 1,
      hot_cursor: 0,
      hot_size: 2,
      cold_attempted: 1,
      cold_refreshed: 1,
      cold_cursor: 0,
    });
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
        skipped: 0,
        hot_attempted: 0,
        hot_refreshed: 0,
        hot_cursor: 0,
        hot_size: 0,
        cold_attempted: 0,
        cold_refreshed: 0,
        cold_cursor: 0,
        catalog_size: 0,
        entries: 0,
        duration_ms: expect.any(Number),
      },
    ]);
  });

  it("names the upstream and what the caller got when a stale body is served", async () => {
    const route = "/v1/wfm/items/rhino_prime_set/orders";
    upstream({ body: { data: [] } }, { status: 503, body: { error: "down" } });
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
});
