// The all-items price snapshot: one document that replaces a client's
// thousands of per-item price calls.
//
// Nothing is computed while a client waits. A cron tick walks two cursors over
// the item catalog, refreshes a bounded batch of prices, and writes the document
// to KV; serving it is a single KV read.

import { CACHE_ORIGIN, TTL } from "./cache";
import { items as catalogItems, statistics } from "./routes/wfm";
import type { CatalogItem } from "./routes/wfm";
import { isValidSlug } from "./upstream";

const SNAPSHOT_KEY = "snapshot";
const COLD_CURSOR_KEY = "prewarm-cursor";
const HOT_CURSOR_KEY = "prewarm-hot-cursor";

// `plat` is null for an item warframe.market knows but nobody is trading. `vol`
// is absent for an item no tick has reached yet, which is not the same as a
// measured zero: unmeasured is not evidence of anything, an untraded item is.
export type SnapshotEntry = { plat: number | null; at: number; vol?: number };

export type Snapshot = {
  // Unix seconds of the last prewarm write, and null when prewarm has never
  // run — a client can tell "no snapshot yet" from "snapshot with no prices".
  generation: number | null;
  items: Record<string, SnapshotEntry>;
};

const EMPTY: Snapshot = { generation: null, items: {} };

// An invocation may only make so many subrequests, and the runtime throws on
// the one past the limit rather than queueing it, so a tick has to count what it
// spends. A fetch costs one, and so does every Cache API match and put; KV reads
// and writes cost nothing.
//
// A price through the statistics route is a match, a fetch and a put. A cached
// one is only the match, but a tick that assumed the cheap case would be
// budgeting on the hope that its work was already done.
const SUBREQUESTS_PER_ITEM = 3;

// What a tick spends before it refreshes a single price: the daily-budget
// object it was already asked about, and the catalog read-through.
const TICK_OVERHEAD_SUBREQUESTS = 4;

// Prices in flight at once. warframe.market asks for about three requests a
// second across a public endpoint and this worker is one client standing in for
// every install, so the walk keeps three in the air rather than firing a batch
// of hundreds at once — sequential would leave a 240-item tick waiting on round
// trips it could have overlapped.
const UPSTREAM_CONCURRENCY = 3;

export async function snapshot(env: Env): Promise<Response> {
  const stored = await env.SNAPSHOT.get(SNAPSHOT_KEY);
  return new Response(stored ?? JSON.stringify(EMPTY), {
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": `public, max-age=${TTL.snapshot}`,
    },
  });
}

// One cron tick, split between two walks over the same catalog.
//
// The hot walk covers the items being traded most, so the prices people
// actually look at are the freshest ones; the cold walk carries on through the
// catalog in order, so an item nobody trades is refreshed slowly rather than
// never. Both the batch and the split are configuration — they are what stands
// between us and warframe.market's rate limiter.
//
// At the shipped values, 42 hot and 198 cold every five minutes: the hot set of
// 500 laps in about 59 minutes, just inside the hour a statistics body stays
// fresh, and the ~3840-item catalog laps in about an hour and 40. That is 240
// prices, 724 of the invocation's 1000 subrequests, and one upstream request
// every 1.25 seconds averaged over the tick.
export async function prewarm(env: Env): Promise<void> {
  const started = Date.now();
  const current = await load(env);

  let allowance = Number(env.PREWARM_SUBREQUEST_CEILING) - TICK_OVERHEAD_SUBREQUESTS;

  const catalog = await catalogItems(new Request(`${CACHE_ORIGIN}/v1/wfm-items`));
  const listed = catalog.ok ? ((await catalog.json()) as { items: CatalogItem[] }).items : [];
  // A slug from upstream still builds a URL here, so it passes the same check a
  // client's would.
  const items = listed.map((item) => item.slug).filter(isValidSlug);

  if (items.length === 0) {
    // A tick with no catalog to walk still reports itself. It is the stall an
    // operator has to see — every route keeps answering from a snapshot that
    // quietly stops advancing — and no line at all is indistinguishable from a
    // cron that never fired.
    logTick(started, current, {
      hot: { attempted: 0, walked: 0, refreshed: 0, cursor: 0 },
      cold: { attempted: 0, walked: 0, refreshed: 0, cursor: 0 },
      hot_size: 0,
      catalog_size: 0,
    });
    return;
  }

  const hot = hotSet(current, Number(env.PREWARM_HOT_SIZE));
  const hotShare = Number(env.PREWARM_HOT_BATCH_SIZE);
  const coldShare = Math.max(Number(env.PREWARM_BATCH_SIZE) - hotShare, 0);

  const hotCursor = hot.length === 0 ? 0 : (await cursor(env, HOT_CURSOR_KEY)) % hot.length;
  const coldCursor = (await cursor(env, COLD_CURSOR_KEY)) % items.length;

  const hotRun = await walk(current, hot, hotCursor, Math.min(hotShare, hot.length), allowance);
  allowance -= hotRun.walked * SUBREQUESTS_PER_ITEM;
  const coldRun = await walk(
    current,
    items,
    coldCursor,
    Math.min(coldShare, items.length),
    allowance,
  );

  // Written before anything can throw off the back of the walks, and advanced
  // by what the tick actually finished rather than what it set out to do: a
  // tick that ran out of subrequests must still leave the next one further
  // along than it started, or the walk stalls on the same item forever.
  current.generation = nowSeconds();
  await env.SNAPSHOT.put(SNAPSHOT_KEY, JSON.stringify(current));
  if (hot.length > 0) {
    await env.SNAPSHOT.put(HOT_CURSOR_KEY, String((hotCursor + hotRun.walked) % hot.length));
  }
  await env.SNAPSHOT.put(COLD_CURSOR_KEY, String((coldCursor + coldRun.walked) % items.length));

  logTick(started, current, {
    hot: { ...hotRun, cursor: hotCursor },
    cold: { ...coldRun, cursor: coldCursor },
    hot_size: hot.length,
    catalog_size: items.length,
  });
}

// `attempted` is the batch the lane was asked for, `walked` what its share of
// the subrequests paid for, and `refreshed` how many of those came back.
type Run = { attempted: number; walked: number; refreshed: number };

// A lane's run and where in its list the lane started.
type Lane = Run & { cursor: number };

// Refreshes `attempted` items from `cursor` on, wrapping at the end of the list,
// and stops short of what `allowance` subrequests will pay for. A slug whose
// fetch failed is still walked past: an item upstream answers 500 for every time
// would otherwise hold the cursor still and starve everything behind it.
async function walk(
  snapshot: Snapshot,
  slugs: string[],
  cursor: number,
  attempted: number,
  allowance: number,
): Promise<Run> {
  const run: Run = { attempted, walked: 0, refreshed: 0 };
  const affordable = Math.min(attempted, Math.floor(allowance / SUBREQUESTS_PER_ITEM));

  while (run.walked < affordable) {
    const together = Math.min(UPSTREAM_CONCURRENCY, affordable - run.walked);
    const batch = Array.from(
      { length: together },
      (_, offset) => slugs[(cursor + run.walked + offset) % slugs.length]!,
    );

    // The budget above is what keeps a tick inside its subrequest ceiling, but
    // it is an estimate of the cost, and the runtime's answer to the request
    // past the limit is an exception. Losing the chunk is survivable; losing
    // everything the tick had already refreshed, and the cursor with it, is what
    // leaves the walk restarting from the same place forever.
    let entries: (readonly [string, SnapshotEntry | null])[];
    try {
      entries = await Promise.all(
        batch.map(async (slug) => [slug, await priceEntry(slug)] as const),
      );
    } catch {
      break;
    }
    for (const [slug, entry] of entries) {
      // A failed fetch leaves the previous entry alone. A price from the last
      // pass is worth far more to a client than a hole.
      if (entry) {
        snapshot.items[slug] = entry;
        run.refreshed++;
      }
    }
    run.walked += together;
  }
  return run;
}

// The items worth keeping fresh: the busiest by the volume the last refresh
// recorded. It comes out of the snapshot the tick already holds, so ranking
// costs no upstream call and needs no list anybody has to curate.
//
// An item with no volume yet has not been measured, and a measured zero is one
// nobody is trading; neither has earned a place in a lane that exists to keep
// traded prices current.
function hotSet(snapshot: Snapshot, size: number): string[] {
  return Object.entries(snapshot.items)
    .filter(([, entry]) => (entry.vol ?? 0) > 0)
    .sort((left, right) => (right[1].vol ?? 0) - (left[1].vol ?? 0))
    .slice(0, size)
    .map(([slug]) => slug);
}

// One line per tick. Failures are a count, never the slugs behind them: a
// prewarm walks the catalog on its own schedule and its slugs are nobody's
// lookup, but the count is what an operator acts on and one blanket rule about
// slugs in logs is easier to keep than two.
//
// `skipped` is the batch the tick never got to, which is a subrequest ceiling
// too low for the configured batch rather than an upstream that failed —
// counting the two together would read as a bad afternoon at warframe.market.
function logTick(
  started: number,
  snapshot: Snapshot,
  tick: { hot: Lane; cold: Lane; hot_size: number; catalog_size: number },
): void {
  const attempted = tick.hot.attempted + tick.cold.attempted;
  const walked = tick.hot.walked + tick.cold.walked;
  const refreshed = tick.hot.refreshed + tick.cold.refreshed;

  console.log(
    JSON.stringify({
      event: "prewarm",
      attempted,
      refreshed,
      failed: walked - refreshed,
      skipped: attempted - walked,
      hot_attempted: tick.hot.attempted,
      hot_refreshed: tick.hot.refreshed,
      hot_cursor: tick.hot.cursor,
      hot_size: tick.hot_size,
      cold_attempted: tick.cold.attempted,
      cold_refreshed: tick.cold.refreshed,
      cold_cursor: tick.cold.cursor,
      catalog_size: tick.catalog_size,
      entries: Object.keys(snapshot.items).length,
      duration_ms: Date.now() - started,
    }),
  );
}

async function load(env: Env): Promise<Snapshot> {
  const stored = await env.SNAPSHOT.get(SNAPSHOT_KEY);
  return stored ? (JSON.parse(stored) as Snapshot) : { generation: null, items: {} };
}

// The catalog can gain and lose items between refreshes, so a stored cursor is
// only an approximate position — the caller re-anchors it to the current length
// rather than trusting it to be in range.
async function cursor(env: Env, key: string): Promise<number> {
  return Number(await env.SNAPSHOT.get(key)) || 0;
}

// Through the worker's own statistics route rather than straight to
// warframe.market, so the tick leaves the per-item chart body in the edge cache
// as well as the price in the snapshot: the request that asks for that chart
// later is then answered without an upstream call. No `ctx` reaches here, so the
// refresh runs in the foreground — a tick has nowhere to defer work to.
async function priceEntry(slug: string): Promise<SnapshotEntry | null> {
  const response = await statistics(
    new Request(`${CACHE_ORIGIN}/v1/wfm/items/${slug}/statistics`),
    slug,
  );

  // 404 is warframe.market's answer about the item — it is not traded — rather
  // than a failure to reach it, so it is recorded as a price of none.
  if (response.status === 404) return { plat: null, at: nowSeconds(), vol: 0 };
  if (!response.ok) return null;

  const body = (await response.json()) as Statistics;
  return { plat: medianPlatinum(body), at: nowSeconds(), vol: recentVolume(body) };
}

type StatEntry = { median?: number; volume?: number };
type Statistics = {
  payload?: { statistics_closed?: { "48hours"?: StatEntry[]; "90days"?: StatEntry[] } };
};

// Trades over the last 48 hours: the same window the price derivation tests for
// thinness, so what counts as recent means one thing across the whole document.
// One number rather than a history, because every install downloads the
// snapshot whole.
function recentVolume(body: Statistics): number {
  const recent = body.payload?.statistics_closed?.["48hours"] ?? [];
  return recent.reduce((sum, entry) => sum + (entry.volume ?? 0), 0);
}

// The same platinum price the app derives when it asks warframe.market itself,
// so a snapshot entry and a direct fetch never disagree.
function medianPlatinum(body: Statistics): number | null {
  const closed = body.payload?.statistics_closed;
  const recent = closed?.["48hours"] ?? [];
  const long = closed?.["90days"] ?? [];

  // Under three trades in two days the recent window is one person's mood, so
  // the 90-day window decides instead.
  const price = recentVolume(body) >= 3 ? trimmedMedian(recent) : null;
  return price ?? trimmedMedian(long);
}

function trimmedMedian(entries: StatEntry[]): number | null {
  const prices = entries
    .map((entry) => entry.median ?? 0)
    .filter((price) => price > 0)
    .sort((a, b) => a - b);
  if (prices.length === 0) return null;

  // Drop the outer 15% at each end: a thin market's daily medians include days
  // moved by a single absurd listing.
  const cut = Math.floor(prices.length * 0.15);
  const trimmed = cut * 2 < prices.length ? prices.slice(cut, prices.length - cut) : prices;

  const mid = trimmed.length >> 1;
  const median =
    trimmed.length % 2 === 0 ? (trimmed[mid - 1]! + trimmed[mid]!) / 2 : trimmed[mid]!;
  return Math.round(median);
}

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}
