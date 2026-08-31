# FrameForge cache worker

A Cloudflare Worker that sits in front of FrameForge's public upstreams so that
every install's identical requests collapse into one fetch at the edge.

It serves public data only. No account data, no telemetry, no per-user state:
warframe.market logins, a user's own orders and auctions, and official API sync
stay on the user's machine and never route through here.

## Contract

Everything lives under `/v1/`. Per-item bodies are relayed from the upstream
unchanged, so a client parses exactly what it would parse talking to the
upstream direct; the catalog and the snapshot are worker-native shapes.

| Method | Path | Upstream | Fresh for |
| --- | --- | --- | --- |
| GET | `/v1/wfm/items/:slug/statistics` | `api.warframe.market/v1/items/:slug/statistics` | 5 min |
| GET | `/v1/wfm/items/:slug/orders` | `api.warframe.market/v2/orders/item/:slug` | 30 s |
| GET | `/v1/worldstate` | `api.warframe.com/cdn/worldState.php` | 45 s |
| GET | `/v1/catalog/drops` | `warframe-drop-data/.../all.json` | 6 h, revalidated with `If-None-Match` |
| GET | `/v1/wfm-items` | `api.warframe.market/v2/items`, reshaped | 6 h |
| GET | `/v1/snapshot` | none — built by the cron prewarm | 5 min |

Prices and statistics are one upstream document, so one route serves both.

### `/v1/wfm-items`

The tradable-item catalog, cut down to what a client needs to search by name
and address an item. Search itself stays client-side.

```json
{
  "items": [
    { "slug": "mirage_prime_set", "name": "Mirage Prime Set", "id": "54a73e65e779893a797f0f0f" }
  ]
}
```

An upstream item with no slug or no English name is dropped.

### `/v1/snapshot`

Every item's platinum price in one document, so a client downloads one body
instead of making thousands of per-item calls.

```json
{
  "generation": 1756713600,
  "items": {
    "mirage_prime_set": { "plat": 120, "at": 1756713600 },
    "nikana_prime_set": { "plat": null, "at": 1756695300 }
  }
}
```

`generation` is the Unix second of the last prewarm write, and `null` when
prewarm has never run — an empty snapshot a client can recognise as "nothing
here yet" rather than "nothing is worth anything". `at` is when that item's
price was last refreshed, which for the oldest entry is one full prewarm pass
ago. `plat` is `null` for an item warframe.market knows but nobody trades.

The price is the same number the app derives when it asks warframe.market
itself: the trimmed median of the daily medians over the last 48 hours when
those days hold at least three sales, and over the last 90 days otherwise.

Serving the snapshot is one read of stored state. It never fetches an upstream,
so a request costs nothing upstream however cold the edge is.

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

### Prewarm

A cron trigger refreshes the snapshot incrementally. Each tick reads a cursor
from KV, refreshes `PREWARM_BATCH_SIZE` items from there on, wraps at the end
of the catalog, and stores the cursor for the next tick. Both the schedule and
the batch size live in `wrangler.jsonc`; at 60 items every 5 minutes a
~4000-item catalog is walked end to end in about five hours, which is the
oldest an entry's `at` gets.

An item whose fetch fails keeps the entry the last pass gave it — a price from
a few hours ago beats a hole — so `generation` advancing does not mean every
entry was rewritten.

State lives in the `SNAPSHOT` KV namespace under two keys: `snapshot` for the
document, `prewarm-cursor` for the position. Both are public, item-keyed data.

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
