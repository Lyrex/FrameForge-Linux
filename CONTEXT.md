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
