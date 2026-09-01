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
| GET | `/v1/wfm/items/:slug/statistics` | `api.warframe.market/v1/items/:slug/statistics` | 1 h, then served stale while it refreshes |
| GET | `/v1/wfm/items/:slug/orders` | `api.warframe.market/v2/orders/item/:slug` | 30 s |
| GET | `/v1/worldstate` | `api.warframe.com/cdn/worldState.php` | 45 s |
| GET | `/v1/catalog/drops` | `warframe-drop-data/.../all.json` | 6 h, then served stale while it refreshes; revalidated with `If-None-Match` |
| GET | `/v1/wfm-items` | `api.warframe.market/v2/items`, reshaped | 6 h, then served stale while it refreshes |
| GET | `/v1/snapshot` | none — built by the cron prewarm | 5 min |
| GET | `/v1/health` | none | not cached |

Prices and statistics are one upstream document, so one route serves both.

### `/v1/wfm-items`

The tradable-item catalog, cut down to what a client needs to search by name
and address an item. Search itself stays client-side.

```json
{
  "items": [
    { "slug": "mirage_prime_set", "name": "Mirage Prime Set" }
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
    "mirage_prime_set": { "plat": 120, "at": 1756713600, "vol": 418 },
    "nikana_prime_set": { "plat": null, "at": 1756695300, "vol": 0 }
  }
}
```

`generation` is the Unix second of the last prewarm write, and `null` when
prewarm has never run — an empty snapshot a client can recognise as "nothing
here yet" rather than "nothing is worth anything". `at` is when that item's
price was last refreshed. `plat` is `null` for an item warframe.market knows but
nobody trades.

`vol` is that item's sales over the last 48 hours, which is what ranks it for
prewarming; it is absent for an item no tick has reached yet, and `0` for one
that was measured and is not being traded. It is one number rather than a
history because every install downloads this document whole.

The price is the same number the app derives when it asks warframe.market
itself: the trimmed median of the daily medians over the last 48 hours when
those days hold at least three sales, and over the last 90 days otherwise.

Serving the snapshot is one read of stored state. It never fetches an upstream,
so a request costs nothing upstream however cold the edge is.

### `/v1/health`

```json
{ "status": "ok" }
```

Liveness, nothing more: no upstream call, no stored state, and no report on the
data being served. It answers whatever else is wrong, including while the
worker is standing down on a spent budget — an operator has to be able to tell
"stood down" from "gone". It is the one route the budget neither blocks nor
counts.

`:slug` is a warframe.market slug — lowercase, digits and underscores. Anything
else is `400 {"error":"invalid_slug"}`.

Every response carries `X-FrameForge-Cache: hit | miss | stale | revalidated |
revalidating`, and one carries the upstream's `ETag` whenever the upstream gave one. A client
that sends that value back as `If-None-Match` gets `304` with no body — worth
~30 MB a refresh on the drop catalog.

`If-None-Match` is the one request header the worker reads: a validator says
which bytes the caller already has, not who the caller is. It is not relayed
either — the validator sent upstream is always the worker's own cached one.

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
stops using the worker and fetches upstreams directly until the next UTC
midnight. Everything else is ordinary HTTP. The daily budget is what emits it.

### Freshness and failure

A fresh cached body is served without touching the upstream. A stale or absent
one costs one upstream fetch. If that fetch fails — timeout, connection error,
or 5xx — the last cached body is served instead of the failure, marked `stale`;
with nothing cached, the upstream's error reaches the client.

A 4xx is the upstream's answer about the item rather than a failure to reach
it, so it is passed through and not cached.

### Stale while revalidating

Statistics and the catalogs go further: past its freshness, the body already
held is served straight away, marked `revalidating`, and the refresh runs after
the response has gone. Nobody waits on warframe.market for bytes the edge
already has, and with no requests at all it is the prewarm cycle that keeps them
current. Many callers arriving on the same expired entry start one refresh
between them, not one each.

Order books are deliberately excluded. A trader acts on an order book, so an
expired one blocks on the upstream rather than handing back a book from a minute
ago, and nothing prewarms them either. The worldstate is excluded for a
different reason: every entry in it is a window that expires and nothing
refreshes it in the background, so the first request after a quiet spell would
otherwise be answered from a fossil.

### Prewarm

A cron trigger refreshes the snapshot incrementally, splitting each tick between
two walks over the same catalog:

- the **hot** walk covers the `PREWARM_HOT_SIZE` items with the highest recorded
  `vol`, so the prices people actually look at are the freshest;
- the **cold** walk carries on through the whole catalog in order, so an item
  nobody trades is refreshed slowly rather than never.

Each has its own cursor in KV and its own share of `PREWARM_BATCH_SIZE`. At the
shipped values — 240 items every 5 minutes, 42 of them hot — the 500-item hot
set laps in about 59 minutes, inside the hour a price stays fresh, and the
~3840-item catalog laps in about an hour and 40. That is one request to
warframe.market every 1.25 seconds averaged over the tick, with at most three in
flight at once.

A tick refreshes through the same cache a request uses, so it warms the per-item
statistics body as well as writing the price into the snapshot.

An item whose fetch fails keeps the entry the last pass gave it — a price from
a few hours ago beats a hole — so `generation` advancing does not mean every
entry was rewritten.

#### The subrequest ceiling

One invocation may only make so many subrequests: 1000 on the paid plan, 50 on
free. A fetch counts, and so does every Cache API match and put, which makes a
price three. `PREWARM_SUBREQUEST_CEILING` says which limit applies, and a tick
stops cleanly short of it rather than letting the runtime throw on the request
past the limit. The shipped batch spends 724 of 1000 and leaves the rest as
headroom for a catalog fetch and a slow item.

A tick that runs out persists what it managed and advances each cursor by the
items it actually completed, so the next tick carries on from there instead of
restarting at the same place; the batch it never got to is `skipped` in the log
line. Dropping to the free plan means setting the ceiling to 50 and the batch to
about 15.

State lives in the `SNAPSHOT` KV namespace under three keys: `snapshot` for the
document, `prewarm-cursor` and `prewarm-hot-cursor` for the two positions. All
of it is public, item-keyed data.

## Daily budget

Every request counts against one shared daily total. Past
`DAILY_REQUEST_BUDGET` requests, every route answers the unavailable signal and
the cron prewarm does nothing at all, until the counter resets at UTC midnight —
the same instant the client comes back. So a bug or an abusive caller costs a
bounded amount, and the app carries on against the upstreams meanwhile.

The counter lives in the `BUDGET` Durable Object (class `DailyBudget`), because
it has to be one number across every edge location that serves a request; KV's
eventual consistency would read minutes-old totals in exactly the burst the
brake exists to stop. If that object is unreachable the request is served
anyway: an outage of the brake is not evidence the budget was spent, and
locking every client out is the worse failure.

`/v1/health` is outside all of this — neither counted nor blocked.

The default is 100,000/day, the free plan's own ceiling, so the brake trips
before the platform does. Change it in `wrangler.jsonc` and deploy, or on a
running worker without touching the code: Workers → `frameforge-cache` →
Settings → Variables and Secrets → `DAILY_REQUEST_BUDGET`. Saving there rolls a
new version of the same code rather than rebuilding it, but the next
`wrangler deploy` puts the file's value back, so keep the two in step.

## Logs

Everything is a JSON line on `console.log`, which Workers Logs ingests and
makes queryable field by field. One line per request, and only these fields:

```json
{ "route": "/v1/wfm/items/:slug/statistics", "method": "GET", "status": 200, "latency_ms": 41, "version": "3.9.0", "cache": "hit" }
```

`route` is the pattern, never the path: a line naming the slug someone asked
for is a record of what that person was looking up. A request that matched no
pattern — a stray trailing slash is enough — logs `"unmatched"` rather than the
path it asked for. `version` is the client's `X-FrameForge-Version`, or `null`.
`cache` is the `X-FrameForge-Cache` outcome — `hit`, `miss`, `stale`,
`revalidated`, `revalidating` for a body served from the cache while it is
refreshed behind the response, `not_modified` when the caller's own
`If-None-Match` matched, and `null` for a route that never consults the cache. Nothing else is logged — no
address, no user agent, no query, no body — and a test asserts the exact key
set, so a seventh field fails the suite.

The rule a field has to pass is that it describes what the worker did, not who
asked. A cache outcome is the worker's own behaviour; a path, an address or a
per-caller counter is not.

Three more lines are operator events rather than requests, each tagged with
`event`:

```json
{ "event": "prewarm", "attempted": 240, "refreshed": 238, "failed": 2, "skipped": 0, "hot_attempted": 42, "hot_refreshed": 42, "hot_cursor": 168, "hot_size": 500, "cold_attempted": 198, "cold_refreshed": 196, "cold_cursor": 1740, "catalog_size": 4012, "entries": 3894, "duration_ms": 7213 }
{ "event": "upstream_down", "upstream": "api.warframe.market", "status": 503, "served": "stale" }
{ "event": "budget_exceeded", "threshold": 100000, "count": 100001 }
```

A `prewarm` line lands on every cron tick, including one that found no catalog
to walk (`catalog_size: 0`) — a prewarm that quietly stops is otherwise
invisible, because every route carries on answering from a snapshot that no
longer advances. Each lane reports what it was asked for, what it refreshed and
the position it started from, so a hot tick's progress is legible next to the
cold walk's; `entries` is the snapshot's size after both. `failed` is items the
upstream would not give and `skipped` is items the tick never reached, which is
a subrequest ceiling too low for the batch rather than a bad afternoon at
warframe.market. Failures are a count: the slugs behind them are not logged, so
there is one rule about slugs in logs rather than two.

`upstream_down` carries the host, which is ours, and never the URL, which for a
per-item endpoint carries the slug. `status` is the upstream's, or `null` when
the fetch never got one — a timeout or a refused connection. `served` is what
the caller got instead: `stale` for the last cached body, `unreachable` for
`502`, `passthrough` when the upstream's own 5xx went to the client.

`budget_exceeded` is logged once, on the request that crosses the threshold,
not on the ones that follow it. It is the moment every install worldwide starts
going to the upstreams direct.

### Reading them

Live, against the deployed worker:

```sh
npx wrangler tail --format json                       # everything
npx wrangler tail --format json | jq -c 'select(.logs[].message[0] | fromjson | .event == "prewarm")'
npx wrangler tail --status error                      # only failed invocations
```

Historically: Workers → `frameforge-cache` → Observability → Logs. The JSON
fields are filterable and groupable there. Two worth having saved:

- **Hit rate by route** — filter `cache` exists, group by `route`, and count by
  `cache`. `hit + revalidating + revalidated + not_modified` over the total is
  what the worker is saving the upstreams; a route sitting near all-`miss` is one whose TTL is
  shorter than the interval clients poll it at.
- **Prewarm failures over time** — filter `event = prewarm`, chart `sum(failed)`
  and `sum(refreshed)` by time. `refreshed` flat at zero means the snapshot has
  stopped advancing while every route still answers, which is the failure that
  otherwise shows up days later as stale prices. `catalog_size: 0` on the same
  line points at the catalog upstream rather than at the per-item fetches.

Sampling is off (`head_sampling_rate: 1` in `wrangler.jsonc`): at this volume
the whole stream is affordable, and a sampled stream turns both of those from
counts into estimates.

## Deploying

`scripts/deploy-worker.sh` from the repository root walks through it: wrangler
login, creating the KV namespace and writing its id into `wrangler.jsonc`, the
budget threshold, `wrangler deploy` (which applies the `DailyBudget`
migration), a health check against the deployed host, and reconciling the URL
the app ships as its default. It is re-runnable: each step checks whether it has
already been done.

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

Stale-if-error, using the order book because it is the class that still blocks
on the upstream, and because its 30-second freshness is short enough to wait
out:

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
