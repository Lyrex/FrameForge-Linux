import { afterEach, describe, expect, it, vi } from "vitest";

import worker from "../src/index";
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

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
  seenHeaders = [];
});

const get = (path: string, headers: HeadersInit = {}) =>
  worker.fetch(new Request(`${WORKER}${path}`, { headers }));

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
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(Date.now() + 24 * 60 * 60 * 1000);
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
