# 8. OCR is a first-class observation source, not debt

Date: 2026-09-01

## Status

Accepted

## Context

FrameForge reads the game through three channels: process memory, EE.log,
and OCR of the screen. Memory is the richest and EE.log the cheapest, which
invites a purity principle — "memory and log first, OCR as a last resort" —
and with it the standing temptation to rewrite OCR-based features onto the
other channels.

That principle was considered and rejected. Riven detection and reward
detection both rely on OCR today because the information they need is not
reliably reachable any other way; the riven watcher already pairs a memory
trigger with an OCR read. The channels are complements with different
failure modes, not a quality ranking.

## Decision

Each feature uses whichever observation source reads its data most
reliably, and OCR is a legitimate answer. There is no mandate to migrate
OCR-based features to memory or EE.log.

## Consequences

The OCR pipeline is load-bearing and gets maintained accordingly —
resolution and UI-scale handling, game UI changes, and Linux capture
(ADR-3) are ongoing costs, not cleanup items.

If a memory or log path to the same data later becomes reliable, switching
is a per-feature judgment about reliability, never a cleanup crusade.
