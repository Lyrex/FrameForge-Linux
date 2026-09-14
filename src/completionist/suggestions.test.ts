// Run with: node --experimental-strip-types --test src/completionist/suggestions.test.ts

import assert from "node:assert/strict";
import { test } from "node:test";
import { actionText, chanceText, DEFAULT_CONTROLS, detailText, inSuggestions, parseControls, readyText, remainingText, visibleOpportunities } from "./suggestions.ts";
import { RELIC_SUGGESTION_THRESHOLD } from "../constants/relics.ts";
import type { Coverage, Opportunity, RelicPart } from "../types/mastery.ts";

const opportunity = (name: string, over: Partial<Opportunity> = {}): Opportunity => ({
  unique_name: `/Lotus/Weapons/Tenno/${name}`, name, category: "Primary", image_name: null, mastery_req: null,
  cap: 30, earned_rank: 12, remaining_mastery: 1800, state: "partial", unobtainable: null, excluded: false,
  stage: "level_claim", action: "level", owned: true, owned_level: 12, build_completion_ms: null,
  vendors: [], spend: null, craft: null, relic: null, access: "available", blockers: [], ...over,
});

const relicFarm = (name: string, coverage: Coverage, parts: RelicPart[] = [], over: Partial<Opportunity> = {}): Opportunity =>
  opportunity(name, { stage: "acquire", action: "farm", owned: false, owned_level: null, state: "missing", earned_rank: 0, remaining_mastery: 3000,
    craft: { requirements: [], builds: [], credits: 15_000, credits_short: 0 }, relic: { parts, coverage }, ...over });

const complete = (probability: number): Coverage => ({ kind: "complete", probability });

test("stored controls are validated field by field and fall back to defaults", () => {
  assert.deepEqual(parseControls(null), DEFAULT_CONTROLS);
  assert.deepEqual(parseControls("not json"), DEFAULT_CONTROLS);
  assert.deepEqual(parseControls(JSON.stringify({ view: "collection", sort: "name", progress: "bogus", category: "Melee", hideRelics: true })),
    { ...DEFAULT_CONTROLS, view: "collection", sort: "name", category: "Melee", hideRelics: true });
  assert.equal(parseControls(JSON.stringify({ hideRelics: "yes" })).hideRelics, false);
});

test("a relic route joins Suggestions strictly above the threshold, on the unrounded chance", () => {
  assert.equal(RELIC_SUGGESTION_THRESHOLD, 0.85);
  assert.equal(inSuggestions(relicFarm("Braton Prime", complete(0.8499))), false);
  assert.equal(inSuggestions(relicFarm("Braton Prime", complete(0.85))), false);
  assert.equal(inSuggestions(relicFarm("Braton Prime", complete(0.8501))), true);
  assert.equal(inSuggestions(relicFarm("Braton Prime", { kind: "partial", missing: ["Barrel"], short: [] })), false);
  assert.equal(inSuggestions(relicFarm("Braton Prime", { kind: "unknown" })), false);
  assert.equal(inSuggestions(opportunity("Braton")), true);
});

test("Suggestions and More relics split the list, and More relics ranks by chance then partial then unknown", () => {
  const list = [
    opportunity("Braton"),
    relicFarm("Akstiletto Prime", { kind: "unknown" }),
    relicFarm("Vectis Prime", complete(0.42)),
    relicFarm("Saryn Prime", { kind: "partial", missing: ["Neuroptics Blueprint"], short: [] }, [], { remaining_mastery: 6000 }),
    relicFarm("Braton Prime", complete(0.9)),
    relicFarm("Vasto Prime", complete(0.85)),
  ];
  const names = (o: Opportunity[]) => o.map(x => x.name);
  assert.deepEqual(names(visibleOpportunities(list, DEFAULT_CONTROLS, "")), ["Braton", "Braton Prime"]);
  assert.deepEqual(names(visibleOpportunities(list, { ...DEFAULT_CONTROLS, hideRelics: true }, "")), ["Braton"]);
  const relics = { ...DEFAULT_CONTROLS, result: "relics" as const };
  assert.deepEqual(names(visibleOpportunities(list, relics, "")), ["Vasto Prime", "Vectis Prime", "Saryn Prime", "Akstiletto Prime"]);
  assert.deepEqual(names(visibleOpportunities(list, { ...relics, hideRelics: true }, "")), ["Vasto Prime", "Vectis Prime", "Saryn Prime", "Akstiletto Prime"]);
  assert.deepEqual(names(visibleOpportunities(list, relics, "prime")), ["Vasto Prime", "Vectis Prime", "Saryn Prime", "Akstiletto Prime"]);
  assert.deepEqual(names(visibleOpportunities(list, { ...relics, availability: "blocked" }, "")), []);
  // A stock change that lifts the chance past the threshold moves the row.
  const lifted = list.map(o => o.name === "Vectis Prime" ? relicFarm("Vectis Prime", complete(0.86)) : o);
  assert.deepEqual(names(visibleOpportunities(lifted, DEFAULT_CONTROLS, "")), ["Braton", "Vectis Prime", "Braton Prime"]);
  assert.deepEqual(names(visibleOpportunities(lifted, relics, "")), ["Vasto Prime", "Saryn Prime", "Akstiletto Prime"]);
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
  const barrel = { unique_name: "/Lotus/Types/Recipes/Weapons/WeaponParts/BratonPrimeBarrel", name: "Barrel", needed: 1, from_stock: 0, short: 1 };
  const relicParts: RelicPart[] = [{ unique_name: barrel.unique_name, name: "Barrel", needed: 2, relics: [
    { name: "Lith B4 Radiant", count: 3, chance: 0.1667 }, { name: "Lith B4 Intact", count: 1, chance: 0.2533 },
  ] }];
  const relicFarmed = relicFarm("Braton Prime", complete(0.4231), relicParts,
    { craft: { requirements: [barrel, ferrite], builds: [], credits: 15_000, credits_short: 0 } });
  assert.equal(actionText(relicFarmed), "Farm relics + 1 item");
  assert.equal(actionText({ ...relicFarmed, craft: { ...relicFarmed.craft!, requirements: [barrel] } }), "Farm relics");
  assert.equal(chanceText(relicFarmed.relic!.coverage), "42%");
  assert.equal(chanceText(complete(0.005)), "0.5%");
  assert.equal(chanceText(complete(0)), "0%");
  assert.equal(chanceText({ kind: "partial", missing: [], short: ["Barrel"] }), "Partial");
  assert.equal(chanceText({ kind: "unknown" }), "Unknown");
  assert.equal(detailText(relicFarmed, now), "Barrel ×2 from Lith B4 Radiant ×3, Lith B4 Intact ×1 · Credits 15,000 · Short Ferrite ×100");
  const partial = relicFarm("Braton Prime", { kind: "partial", missing: ["Blueprint"], short: ["Barrel"] },
    [{ unique_name: "/bp", name: "Blueprint", needed: 1, relics: [] }, ...relicParts]);
  assert.equal(detailText(partial, now), "No relic for Blueprint · Too few relics for Barrel · Barrel ×2 from Lith B4 Radiant ×3, Lith B4 Intact ×1 · Credits 15,000");
  assert.equal(readyText(5_000_000, now), "ready");
  assert.equal(readyText(now + 3_720_000, now), "ready in 1h 2m");
  assert.equal(readyText(now + 45_000, now), "ready in 1m");
});
