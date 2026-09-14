// Run with: node --experimental-strip-types --test src/lib/formatters.test.ts

import assert from "node:assert/strict";
import { test } from "node:test";
import { formatAge } from "./formatters.ts";

test("age picks the largest whole unit and never a fraction", () => {
  const now = 1_700_000_000;
  assert.equal(formatAge(now, now), "0s ago");
  assert.equal(formatAge(now - 59, now), "59s ago");
  assert.equal(formatAge(now - 60, now), "1m ago");
  assert.equal(formatAge(now - 3_599, now), "59m ago");
  assert.equal(formatAge(now - 3_600, now), "1h ago");
  assert.equal(formatAge(now - 86_399, now), "23h ago");
  assert.equal(formatAge(now - 86_400, now), "1d ago");
  assert.equal(formatAge(now - 8 * 86_400, now), "8d ago");
});

test("no timestamp reads as never", () => {
  assert.equal(formatAge(null, 1_700_000_000), "never");
});
