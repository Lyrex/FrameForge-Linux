# FrameForge — Warframe Companion `v3.0.0-linux.1`

A desktop companion for Warframe — live inventory, market prices, trading, timers, relic overlay, and riven analysis. Read-only, no game modification.

> **Windows 10/11 for the full feature set.** Inventory scanning requires Warframe to be running; all other features work standalone.
>
> **Linux** runs against Warframe under Steam Proton: memory scanning goes through `/proc`, and screen capture and OCR through X11 and Tesseract. Only the saved warframe.market session is Windows-only, since there is no OS credential store behind it.

---

## About this fork

This is the Linux fork of [WyrmStudios/FrameForge](https://github.com/WyrmStudios/FrameForge). It exists because the Linux port was declined upstream, not because of a disagreement: upstream is the project, this is its platform branch, and fixes that are not Linux-specific are still sent upstream as individual pull requests. The app, its name and its bundle identifier are unchanged from upstream, so the version string is what separates the two: `2.9.0-linux.1` is built on upstream's 2.9.0, and `-linux.<n>` counts the Linux-only releases cut on that base.

Releases here are Linux only: `.deb`, `.rpm` and `.AppImage`. **Windows users want [upstream's releases](https://github.com/WyrmStudios/FrameForge/releases)**, not this repository.

The overlay stack, the `EE.log` discovery and several cross-platform fixes started from [xamionex/FrameForge-Linux](https://github.com/xamionex/FrameForge-Linux). That material was reworked here rather than taken as-is, so defects in it are ours and belong in this repository's issues, not theirs.

Upstream releases are merged in, tag by tag ([docs/UPSTREAM-SYNC.md](docs/UPSTREAM-SYNC.md)); `main` is not rewritten.

---

## Features

### Live Inventory
Reads your inventory directly from Warframe's process memory (read-only, same API as Overwolf). Instead of scanning for individual item patterns, FrameForge locates and captures the full account JSON blob that the game client holds in memory — the same authoritative data the game itself uses.

This gives complete coverage: resources, mods, arcanes, relics, weapons, Warframes, companions, blueprints, cosmetics (glyphs, palettes, emotes, titles, ship skins), sigils, pending Foundry jobs, credits, and more. Items that leave your inventory (traded, consumed, or expired) are correctly detected as dropped to zero. Inventory is persisted to disk and restored instantly on next launch — no login required.

**View modes** — Cards (icon + text), Icon grid, Text cards, List with icon, or Compact list (text-only, maximum density). Persisted per tab.

### Foundry
Browse every craftable item with full ingredient trees. Components are colour-coded by ownership status and show which relics drop them. Star items to track them in the Modular Window. Filter by Prime, Vaulted/Unvaulted, Owned/Unowned, Ready to build, and Mastered/Unmastered.

**View modes** — switch between Cards (icon + text), Icon grid, Text cards, List (compact row with icon), and Compact list (text-only) via the toggle in the filter bar. Each tab remembers its own preference.

### Market Helper
Browse Prime sets and mods with live platinum prices from [warframe.market](https://warframe.market). Prices are loaded from the [FrameForgePricing](https://github.com/WyrmStudios/FrameForgePricing) mirror on startup — a daily bulk cache updated twice per day from relics.run. No per-item network calls needed. Click any item for a live order popup with sell/buy orders, 3-week price chart, and one-click listing (requires WFM login). Mod cards show per-rank chips — click a chip to pre-fill the listing form with that exact rank and quantity. A **rank selector** in the orders popup filters listings to a specific mod/arcane rank.

**Recipe-aware duplication detection** — multi-count recipes (e.g. Aksomati Prime requires 2× Barrel) are respected when calculating sellable duplicates and the "Has dupes" filter.

**WFM Status Automation** (requires WFM login):
- Go Invisible on startup — set your status to Invisible the moment FrameForge opens
- Go Invisible on close — set your status before the app exits (X button or taskbar close)
- Auto-invisible timer — automatically go Invisible after a configurable number of minutes

### Trading
Full warframe.market integration — manage active listings, post new orders, and receive trade whispers from in-game chat.

- **One-click whisper copy** — every order row has a 📋 button that copies the correct WFM trade message to your clipboard: `I want to buy` from sellers, `I want to sell` to buyers.
- **Auto trade detection** — when a trade completes in-game, the matching WFM whisper is automatically marked complete, the sold reply is copied to your clipboard, and the whisper stays visible as a ghost for 5 minutes.
- **Auto listing update** — after a sale is detected, the corresponding WFM sell listing is automatically decremented (or deleted if the last copy). Works regardless of which tab is active. An **Auto-updated listings** changelog at the bottom of the Listings tab shows every automatic change with an Undo button per entry.
- **Status auto-reconnect** — if WFM drops your status to offline, it is automatically restored without any action needed. Session token stored in Windows Credential Manager.
- **Riven auction management** — view all active riven auctions with AUC/DIR badges, starting price, and buyout price. Edit price, visibility, and auction type (AUC↔DIR) via a dedicated popup. Type switching is handled server-side with a guaranteed full data fetch.
- **In-app WFM login** — authenticate via a built-in WebView browser popup instead of entering credentials manually.

### Relic Helper
Browse void fissure drop tables with rarity colour-coding, ownership status, and platinum values. Supports all refinement levels (Intact → Radiant). View modes apply here too.

**Relic Planner** — EV (expected value) calculator per refinement tier. Pick your metric (Platinum or Ducats), squad size (Solo → 4-player radshare), and filter by era, owned, or vaulted. Sortable columns: click any header (Relic, Owned, Intact, Except., Flawless, Radiant, Refine gain) to sort ascending or descending.

### Timers
Live dashboard from DE's worldstate API:
- World cycles (Cetus, Orb Vallis, Cambion Drift, Zariman) with countdowns
- Bounty reset timers per open world
- Daily/Weekly resets, Sortie, Archon Hunt, The Circuit, Deep Archimedea
- Baro Ki'Teer, Prime Resurgence, Nightwave, Darvo deal, community events
- Alerts, Invasions, Void Fissures with configurable fissure watches

### Statistics
- **Trades** — auto-detected from EE.log. Captures all items from both sides of every trade, including item-for-item barters (no platinum involved). Each trade is classified as Sale, Purchase, or Trade.
- **Trade Log** — individual trade cards showing the full exchange: what you gave, what you received, with which player, and when. Toggle between Log and Analytics views in the Reports tab.
- **Reports** — date-filtered KPIs, platinum charts, per-item breakdown, top trading partners
- **Item Report** — track any item's quantity over time with daily snapshots and drag-to-reorder cards

### Riven Analyzer
Analyses riven rolls against the community-curated [44bananas spreadsheet](https://docs.google.com/spreadsheets/d/1zbaeJBuBn44cbVKzJins_E3hTDpnmvOk8heYN-G8yy8) (413+ weapons). Click **Check Riven** while the riven screen is open for instant per-stat quality ratings. Comparison mode shows old vs new roll side-by-side after each cycle. Supports primary, secondary, melee, and archwing weapons. On Linux, Alt-Tab out of the game before clicking the overlay's buttons — the game keeps the pointer grab until it loses focus.

### OCR Relic Reward Overlay
When a void fissure reward screen opens, FrameForge automatically captures it — via WinRT OCR on Windows, Tesseract on Linux — and shows a transparent overlay with platinum price, ducat value, and set completion for each card. Priority mode: Completion / Plat / Ducats / Set Value.

The item catalog used for OCR matching is built exclusively from known relic reward names — no false matches from non-reward items. Survival fissure multi-round sessions are fully supported: the selected relic carries over between rounds correctly.

### Modular Window
Customisable sidebar with reorderable sections: tracked crafting items, favourite inventory items, pinned timers, and watched fissures.

### Settings
Reorganised into a tabbed sidebar layout: **General** (overlay, scanner, API, account info, pop-out), **Market** (WFM status automation), **Accessibility** (colorblind mode, text size up to 200%), **Data** (item database, cache), and **Debugging** (loggers, diagnostic tools with folder access and one-click clear).

---

## EULA Transparency

One feature touches a EULA grey area and is **off by default** with an explicit opt-in warning:

- **Memory Scanner** — `ReadProcessMemory` for live inventory. Read-only, same API as Overwolf.

The **Warframe Companion API** (`api.warframe.com/api/inventory.php`) has been **temporarily suspended**. DE confirmed third-party tools run at your own risk but could not clarify whether this specific undocumented endpoint is permitted. The feature is disabled until clearer guidance is received.

Everything else (Foundry, Market, Relics, Timers, Statistics) runs on public data only.

---

## Is This Safe?

| | |
|---|---|
| Memory access | Read-only `ReadProcessMemory` — never writes, never injects |
| Game modification | None |
| Network | warframe.market, DE worldstate, WFCD GitHub repos, FrameForgePricing mirror. No FrameForge server, no telemetry |
| Credentials | WFM token in Windows Credential Manager (on Linux the session is not saved). Warframe API credentials never written to disk |

Source code is fully public under GPLv3 — build and verify it yourself.

---

## Requirements

- Windows 10 or 11 (64-bit) from upstream, or Linux with glibc 2.39 or newer running Warframe through Steam Proton
- Warframe installed for inventory scanning (other features work without it)
- [warframe.market](https://warframe.market) account for trading features (optional)

---

## Installation

**Linux** — download the `.deb`, `.rpm` or `.AppImage` from [**Releases**](../../releases). Ubuntu 24.04, Debian 13 and Fedora 40 are all above the glibc floor; Debian 12 is below it and will install cleanly but fail to launch.

**Windows** — download the installer from [upstream's Releases](https://github.com/WyrmStudios/FrameForge/releases), run it (click **More info → Run anyway** if SmartScreen warns you — there is no code-signing certificate), and launch FrameForge from Start or the desktop shortcut.

---

## Building From Source

### Windows

Build Windows from upstream rather than from this fork — see [About this
fork](#about-this-fork) for why.

```powershell
# Prerequisites: Node.js 20+, pnpm, Rust MSVC toolchain
rustup default stable-x86_64-pc-windows-msvc

git clone https://github.com/WyrmStudios/FrameForge.git
cd FrameForge
pnpm install
pnpm tauri dev      # dev mode with hot reload
pnpm tauri build    # installer → src-tauri/target/release/bundle/
```

### Linux

Releases carry the three packages, so building is for development or for a
branch no release covers.

```sh
# Prerequisites: Node.js 20+, pnpm, a Rust toolchain, and the system
# development headers listed in .github/actions/linux-build-deps/action.yml
git clone https://github.com/Lyrex/FrameForge-Linux.git
cd FrameForge-Linux
pnpm install
```

The frontend and backend are started separately:

```sh
pnpm dev

# In another terminal:
cd src-tauri
cargo run
```

Packages build with the normal command:

```sh
NO_STRIP=true pnpm tauri build   # deb, rpm and AppImage in src-tauri/target/release/bundle/
```

`NO_STRIP=true` is only needed for the AppImage: linuxdeploy ships an old
`strip` that cannot read the `.relr.dyn` sections modern distributions use, and
it aborts the bundle rather than skipping the library. Building only `deb` and
`rpm` (`--bundles deb,rpm`) does not need it.

Warframe must be running through Steam Proton — `EE.log` is read from the
Proton prefix, and memory scanning reads `/proc/<pid>/mem`, which requires
`kernel.yama.ptrace_scope` to permit same-user process access. Persistent
warframe.market sessions are unavailable, and the "remember me" control is
hidden rather than offered.

---

## Tech Stack

| Layer | Technology |
|---|---|
| Frontend | React 19, TypeScript 5.8, Vite 7 |
| Desktop shell | Tauri 2 |
| Backend | Rust 2021 edition |
| Database | SQLite (local only) |
| Windows APIs | ReadProcessMemory, WinRT OCR, DXGI, GDI, Windows Credential Manager |

---

## Data & Privacy

- No account required for most features
- No telemetry — no FrameForge server
- All data stored locally at `%LOCALAPPDATA%\warframe-companion\` (Windows) or `~/.local/share/warframe-companion/` (Linux)
- WFM session token stored in Windows Credential Manager if "Stay logged in" is enabled

---

## License

GPLv3 — see [LICENSE](LICENSE).

---

## Contributing

Bug reports, feature requests, and PRs welcome via [GitHub Issues](../../issues). Use the issue templates. For large changes, open an issue first to align on approach.

---

*FrameForge is not affiliated with Digital Extremes Ltd. Warframe is a trademark of Digital Extremes Ltd.*
