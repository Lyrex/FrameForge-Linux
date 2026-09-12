# 7. No guided feature tour

Date: 2026-09-01

## Status

Accepted

## Context

Onboarding for public distribution needs two things: getting the app
connected (game detection, warframe.market login, overlay setup) and
letting the user find the features. A first-run setup wizard covers the
first and is planned. A step-by-step feature tour would cover the second
— at the cost of coupling an overlay walkthrough to every screen it
presents, which then breaks silently each time a screen changes.

## Decision

Build the setup wizard; do not build a guided feature tour.

## Consequences

Feature discoverability rests on the UI itself and, where a screen truly
needs explanation, on that screen — which is where the fix belongs anyway:
a view that needs a tour to be understood needs a better view.
