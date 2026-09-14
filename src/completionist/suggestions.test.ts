// Run with: node --experimental-strip-types --test src/completionist/suggestions.test.ts

import assert from "node:assert/strict";
import { test } from "node:test";
import { actionText, DEFAULT_CONTROLS, detailText, parseControls, readyText, remainingText, visibleOpportunities } from "./suggestions.ts";
import type { Opportunity } from "../types/mastery.ts";

const opportunity = (name: string, over: Partial<Opportunity> = {}): Opportunity => ({
  unique_name: `/Lotus/Weapons/Tenno/${name}`, name, category: "Primary", image_name: null, mastery_req: null,
  cap: 30, earned_rank: 12, state: "partial", unobtainable: null, excluded: false,
  stage: "level_claim", action: "level", remaining_mastery: 1800, owned: true, owned_level: 12, build_completion_ms: null,
  vendors: [], access: "available", blockers: [], ...over,
});

test("stored controls are validated field by field and fall back to defaults", () => {
  assert.deepEqual(parseControls(null), DEFAULT_CONTROLS);
  assert.deepEqual(parseControls("not json"), DEFAULT_CONTROLS);
  assert.deepEqual(parseControls(JSON.stringify({ view: "collection", sort: "name", progress: "bogus", category: "Melee" })),
    { ...DEFAULT_CONTROLS, view: "collection", sort: "name", category: "Melee" });
});

test("filters narrow by category, progress and availability; search matches the name", () => {
  const list = [
    opportunity("Braton"),
    opportunity("Hek", { category: "Primary", state: "missing", earned_rank: 0, remaining_mastery: 3000, access: "blocked", blockers: ["Requires MR 4"] }),
    opportunity("Skana", { category: "Melee", state: "unknown", earned_rank: null, remaining_mastery: null, access: "unknown" }),
  ];
  const names = (o: Opportunity[]) => o.map(x => x.name);
  assert.deepEqual(names(visibleOpportunities(list, DEFAULT_CONTROLS, "")), ["Braton", "Hek", "Skana"]);
  assert.deepEqual(names(visibleOpportunities(list, { ...DEFAULT_CONTROLS, category: "Melee" }, "")), ["Skana"]);
  assert.deepEqual(names(visibleOpportunities(list, { ...DEFAULT_CONTROLS, progress: "missing" }, "")), ["Hek"]);
  assert.deepEqual(names(visibleOpportunities(list, { ...DEFAULT_CONTROLS, availability: "available" }, "")), ["Braton"]);
  assert.deepEqual(names(visibleOpportunities(list, { ...DEFAULT_CONTROLS, availability: "unknown" }, "")), ["Skana"]);
  assert.deepEqual(names(visibleOpportunities(list, DEFAULT_CONTROLS, "sKa")), ["Skana"]);
});

test("sorting by name stays inside the stage order", () => {
  const list = [
    opportunity("Zenith", { remaining_mastery: 3000 }),
    opportunity("Braton"),
    opportunity("Amprex", { stage: "acquire", action: "buy", owned: false, owned_level: null, vendors: [{ syndicate: "Steel Meridian", tier: "General", blueprint: false }] }),
    opportunity("Vectis", { stage: "acquire", action: "buy", owned: false, owned_level: null, remaining_mastery: null }),
  ];
  assert.deepEqual(visibleOpportunities(list, DEFAULT_CONTROLS, "").map(o => o.name), ["Zenith", "Braton", "Amprex", "Vectis"]);
  assert.deepEqual(visibleOpportunities(list, { ...DEFAULT_CONTROLS, sort: "name" }, "").map(o => o.name), ["Braton", "Zenith", "Amprex", "Vectis"]);
});

test("labels spell out the action, the route and unknowns", () => {
  const now = 10_000_000;
  assert.equal(remainingText(1800), "+1,800");
  assert.equal(remainingText(null), "Unknown");
  assert.equal(actionText(opportunity("Braton", { owned_level: 5 })), "Level R5 → R30");
  assert.equal(actionText(opportunity("Braton", { owned_level: null })), "Level to R30");
  assert.equal(actionText(opportunity("Hek", { action: "claim", owned: false, owned_level: null, build_completion_ms: 5_000_000 })), "Claim from Foundry");
  const bought = opportunity("Hek", { action: "buy", owned: false, owned_level: null, vendors: [
    { syndicate: "Cephalon Simaris", tier: "", blueprint: true },
    { syndicate: "Steel Meridian", tier: "General", blueprint: false },
  ] });
  assert.equal(actionText(bought), "Buy blueprint from Cephalon Simaris");
  assert.equal(detailText(bought, now), "Cephalon Simaris (blueprint) · Steel Meridian, General");
  assert.equal(detailText(opportunity("Braton"), now), "Owned copy");
  assert.equal(detailText(opportunity("Braton", { build_completion_ms: 5_000_000 }), now), "Owned copy · Build ready");
  assert.equal(detailText(opportunity("Braton", { build_completion_ms: now + 60_000 }), now, "14:32"), "Owned copy · Build ready in 1m (14:32)");
  assert.equal(readyText(5_000_000, now), "ready");
  assert.equal(readyText(now + 3_720_000, now), "ready in 1h 2m");
  assert.equal(readyText(now + 45_000, now), "ready in 1m");
});
