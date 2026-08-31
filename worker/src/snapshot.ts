// The all-items price snapshot: one document that replaces a client's
// thousands of per-item price calls.
//
// Nothing is computed while a client waits. A cron tick walks a cursor over the
// item catalog, refreshes a bounded batch of prices, and writes the document to
// KV; serving it is a single KV read.

import { CACHE_ORIGIN, TTL } from "./cache";
import { items as catalogItems, statistics } from "./routes/wfm";
import type { CatalogItem } from "./routes/wfm";

const SNAPSHOT_KEY = "snapshot";
const CURSOR_KEY = "prewarm-cursor";

// `plat` is null for an item warframe.market knows but nobody is trading.
export type SnapshotEntry = { plat: number | null; at: number };

export type Snapshot = {
  // Unix seconds of the last prewarm write, and null when prewarm has never
  // run — a client can tell "no snapshot yet" from "snapshot with no prices".
  generation: number | null;
  items: Record<string, SnapshotEntry>;
};

const EMPTY: Snapshot = { generation: null, items: {} };

export async function snapshot(env: Env): Promise<Response> {
  const stored = await env.SNAPSHOT.get(SNAPSHOT_KEY);
  return new Response(stored ?? JSON.stringify(EMPTY), {
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": `public, max-age=${TTL.snapshot}`,
    },
  });
}

// One cron tick: refresh PREWARM_BATCH_SIZE items from the cursor onwards,
// wrapping at the end of the catalog, then advance the cursor. The batch is the
// only thing standing between us and warframe.market's rate limiter, so it is
// configuration rather than a number buried here.
export async function prewarm(env: Env): Promise<void> {
  const catalog = await catalogItems(new Request(`${CACHE_ORIGIN}/v1/wfm-items`));
  if (!catalog.ok) return;
  const { items } = (await catalog.json()) as { items: CatalogItem[] };
  if (items.length === 0) return;

  const batch = Math.min(env.PREWARM_BATCH_SIZE, items.length);
  // The catalog can gain and lose items between refreshes, so the cursor is
  // only an approximate position — it is re-anchored to the current length
  // rather than trusted to be in range.
  const cursor = (Number(await env.SNAPSHOT.get(CURSOR_KEY)) || 0) % items.length;
  const current = await load(env);

  for (let step = 0; step < batch; step++) {
    const slug = items[(cursor + step) % items.length]!.slug;
    const entry = await priceEntry(slug);
    // A failed fetch leaves the previous entry alone. A price from the last
    // pass is worth far more to a client than a hole.
    if (entry) current.items[slug] = entry;
  }

  current.generation = nowSeconds();
  await env.SNAPSHOT.put(SNAPSHOT_KEY, JSON.stringify(current));
  await env.SNAPSHOT.put(CURSOR_KEY, String((cursor + batch) % items.length));
}

async function load(env: Env): Promise<Snapshot> {
  const stored = await env.SNAPSHOT.get(SNAPSHOT_KEY);
  return stored ? (JSON.parse(stored) as Snapshot) : { generation: null, items: {} };
}

async function priceEntry(slug: string): Promise<SnapshotEntry | null> {
  const response = await statistics(
    new Request(`${CACHE_ORIGIN}/v1/wfm/items/${slug}/statistics`),
    slug,
  );
  // 404 is warframe.market's answer about the item — it is not traded — rather
  // than a failure to reach it, so it is recorded as a price of none.
  if (response.status === 404) return { plat: null, at: nowSeconds() };
  if (!response.ok) return null;

  return { plat: medianPlatinum((await response.json()) as Statistics), at: nowSeconds() };
}

type StatEntry = { median?: number; volume?: number };
type Statistics = {
  payload?: { statistics_closed?: { "48hours"?: StatEntry[]; "90days"?: StatEntry[] } };
};

// The same platinum price the app derives when it asks warframe.market itself,
// so a snapshot entry and a direct fetch never disagree.
function medianPlatinum(body: Statistics): number | null {
  const closed = body.payload?.statistics_closed;
  const recent = closed?.["48hours"] ?? [];
  const long = closed?.["90days"] ?? [];

  // Under three trades in two days the recent window is one person's mood, so
  // the 90-day window decides instead.
  const volume = recent.reduce((sum, entry) => sum + (entry.volume ?? 0), 0);
  const price = volume >= 3 ? trimmedMedian(recent) : null;
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
