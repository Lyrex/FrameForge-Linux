# 2. A broad companion whose identity is how it reads the game

Date: 2026-09-01

## Status

Amended by ADR-10: the platform claim is Linux only.

## Context

FrameForge overlaps heavily with WFHelper, an established companion with a
broader feature surface: progression trackers, arbitration scheduling and
analytics, drop-table search, and more app polish. The open question was
whether FrameForge should narrow itself to what it uniquely does — live
memory scanning and trading — or compete on the full surface.

Narrowing was rejected. The features other companions carry are wanted by
the same players FrameForge serves, and several (arbitration tooling in
particular) have demonstrated community demand. What actually separates
FrameForge is not which features it has but the layer underneath them:
it reads account state live from the running game where others scrape
credentials or screenshots, it is native on Linux where the competition
is broken, and its Rust/Tauri stack keeps it fast.

## Decision

FrameForge is the full-featured Warframe companion that reads your account
live from the running game — native and first-class on Linux and Windows,
fast, and your account data never leaves your machine.

Breadth is the ambition; those three claims — live truth from memory,
first-class on both platforms, account data local — are the identity.
A missing feature is backlog. A feature that would compromise one of the
three claims is not built that way, however useful.

## Consequences

The feature list will permanently trail the field somewhere; that is
acceptable and expected, and a gap in it is never an emergency.

Every roadmap argument reduces to one test: does this serve players
without weakening live-read, cross-platform, or account-data-locality?
The gates themselves are recorded separately: platform parity in ADR-3,
read-only access in ADR-4, the boundary between local and remote data in
ADR-5.
