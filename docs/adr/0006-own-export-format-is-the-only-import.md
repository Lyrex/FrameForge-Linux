# 6. FrameForge's own export format is the only import

Date: 2026-09-01

## Status

Accepted

## Context

Other companions import data from each other's files — cached inventories,
helper JSON, historical statistics. For FrameForge the inventory case is
already solved by a better source: the scanner reads current account state
from the game itself, so a stale snapshot written by another tool is a
worse data path than any we have, and parsing it would chain us to that
tool's private format.

Historical statistics are the one thing the game cannot replay: a user
migrating from another tool loses their old trade history. That loss is
real but has no demonstrated demand yet, and each foreign format supported
is a permanent parsing liability for a one-time migration.

## Decision

FrameForge exports its statistics and trade history as JSON, and imports
exactly that format. Other tools' data enters through the game, not
through their files.

## Consequences

Users own their data: history can be backed up, moved between machines,
and inspected as plain JSON. The export format becomes an interface —
changing it needs versioning or a migration path from the first release
on.

Migrating users get their current account state by running the scanner
once, and start their history fresh. TODO: revisit a one-off statistics
converter if migrating users actually ask for one; it would target the
import format as a standalone tool rather than adding foreign parsers to
the app.
