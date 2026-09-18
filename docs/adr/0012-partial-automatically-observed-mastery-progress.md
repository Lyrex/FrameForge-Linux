# 12. Mastery progress is partially and automatically observed

Date: 2026-09-12

## Status

Accepted

## Decision

Mastery progress comes from verified local game observations only. A source
kind no verified observation covers is Unknown, and Unknown is never zero; an
entry absent from a confirmed field is zero. Coverage gaps do not block
release: a calculation that needs a missing kind reports Unknown, or the
widest honest bound where one exists. Manual progress entry is excluded.

Progress is one account's: whatever the last observation wrote, under
whichever account was logged in. A switch is not tracked. The next
observation overwrites, and the mastery plan stays until the player
re-plans.

## Rationale

Treating absent data as zero would recommend completed activities and
overstate remaining gains. Gating release on complete coverage would withhold
recommendations the verified sources already support. Manual entry would fill
gaps at the cost of a second authority and reconciliation rules for when the
observation later arrives.

Keying progress per player name left the inventory cache unkeyed, and the
rules reconciling the two stores' owners produced every case of Suggestions
going blank without explanation. One account per install is the intent; a
second account gets its state from its first scan.

## Consequences

Users see Unknown widely until each source kind's extraction is verified.
Extends [ADR-11](0011-no-official-api-access.md) without reopening account API
access; memory, EE.log, and screen observations remain complementary under
[ADR-8](0008-ocr-is-a-first-class-observation-source.md).

After a switch, the previous account's progress, plan and inventory show
under the new name until the new account's first observation. An observation
that covers only some source kinds leaves the others as the previous account
observed them.
