// warframe.market public item data. Bodies are relayed unchanged so a client
// parses exactly what it would parse talking to warframe.market directly.

import { readThrough, TTL } from "../cache";
import { jsonError } from "../unavailable";
import { isValidSlug, wfmOrders, wfmStatistics } from "../upstream";

// Prices and statistics are one upstream document, so one route serves both.
export function statistics(request: Request, slug: string): Promise<Response> | Response {
  if (!isValidSlug(slug)) return jsonError(400, "invalid_slug");
  return readThrough(request, wfmStatistics(slug), TTL.statistics);
}

export function orders(request: Request, slug: string): Promise<Response> | Response {
  if (!isValidSlug(slug)) return jsonError(400, "invalid_slug");
  return readThrough(request, wfmOrders(slug), TTL.orders);
}
