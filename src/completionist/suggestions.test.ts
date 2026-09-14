// Run with: node --experimental-strip-types --test src/completionist/suggestions.test.ts

import assert from "node:assert/strict";
import { test } from "node:test";
import { actionText, DEFAULT_CONTROLS, detailText, parseControls, readyText, remainingText, visibleOpportunities } from "./suggestions.ts";
import type { Opportunity } from "../types/mastery.ts";

const opportunity = (name: string, over: Partial<Opportunity> = {}): Opportunity => ({
  unique_name: `/Lotus/Weapons/Tenno/${name}`, name, category: "Primary", image_name: null, mastery_req: null,
  cap: 30, earned_rank: 12, remaining_mastery: 1800, state: "partial", unobtainable: null, excluded: false,
  stage: "level_claim", action: "level", owned: true, owned_level: 12, build_completion_ms: null,
  vendors: [], spend: null, craft: null, access: "available", blockers: [], ...over,
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
  const node = { key: "SolNode27", planet: "Earth", mode: "normal" as const, junction: false };
  const ePrime = opportunity("E Prime", { unique_name: "SolNode27", category: "Star Chart", cap: 1, earned_rank: 0, state: "missing",
    stage: "acquire", action: "complete", remaining_mastery: null, owned: false, owned_level: null, access: "unknown", blockers: ["Node unlock not observed"], node });
  assert.equal(actionText(ePrime), "Complete node");
  assert.equal(detailText(ePrime, now), "Earth");
  assert.equal(actionText({ ...ePrime, action: "unlock", node: { ...node, junction: true } }), "Unlock junction");
  assert.equal(detailText(opportunity("Braton", { build_completion_ms: 5_000_000 }), now), "Owned copy · Build ready");
  assert.equal(detailText(opportunity("Braton", { build_completion_ms: now + 60_000 }), now, "14:32"), "Owned copy · Build ready in 1m (14:32)");
  const spend = opportunity("Railjack", {
    unique_name: "LPP_SPACE", category: "Intrinsics", cap: 50, earned_rank: 45, remaining_mastery: 7500, owned: false, owned_level: null,
    action: "spend", spend: { ranks: 5, points: 2048, mastery: 7500, tracks: [
      { track: "Piloting", from: 9, to: 10 }, { track: "Gunnery", from: 8, to: 10 }, { track: "Engineering", from: 8, to: 10 },
    ] },
  });
  assert.equal(actionText(spend), "Spend 2,048 points for 5 ranks");
  assert.equal(detailText(spend, now), "+7,500 mastery · Piloting R9 → R10 · Gunnery R8 → R10 · Engineering R8 → R10");
  assert.equal(actionText(opportunity("Drifter", { action: "spend", spend: { ranks: 1, points: 205, mastery: 1500, tracks: [{ track: "Endurance", from: 8, to: 9 }] } })), "Spend 205 points for 1 rank");
  const chassis = { unique_name: "/Lotus/Types/Recipes/Parts/Chassis", name: "Chassis", needed: 1, from_stock: 0, short: 0 };
  const ferrite = { unique_name: "/Lotus/Types/Items/MiscItems/Ferrite", name: "Ferrite", needed: 150, from_stock: 50, short: 100 };
  const crafted = opportunity("Hek", { stage: "craft", action: "craft", owned: false, owned_level: null,
    craft: { requirements: [chassis], builds: [], credits: 15_000, credits_short: 0 } });
  assert.equal(actionText(crafted), "Craft now");
  assert.equal(actionText({ ...crafted, access: "blocked", blockers: ["Needs 5,000 more credits"] }), "Craft");
  assert.equal(detailText(crafted, now), "Credits 15,000");
  const built = opportunity("Hek", { stage: "craft", action: "build", owned: false, owned_level: null,
    craft: { requirements: [chassis], builds: [{ unique_name: chassis.unique_name, name: "Chassis", crafts: 2 }], credits: null, credits_short: 0 } });
  assert.equal(actionText(built), "Build 1 part, then craft");
  assert.equal(detailText(built, now), "Credits unknown · Build Chassis ×2");
  const farmed = opportunity("Hek", { stage: "craft", action: "farm", owned: false, owned_level: null,
    craft: { requirements: [chassis, ferrite], builds: [], credits: 20_000, credits_short: 5_000 } });
  assert.equal(actionText(farmed), "Farm 1 item");
  assert.equal(detailText(farmed, now), "Credits 20,000 · Short Ferrite ×100");
  assert.equal(readyText(5_000_000, now), "ready");
  assert.equal(readyText(now + 3_720_000, now), "ready in 1h 2m");
  assert.equal(readyText(now + 45_000, now), "ready in 1m");
});
