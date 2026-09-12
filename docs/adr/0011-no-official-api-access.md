# 11. The official companion API is not used

Date: 2026-09-03

## Status

Accepted. Supersedes ADR-9.

## Context

ADR-9 kept the official-API inventory sync and the console login behind
an explicit at-own-risk consent dialog, to keep console and
cross-platform players reachable. That gate was built but never merged.
Meanwhile the sync stayed suspended and nothing in the app reached the
login or sync commands.

The path is the one Digital Extremes can detect server-side, and the app
offering a switch for it, however many warnings sit in front, makes the
app the thing that put the user's account at risk. A consent dialog moves
the blame, not the exposure.

## Decision

The app offers no way to reach the official API: no setting, no consent
dialog, no login form. The account login, console login and API
inventory sync code is removed rather than left unreachable. Account
state comes from the running game only (memory, EE.log, screen).

## Consequences

Console and cross-platform players are out of scope: without a local
game process there is nothing to read. That is the cost of the decision,
taken knowingly.

The app has no detectable server-side footprint against DE. If DE ever
sanctions API use, this is reopened as a new decision, not by restoring
the deleted code.
