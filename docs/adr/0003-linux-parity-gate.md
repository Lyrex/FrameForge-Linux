# 3. Nothing ships that works on Windows only

Date: 2026-09-01

## Status

Superseded by ADR-10.

## Context

Linux support is one of FrameForge's three identity claims (ADR-2), and it
is the one competitors have failed to hold: the closest comparable tool
ships a Linux build that is borderline unusable. That failure mode does
not arrive as a decision; it accumulates one Windows-only feature at a
time, each individually reasonable, until the Linux build is a second-class
port.

FrameForge already carries the hard parts on both platforms: the memory
scanner, the overlays, and EE.log discovery each have working Linux paths.

## Decision

A feature ships when it works on Linux and Windows, or it does not ship.
Where a mechanism cannot be made cross-platform, the feature is redesigned
around one that can, or it waits.

## Consequences

Windows-only quick wins are rejected even when the Windows implementation
is a fraction of the effort. Platform-specific code is allowed — the
scanner and credential store already split by platform — but the
capability it delivers must exist on both sides before release.

Estimates for anything touching the game process, windowing, or the
filesystem must include the second platform from the start, not as a
follow-up.
