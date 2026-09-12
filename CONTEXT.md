# FrameForge

A desktop companion for Warframe: it observes the game (process memory, `EE.log`,
the reward screen) and joins what it sees against warframe.market and the
community item data. It never writes to the game.

## Language

### Items

**Item**:
Anything the game can put in your account: a resource, mod, arcane, relic,
weapon, Warframe, blueprint, or cosmetic. Identified by its `unique_name`.

**unique_name**:
The game's own identifier for an item, a `/Lotus/...` path. The canonical
identity of an item everywhere in FrameForge — any other name is a label or a
foreign key, never an identity.
_Avoid_: item id, path, internal name

**Display name**:
The human-readable name the game shows for an item. Localised, and in `EE.log`
it can carry rank dots and rank suffixes, so it is not stable enough to identify
an item by.
_Avoid_: item_name, name (when identity is meant)

**Slug**:
warframe.market's identifier for an item (`url_name`). Foreign to the game, so a
slug only exists for items warframe.market trades, and maps to a `unique_name`
rather than replacing it.
_Avoid_: url_name, item_url, market id

### Account state and mastery

**Account state**:
The player's possessions and progression, including equipment history,
Intrinsic ranks, and mission completion. Inventory is its owned-item subset.
_Avoid_: inventory (when progression is included)

**Mastery source**:
One thing that awards mastery credit to an account: an equipment type, an
Intrinsic track, a junction, or a chart node in one mode. A node's normal and
Steel Path completions are two sources, since each is unlocked and blocked
separately.
_Avoid_: Intrinsic rank (a rank is progress within a track, not a source)

**Affinity**:
The experience the game records per equipment type, cumulative across every
copy, Forma reset, and sale, and never decreasing. Ranks and mastery credit are
derived from it; a Warframe-like piece needs twice the affinity per rank that a
weapon does.
_Avoid_: XP, level (when the permanent record is meant)

**Equipment rank**:
How far a piece of equipment has progressed, 0 up to its rank cap, as derived
from affinity. Distinct from the account's Mastery Rank and from an owned
copy's current level.
_Avoid_: item rank, level (when derived progress is meant)

**Rank cap**:
The highest equipment rank that still awards mastery credit: 30 for most
equipment, 40 for Kuva, Tenet and Coda weapons, the Paracesis, and Necramechs.
The catalogue does not know every cap, so FrameForge's own rules decide.
_Avoid_: max level, level cap (when the mastery ceiling is meant)

**Level cap**:
The ceiling an owned copy can currently reach: 30, raised by 2 per Forma on
rank-40 equipment, up to the rank cap. A property of the copy, not of the
mastery source.
_Avoid_: rank cap

**Mastery credit**:
Mastery points awarded per equipment rank: 200 for Warframe-like equipment
(Warframes, Archwings, Necramechs, companions, K-Drives, the Plexus), 100 for
weapons, including sentinel, MOA and Hound weapons.

**Earned mastery**:
Mastery credit permanently awarded to an account, independent of current
equipment ownership or the level of an owned copy.
_Avoid_: affinity, equipment level (when permanent account credit is meant)

**Remaining mastery**:
The mastery credit an account can still earn from a mastery source.
_Avoid_: missing items (when unearned credit is meant)

**Mastered**:
A mastery source for which the account has earned all eligible mastery credit.
For equipment, this is independent of current ownership or an owned copy's level.
_Avoid_: owned, max-level copy (when permanent mastery completion is meant)

**Node key**:
The game's own identifier for a star chart node or junction, as carried in
account state. The canonical identity of a node everywhere in FrameForge; the
planet and node name are labels.
_Avoid_: node name, planet/node string (when identity is meant)

**Confirmed**:
Progress for a mastery source kind that a game observation with a known time
established. An entry absent from a confirmed field is zero credit, not
unknown.
_Avoid_: observed, fresh, cached

**Unconfirmed**:
Progress carried over from a cache with no observation time. Shown, but not
trusted until the game is observed again.
_Avoid_: stale, legacy

**Unknown**:
Progress for a mastery source kind that no verified observation covers.
Never displayed as zero.

**Acquisition route**:
A way to obtain the equipment, components, or progression needed to earn
remaining mastery, such as crafting, relic rewards, purchases, or unlocks.

**Opportunity**:
A mastery source with remaining mastery and at least one acquisition route.

**Mastery plan**:
An ordered selection of actions toward a target Mastery Rank, with potential
gains conditional on completing those actions.

### The game's log

**Log path override**:
The location of `EE.log` as chosen by the player, taking precedence over
FrameForge's own detection. The game writes the log only once it has run, so an
override naming a file that does not exist yet is a normal state, not an error.
_Avoid_: custom log path, manual path (when the setting is meant)

### Trading

**Trade**:
One completed exchange between two players. A trade carries one or more items
and/or platinum on each side, each with its own quantity; a trade with items on
both sides and no platinum is still a trade.

### warframe.market

**WFM session**:
The warframe.market login state, carried as a token bundle rather than as the
credentials that produced it. The email and password are typed once at login and
never stored; the session is what outlives the app being closed.
_Avoid_: credentials, password (when the token is meant)

### Arbitrations

**Arbitration tier**:
The community's rating, S down to D, of how well a star chart node farms
Vitus Essence during its arbitration hour, as published by the Arbitration
Goons. A node the rating does not cover is Unrated.
_Avoid_: rank, grade, score

**Tier filter**:
Which arbitration tiers the schedule browser lists. The hour running right now
is always shown regardless of the filter.

**Alert tiers**:
The arbitration tiers whose every scheduled hour raises an alert. Independent
of both the tier filter and favorited nodes: an hour alerts if its node is
favorited or its tier is an alert tier.
_Avoid_: notify tiers, tier favorites

### Updates

**Release notes**:
The text published with a FrameForge release, shown when a newer version is
offered. Distinct from the Change log, which lists inventory changes the app
observed in the game.
_Avoid_: changelog, notes (when the release text is meant)
