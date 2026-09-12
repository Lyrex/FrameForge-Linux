# 13. Mastery rules are FrameForge's, the catalogue is data

Date: 2026-09-12

## Status

Accepted

## Decision

How affinity becomes an equipment rank, where the rank cap sits, and what
awards mastery credit are rules encoded in FrameForge, in one place. The
WFCD catalogue supplies which equipment exists and per-item data such as
`maxLevelCap`; the rules consult that data and fill or override it. A gap
or error in the catalogue is fixed in the rules, not accepted.

## Rationale

The catalogue ships no cap for Necramechs, marks Amp Prisms and Infested
Kitgun chambers non-masterable, and omits the Plexus, while the game's
profile credits all of them. Trusting the catalogue field wherever it
happened to be read produced four sites deriving rank and cap differently,
and a rank-40 weapon showing mastered in one view and partial in another.
The alternative, patching the catalogue snapshot on fetch, hides the rule
in a parser and keeps it out of tests.

## Consequences

New equipment the catalogue lists correctly needs no code change. New
equipment whose cap or masterability the catalogue gets wrong shows wrong
until a rule is added; the wiki's Mastery Rank page is the reference to
check against.
