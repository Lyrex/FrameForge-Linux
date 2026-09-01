// Steam news, which the app merges into its own worldstate view, is not served
// here: it is a separate upstream on a different cadence.

import { readThrough } from "../cache";
import { WORLDSTATE } from "../upstream";

export function worldstate(request: Request): Promise<Response> {
  return readThrough(request, WORLDSTATE, "worldstate");
}
