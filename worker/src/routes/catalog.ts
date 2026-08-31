// TODO: only drop data is served. The item, recipe and public-export catalogs
// the app also pulls from GitHub belong here too, each one a line in the route
// table once its shape is pinned down.

import { readThrough, TTL } from "../cache";
import { DROP_DATA } from "../upstream";

export function drops(request: Request): Promise<Response> {
  return readThrough(request, DROP_DATA, TTL.catalog);
}
