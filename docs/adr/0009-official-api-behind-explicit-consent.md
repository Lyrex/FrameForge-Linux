# 9. Official API sync stays, behind explicit at-own-risk consent

Date: 2026-09-01

## Status

Superseded by ADR-11.

## Context

Fetching the account inventory through the official companion API is the
one data path Digital Extremes can detect server-side. Their official
stance disallows third-party tools altogether; their practical tolerance
of read-only tools (ADR-4) is informal and has not been clarified for API
use. That uncertainty is why the sync's setting is currently suspended.

Dropping the path entirely would be the cleanest risk story, but it is
not just a feature: console and cross-platform players have no local game
process to scan, so credential-based API access is their only way into
FrameForge at all. Cutting it cuts the segment.

## Decision

Keep the API sync and the console login, disabled by default. Restore the
setting behind an explicit opt-in that states the risk in plain terms:
DE can detect API access, has not sanctioned it, and the user enables it
at their own risk.

## Consequences

Console players are supported, and the risk is carried knowingly by each
user who opts in rather than silently by everyone.

The exposure stays one toggle wide: if DE objects, disabling the path
removes the entire detectable surface without touching any other feature.
If DE clarifies that API use is acceptable, the consent gate can be
relaxed to a plain setting.
