# 4. The game is observed, never written to

Date: 2026-09-01

## Status

Accepted

## Context

FrameForge's most valuable data path is reading the running game's process
memory. Digital Extremes officially disallows third-party tools, but their
practical tolerance draws the line at interference: tools that only read
have coexisted with the game for years, while anything that writes to the
process is cheating-tool territory and risks the accounts of everyone
running it.

The same line separates a companion from an automation tool. Features like
auto-accepting trades or acting in-game on the player's behalf would each
require crossing it.

## Decision

FrameForge observes the game — process memory, EE.log, the screen — and
never writes to any of it. The scanner opens the process for reading only,
and no feature is built that requires injecting input, modifying memory,
or touching game files.

## Consequences

Feature requests that amount to automation are declined outright rather
than weighed case by case; the answer is structural.

The tolerance FrameForge relies on is informal and can change. Staying
strictly read-only is the strongest position available under it, and the
one part of the risk posture entirely under our control. The residual
risk that remains — use of the official API, which DE can detect
server-side — is handled separately in ADR-9.
