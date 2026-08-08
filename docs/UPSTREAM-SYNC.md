# Syncing with upstream

This fork tracks [WyrmStudios/FrameForge](https://github.com/WyrmStudios/FrameForge)
and pulls its releases in for as long as the fork lives. This document is the
procedure for one sync, and the ledger of our changes that upstream has since
absorbed.

## Rules

- **Merge, never rebase.** `main` is published history; it is never rewritten.
  Upstream arrives as a merge commit.
- **Merge upstream release tags only, never `upstream/main`.** Upstream tags
  whatever their tip happens to be, so the tip routinely runs ahead of the
  newest tag. Our version string (`<upstream-version>-linux.<n>`) claims an
  exact upstream base; merging an untagged tip would make it a lie.
- **Sync when we intend to cut a fork release**, not on every upstream
  release. The fork's value can only be verified in a manual session against
  the live game, and that session happens at release time; a synced-but-
  unverified `main` is a branch nobody has run. Sync early only when upstream
  ships something we specifically want, or when one of our fixes lands
  upstream.
- **Version scheme:** `<upstream-version>-linux.<n>`. `n` counts fork releases
  on the same upstream base and resets to 1 on each sync. A semver prerelease
  rather than build metadata, which the fork used up to `2.9.0+linux.2`: semver
  ignores build metadata in precedence, so `+linux.2` and `+linux.3` compare
  equal and no updater could tell them apart, and the `+` arrives URL-encoded
  in release asset links. A prerelease does sort below the bare upstream
  version, which costs nothing here because this repo never publishes a
  bare-version artifact to compare against.

## Procedure

```sh
# one-time, per clone
git config rerere.enabled true

# 0. preconditions
git status --porcelain              # must be empty

# 1. fetch upstream, pick the newest RELEASE TAG (never main)
git fetch upstream --tags
git tag --sort=-v:refname --merged upstream/main | head -1
# -> TAG, e.g. v2.8.0. --merged excludes our own v*-linux.* tags,
#    which are not ancestors of upstream/main.

# 2. read the delta before touching anything
git log --no-merges --reverse \
  --format='%h %s%n    %(trailers:key=Fork-delta,valueonly)' \
  upstream/main..main
# Cross-check against the ledger below: commits listed there have been
# absorbed upstream, and their conflicts resolve toward upstream's side.

# 3. merge the tag
git merge TAG
```

Resolve conflicts by this playbook:

- **Version files** (`package.json`, `src-tauri/Cargo.toml`,
  `src-tauri/tauri.conf.json`): guaranteed to conflict — upstream bumps the
  same three lines every release. Never take either side. Recompute:
  `<TAG version>-linux.1`.
- **`Cargo.lock`:** regenerate, never hand-merge. `git checkout TAG --
  src-tauri/Cargo.lock`, then `cargo check` (which re-adds our dependencies
  and stamps the fork version), then stage it. Note upstream has shipped this
  file stale before (v2.8.0's lock still said 2.7.0), so do not trust their
  side either.
- **Our own code coming back at us:** a conflict where upstream's side
  contains a fix we wrote means it was absorbed into their release squash.
  Take upstream's side, then record the commit in the ledger below — in the
  same commit as the merge.
- **Everything else:** preserve both intents; upstream's platform-neutral
  code plus our `cfg`-gated Linux code is the normal resolution shape.

```sh
# 4. after the merge commit: checks that must pass before anything publishes
cargo test --locked --manifest-path src-tauri/Cargo.toml
pnpm build
shellcheck scripts/*.sh
bash scripts/check-version.sh
```

Two known silent hazards to check by hand — neither reliably *conflicts*:

- The Linux test module in `src-tauri/src/memory_scanner.rs`. Upstream has no
  Linux code, so a merge can drop or duplicate tests there without a
  conflict. Diff the module against pre-merge `main`.
- Upstream additions that call Windows APIs ungated (v2.8.0's
  `get_system_locale` called `windows_sys` from shared code). The Linux build
  catches these; the fix is a `cfg`-gated variant, kept as its own commit
  with a `Fork-delta: port` trailer.

```sh
# 5. verify live, then release
# Manual session against Warframe under Proton: inventory scan populates,
# relic overlay over a fullscreen game (click-through, disappears on
# dismiss), riven overlay buttons clickable. Nothing below runs until this
# passes.
git push origin main
git tag v<TAG-version>-linux.1
git push origin v<TAG-version>-linux.1     # triggers the release workflow
```

## Delta classification

Every fork commit carries a `Fork-delta:` trailer, written when the commit is
made:

- `port` — Linux implementation of something Windows already does. Never goes
  upstream.
- `upstreamable` — platform-agnostic fix that should become an upstream PR.
  This is the extraction queue.
- `fork-only` — packaging, CI, README, identity.

The current delta is the inventory command in step 2, minus the ledger below.
Under merge, history never shrinks: absorbed commits stay in `git log`
forever, so the ledger is the only record that they no longer count.

## Ledger: absorbed by upstream

Append-only. One line per fork change upstream has absorbed, added in the
same commit as the sync merge that confirmed it. Upstream applies PRs by hand
inside release squashes and closes them unmerged, so PR state is not evidence
— confirm by diffing the PR branch's additions against the release tree.

| Upstream release | Fork change | Upstream PR |
|---|---|---|
| v2.8.0 | UTF-8-safe truncation of log previews (`truncate_chars`) | #22 |
| v2.8.0 | OCR reward matching: exact 4-char words, evenly spaced rarity bars | #23 |
| v2.8.0 | Blob scan seeded at the enclosing brace | #25 |
| v2.8.0 | Market list shows known price instead of loading spinner | #26 |
| v2.8.0 | Riven OCR reads the whole card without its border | #27 |
