// FrameForge public-data cache worker.
//
// The contract lives under /v1/. It carries public data only — the same bytes
// for every caller — so nothing here reads a request header beyond the method
// and path, and nothing a client sends is relayed to an upstream.
//
// Clients identify their release with X-FrameForge-Version. That is the only
// client-identifying header the contract has, and no route requires it.

import { drops } from "./routes/catalog";
import { orders, statistics } from "./routes/wfm";
import { worldstate } from "./routes/worldstate";
import { jsonError } from "./unavailable";

type Handler = (request: Request, groups: Record<string, string | undefined>) => Response | Promise<Response>;

const routes: { pattern: URLPattern; handle: Handler }[] = [
  {
    pattern: new URLPattern({ pathname: "/v1/wfm/items/:slug/statistics" }),
    handle: (request, groups) => statistics(request, groups.slug ?? ""),
  },
  {
    pattern: new URLPattern({ pathname: "/v1/wfm/items/:slug/orders" }),
    handle: (request, groups) => orders(request, groups.slug ?? ""),
  },
  {
    pattern: new URLPattern({ pathname: "/v1/worldstate" }),
    handle: (request) => worldstate(request),
  },
  {
    pattern: new URLPattern({ pathname: "/v1/catalog/drops" }),
    handle: (request) => drops(request),
  },
];

export default {
  async fetch(request: Request): Promise<Response> {
    if (request.method !== "GET" && request.method !== "HEAD") {
      return jsonError(405, "method_not_allowed");
    }

    for (const route of routes) {
      const match = route.pattern.exec(request.url);
      if (match) return route.handle(request, match.pathname.groups);
    }

    return jsonError(404, "not_found");
  },
} satisfies ExportedHandler;
