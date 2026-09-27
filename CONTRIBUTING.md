# Contributing to FrameForge

Thanks for wanting to help out.

## Branches

- **`main`** — what people download. Only the maintainer merges into it, from `dev`, when a batch of changes has been tested.
- **`dev`** — the integration branch. Target your pull requests here, not `main`.

## Workflow

1. Fork the repo (or, if you've been given write access, branch off `dev` directly).
2. Make your changes against `dev`.
3. Open a PR into `dev`.
4. The maintainer reviews, merges into `dev`, and does a manual test pass (`pnpm tauri dev`) — there's no automated test suite yet, see below.
5. Periodically, `dev` gets promoted to `main` and a new release is cut.

## Before you open a PR

- Read [CLAUDE.md](CLAUDE.md) and the rule files in [.claude/rules/](.claude/rules/) — they cover the architecture, data-source precedence, memory-scanning constraints, and other conventions specific to this project (e.g. Windows-only, MSVC toolchain, no per-item pattern scanners, fuzzy-match thresholds that shouldn't be raised without benchmarking).
- Use **pnpm**, not npm or yarn.
- There's no automated test suite. Verify your change manually with `pnpm tauri dev` before opening the PR, and describe how you tested it in the PR description.
- Keep changes scoped — this project prefers small, reviewable PRs over large ones.

## Questions

If you're not sure whether an approach fits the project's direction, open an issue or draft PR early rather than building out something large first.
