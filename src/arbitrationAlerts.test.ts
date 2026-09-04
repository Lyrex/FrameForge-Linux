// Run with: node --experimental-strip-types --test src/arbitrationAlerts.test.ts

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  clampLead, dueAlerts, DEFAULT_LEAD_MINS, MAX_LEAD_MINS, MIN_LEAD_MINS,
  type ScheduleEntry,
} from "./arbitrationAlerts.ts";

const HOUR = 3600;

const entry = (start: number, node_id = "SolNode1"): ScheduleEntry => ({
  start, end: start + HOUR, node_id,
  node: "Tessera", region: "Void", mission_type: "Defense", faction: "Corpus",
});

const due = (r: { due: ScheduleEntry[] }) => r.due.map(e => e.node_id);

test("a favorited hour alerts once the lead window opens", () => {
  const entries = [entry(10 * HOUR)];
  assert.deepEqual(due(dueAlerts(entries, ["SolNode1"], 10, [], 10 * HOUR - 601)), []);
  assert.deepEqual(due(dueAlerts(entries, ["SolNode1"], 10, [], 10 * HOUR - 600)), ["SolNode1"]);
  assert.deepEqual(due(dueAlerts(entries, ["SolNode1"], 10, [], 10 * HOUR - 1)), ["SolNode1"]);
});

test("an hour already under way never alerts", () => {
  const entries = [entry(10 * HOUR)];
  assert.deepEqual(due(dueAlerts(entries, ["SolNode1"], 10, [], 10 * HOUR)), []);
  assert.deepEqual(due(dueAlerts(entries, ["SolNode1"], 10, [], 10 * HOUR + 60)), []);
});

test("nothing alerts without a favorite", () => {
  const entries = [entry(10 * HOUR), entry(11 * HOUR, "SolNode2")];
  const result = dueAlerts(entries, [], 60, [], 10 * HOUR - 60);
  assert.deepEqual(result.due, []);
  assert.deepEqual(result.fired, []);
});

test("an unfavorited node in the same window stays quiet", () => {
  const entries = [entry(10 * HOUR, "SolNode1"), entry(10 * HOUR, "SolNode2")];
  assert.deepEqual(due(dueAlerts(entries, ["SolNode2"], 10, [], 10 * HOUR - 60)), ["SolNode2"]);
});

test("an occurrence already fired is not repeated", () => {
  const entries = [entry(10 * HOUR)];
  const first = dueAlerts(entries, ["SolNode1"], 10, [], 10 * HOUR - 600);
  assert.deepEqual(due(first), ["SolNode1"]);

  // Every later tick still sits inside the window and must stay quiet.
  let fired = first.fired;
  for (let tick = 1; tick <= 5; tick++) {
    const next = dueAlerts(entries, ["SolNode1"], 10, fired, 10 * HOUR - 600 + tick * 60);
    assert.deepEqual(next.due, []);
    fired = next.fired;
  }
});

test("fired state survives a restart", () => {
  const entries = [entry(10 * HOUR)];
  const { fired } = dueAlerts(entries, ["SolNode1"], 10, [], 10 * HOUR - 600);
  // Same state read back from settings after a relaunch inside the window.
  assert.deepEqual(dueAlerts(entries, ["SolNode1"], 10, [...fired], 10 * HOUR - 300).due, []);
});

test("the same node alerts again on its next occurrence", () => {
  const entries = [entry(10 * HOUR), entry(40 * HOUR)];
  const { fired } = dueAlerts(entries, ["SolNode1"], 10, [], 10 * HOUR - 60);
  assert.deepEqual(due(dueAlerts(entries, ["SolNode1"], 10, fired, 40 * HOUR - 60)), ["SolNode1"]);
});

test("keys for hours that have started are pruned", () => {
  const entries = [entry(40 * HOUR)];
  const stale = [`${10 * HOUR}@SolNode1`, `${40 * HOUR}@SolNode1`, "garbage"];
  const { fired } = dueAlerts(entries, ["SolNode1"], 10, stale, 20 * HOUR);
  assert.deepEqual(fired, [`${40 * HOUR}@SolNode1`]);
});

test("favoriting mid-window alerts on the next tick", () => {
  const entries = [entry(10 * HOUR)];
  const quiet = dueAlerts(entries, [], 10, [], 10 * HOUR - 300);
  assert.deepEqual(due(dueAlerts(entries, ["SolNode1"], 10, quiet.fired, 10 * HOUR - 240)), ["SolNode1"]);
});

test("a longer lead reaches an hour a shorter one does not", () => {
  const entries = [entry(10 * HOUR)];
  const now = 10 * HOUR - 30 * 60;
  assert.deepEqual(due(dueAlerts(entries, ["SolNode1"], 10, [], now)), []);
  assert.deepEqual(due(dueAlerts(entries, ["SolNode1"], 45, [], now)), ["SolNode1"]);
});

test("lead is clamped into range, so a zero or negative lead cannot fire late", () => {
  assert.equal(clampLead(0), MIN_LEAD_MINS);
  assert.equal(clampLead(-30), MIN_LEAD_MINS);
  assert.equal(clampLead(9999), MAX_LEAD_MINS);
  assert.equal(clampLead(NaN), DEFAULT_LEAD_MINS);
  assert.equal(clampLead(10.4), 10);

  // A settings file claiming no lead still alerts before the hour, not on it.
  const entries = [entry(10 * HOUR)];
  assert.deepEqual(due(dueAlerts(entries, ["SolNode1"], 0, [], 10 * HOUR - 60)), ["SolNode1"]);
  assert.deepEqual(due(dueAlerts(entries, ["SolNode1"], 0, [], 10 * HOUR)), []);
});

test("only occurrences inside the window join the fired state", () => {
  const entries = [entry(10 * HOUR), entry(20 * HOUR)];
  const { fired } = dueAlerts(entries, ["SolNode1"], 10, [], 10 * HOUR - 60);
  assert.deepEqual(fired, [`${10 * HOUR}@SolNode1`]);
});
