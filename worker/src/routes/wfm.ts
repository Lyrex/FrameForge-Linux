// warframe.market public item data. Per-slug bodies are relayed unchanged so a
// client parses exactly what it would parse talking to warframe.market
// directly; the item catalog is the one worker-native shape here.

import { readThrough, TTL } from "../cache";
import { jsonError } from "../unavailable";
import { isValidSlug, WFM_ITEMS, wfmOrders, wfmStatistics } from "../upstream";

// Prices and statistics are one upstream document, so one route serves both.
export function statistics(request: Request, slug: string): Promise<Response> | Response {
  if (!isValidSlug(slug)) return jsonError(400, "invalid_slug");
  return readThrough(request, wfmStatistics(slug), TTL.statistics);
}

export function orders(request: Request, slug: string): Promise<Response> | Response {
  if (!isValidSlug(slug)) return jsonError(400, "invalid_slug");
  return readThrough(request, wfmOrders(slug), TTL.orders);
}

export type CatalogItem = { slug: string; name: string };

type UpstreamItems = { data?: { slug?: string; i18n?: { en?: { name?: string } } }[] };

// The catalog every client downloads to search by name locally. Upstream sends
// every localisation and a pile of per-item detail; a client needs the slug to
// address the item and the English name to search on.
export async function items(request: Request): Promise<Response> {
  const upstream = await readThrough(request, WFM_ITEMS, TTL.catalog);
  if (!upstream.ok) return upstream;

  const body = (await upstream.json()) as UpstreamItems;
  const catalog: CatalogItem[] = [];
  for (const item of body.data ?? []) {
    const name = item.i18n?.en?.name;
    // An item with no slug or no English name cannot be addressed or searched,
    // so it is dropped rather than carried as a hole in the catalog.
    if (item.slug && name) catalog.push({ slug: item.slug, name });
  }

  return new Response(JSON.stringify({ items: catalog }), {
    headers: {
      "Content-Type": "application/json",
      "Cache-Control": upstream.headers.get("Cache-Control") ?? "",
      "X-FrameForge-Cache": upstream.headers.get("X-FrameForge-Cache") ?? "",
    },
  });
}
