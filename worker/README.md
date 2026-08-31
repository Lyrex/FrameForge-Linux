# FrameForge cache worker

A Cloudflare Worker that sits in front of FrameForge's public upstreams so that
every install's identical requests collapse into one fetch at the edge.

It serves public data only. No account data, no telemetry, no per-user state:
warframe.market logins, a user's own orders and auctions, and official API sync
stay on the user's machine and never route through here.

## Contract

Everything lives under `/v1/`. Bodies are relayed from the upstream unchanged,
so a client parses exactly what it would parse talking to the upstream direct.

| Method | Path | Upstream | Fresh for |
| --- | --- | --- | --- |
| GET | `/v1/wfm/items/:slug/statistics` | `api.warframe.market/v1/items/:slug/statistics` | 5 min |
| GET | `/v1/wfm/items/:slug/orders` | `api.warframe.market/v2/orders/item/:slug` | 30 s |
| GET | `/v1/worldstate` | `api.warframe.com/cdn/worldState.php` | 45 s |
| GET | `/v1/catalog/drops` | `warframe-drop-data/.../all.json` | 6 h, revalidated with `If-None-Match` |

Prices and statistics are one upstream document, so one route serves both.

`:slug` is a warframe.market slug — lowercase, digits and underscores. Anything
else is `400 {"error":"invalid_slug"}`.

Every response carries `X-FrameForge-Cache: hit | miss | stale | revalidated`.

Clients identify their release with `X-FrameForge-Version`. That is the only
client-identifying header the contract has; no route requires it, and no
request header of any kind is relayed to an upstream.

### Errors

| Status | Body | Meaning |
| --- | --- | --- |
| 400 | `{"error":"invalid_slug"}` | The slug is not a warframe.market slug |
| 404 | `{"error":"not_found"}` | No such route |
| 405 | `{"error":"method_not_allowed"}` | The contract is read-only |
| 502 | `{"error":"upstream_unreachable"}` | Upstream failed and nothing was cached |
| 503 | `{"error":"worker_unavailable"}` + `X-FrameForge-Worker: unavailable` | Stand down and use the upstreams direct |

The last one is the only signal a client acts on structurally: on seeing it, it
stops using the worker and fetches upstreams directly. Everything else is
ordinary HTTP. Nothing emits it yet.

### Freshness and failure

A fresh cached body is served without touching the upstream. A stale or absent
one costs one upstream fetch. If that fetch fails — timeout, connection error,
or 5xx — the last cached body is served instead of the failure, marked `stale`;
with nothing cached, the upstream's error reaches the client.

A 4xx is the upstream's answer about the item rather than a failure to reach
it, so it is passed through and not cached.

## Local development

```sh
npm install          # in this directory
npm run dev          # wrangler dev on http://127.0.0.1:8787
npm test
npm run typecheck
```

From the repository root, `npm run worker:dev` and `npm run worker:test` do the
same.

### Demoing the cache

Miss, then hit:

```sh
curl -si http://127.0.0.1:8787/v1/worldstate | grep X-FrameForge-Cache
# X-FrameForge-Cache: miss
curl -si http://127.0.0.1:8787/v1/worldstate | grep X-FrameForge-Cache
# X-FrameForge-Cache: hit
```

Stale-if-error, using the order book because its 30-second freshness is short
enough to wait out:

```sh
curl -si http://127.0.0.1:8787/v1/wfm/items/mirage_prime_set/orders | grep X-FrameForge-Cache
sleep 31
# Now take the upstream away — disconnect the network, or block
# api.warframe.market in /etc/hosts — and ask again:
curl -si http://127.0.0.1:8787/v1/wfm/items/mirage_prime_set/orders | grep -E 'HTTP|X-FrameForge-Cache'
# HTTP/1.1 200 OK
# X-FrameForge-Cache: stale
```

Unknown route:

```sh
curl -si http://127.0.0.1:8787/v1/nope
# HTTP/1.1 404 Not Found
# {"error":"not_found"}
```
