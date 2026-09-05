// Run with: node --experimental-strip-types --test src/clockFormat.test.ts

import assert from "node:assert/strict";
import { test } from "node:test";
import { clockOptions, fmtClock } from "./clockFormat.ts";

const NOON_UTC = 1_635_753_600;

test("12h forces an AM/PM hour", () => {
  assert.equal(clockOptions("12h").hour12, true);
  assert.match(fmtClock(NOON_UTC, "12h", "en-US"), /(AM|PM)/);
});

test("24h never shows AM/PM", () => {
  assert.equal(clockOptions("24h").hour12, false);
  assert.doesNotMatch(fmtClock(NOON_UTC, "24h", "en-US"), /(AM|PM)/);
});

test("auto leaves the locale's own default", () => {
  assert.equal(clockOptions("auto").hour12, undefined);
  assert.equal(fmtClock(NOON_UTC, "auto", "en-US"), fmtClock(NOON_UTC, "12h", "en-US"));
});