# 5. A Cloudflare Worker caches public data, and only public data

Date: 2026-09-01

## Status

Accepted

## Context

FrameForge already depends on remote endpoints — warframe.market prices,
worldstate, community item data — and shields them with multi-level local
caches. Those caches only help each user individually: public distribution
multiplies direct upstream traffic per install, and upstream rate limits
then surface as our bug reports.

A shared cache in front of the upstreams fixes that, and Cloudflare
Workers makes it close to free at this scale — but only while everything
it serves is shared-cacheable. The moment per-user data flows through it,
both the cost model and the promise that account data never leaves the
user's machine (ADR-2) break.

## Decision

Run a Cloudflare Worker as an aggregation and cache layer between the app
and its public upstreams. It serves public data only: market prices,
worldstate, item and drop data — the same for every user.

No accounts, no telemetry, no per-user state, and nothing derived from a
user's inventory ever reaches the worker. The account-data pipeline —
scanner, logins, trade history, statistics — stays entirely on the user's
machine.

## Consequences

The public/per-user line must be checked whenever a feature wants
server-side help; anything personalized stays client-side by construction,
even when a server-side join would be more convenient.

The worker is an optimization, not a dependency: if it is down, the app
falls back to fetching upstreams directly, exactly as it does today.
