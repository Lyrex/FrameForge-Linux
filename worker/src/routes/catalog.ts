// TODO: only drop data is served. The item, recipe and public-export catalogs
// the app also pulls from GitHub belong here too, each one a line in the route
// table once its shape is pinned down.

import { readThrough, TTL } from "../cache";
import { DROP_DATA } from "../upstream";

// TODO: the drop catalog is ~30 MB and `readThrough` buffers it whole, then
// puts a clone — 60-90 MB resident against a 128 MB per-isolate ceiling, so a
// second concurrent refresh can push the isolate over. Either stream the body
// past the cache instead of buffering it, or serve the catalog split by
// category so no single response is that large.
export function drops(request: Request): Promise<Response> {
  return readThrough(request, DROP_DATA, TTL.catalog);
}
