# 10. FrameForge is Linux-only

Date: 2026-09-03

## Status

Accepted. Supersedes ADR-3; amends the identity claim in ADR-2.

## Context

ADR-2 named "first-class on Linux and Windows" as one of three identity
claims and ADR-3 gated every feature on working on both. Upstream
FrameForge is the Windows product; this fork exists because upstream's
Linux support did not. Carrying the Windows capture, OCR and memory
paths here meant maintaining a second platform nobody ships from this
tree: the Windows build was dropped from the release workflow and the
Windows modules, cfg gates and paired stubs were deleted from the crate
the same week.

## Decision

This tree builds and ships for Linux only. The identity claim in ADR-2
reads "native and first-class on Linux"; the other two claims stand.
Features need no second-platform path, estimate, or verification.
Windows remains upstream's job.

## Consequences

Platform gates and Windows-specific code do not come back with feature
work. Upstream syncs that bring Windows code are stripped at the merge,
not kept behind cfg gates. Acceptance criteria that read "works on both
platforms" are void.
