// The public upstreams the worker is allowed to reach.
//
// Every URL here serves the same bytes to everybody. Nothing account-derived
// belongs in this file: logins, a user's own orders and auctions, and official
// API sync stay on the user's machine and never route through the worker.

export const USER_AGENT = "FrameForge-Worker (https://github.com/Lyrex/FrameForge)";

// Long enough for a slow origin, short enough that a hung upstream still
// leaves room to answer from the stale copy.
export const UPSTREAM_TIMEOUT_MS = 10_000;

const WFM_API = "https://api.warframe.market";

// v2 is the only item list warframe.market still serves; v1 /items 404s.
export const WFM_ITEMS = `${WFM_API}/v2/items`;

export const wfmStatistics =(slug: string) => `${WFM_API}/v1/items/${slug}/statistics`;

export const wfmOrders = (slug: string) => `${WFM_API}/v2/orders/item/${slug}`;

export const WORLDSTATE = "https://api.warframe.com/cdn/worldState.php";

export const DROP_DATA =
  "https://raw.githubusercontent.com/WFCD/warframe-drop-data/gh-pages/data/all.json";

// warframe.market slugs are lowercase snake_case identifiers. Rejecting
// anything else keeps a crafted slug from walking the path into another
// upstream endpoint.
export function isValidSlug(slug: string): boolean {
  return /^[a-z0-9_]{1,120}$/.test(slug);
}
