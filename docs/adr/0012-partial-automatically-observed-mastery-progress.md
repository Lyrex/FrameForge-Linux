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

## Rationale

Treating absent data as zero would recommend completed activities and
overstate remaining gains. Gating release on complete coverage would withhold
recommendations the verified sources already support. Manual entry would fill
gaps at the cost of a second authority and reconciliation rules for when the
observation later arrives.

## Consequences

Users see Unknown widely until each source kind's extraction is verified.
Extends [ADR-11](0011-no-official-api-access.md) without reopening account API
access; memory, EE.log, and screen observations remain complementary under
[ADR-8](0008-ocr-is-a-first-class-observation-source.md).
