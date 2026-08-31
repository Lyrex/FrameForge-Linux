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

type UpstreamReply = { status?: number; body?: unknown; throws?: boolean };

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
      headers: { "Content-Type": "application/json" },
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

// Each cache class is exercised through its own route, so the TTL that route
// was given is the one under test.
const CLASSES = [
  {
    name: "order books",
    route: "/v1/wfm/items/mirage_prime_set/orders",
    upstream: "https://api.warframe.market/v2/orders/item/mirage_prime_set",
    body: { data: [{ platinum: 120, order_type: "sell" }] },
  },
  {
    name: "prices and statistics",
    route: "/v1/wfm/items/mirage_prime_set/statistics",
    upstream: "https://api.warframe.market/v1/items/mirage_prime_set/statistics",
    body: { payload: { statistics_closed: { "48hours": [{ avg_price: 118 }] } } },
  },
  {
    name: "worldstate",
    route: "/v1/worldstate",
    upstream: "https://api.warframe.com/cdn/worldState.php",
    body: { WorldSeed: "seed", ActiveMissions: [] },
  },
  {
    name: "static catalog",
    route: "/v1/catalog/drops",
    upstream:
      "https://raw.githubusercontent.com/WFCD/warframe-drop-data/gh-pages/data/all.json",
    body: { missionRewards: {} },
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
      items: [{ slug: "mirage_prime_set", name: "MIRAGE_PRIME_SET", id: "id_mirage_prime_set" }],
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
