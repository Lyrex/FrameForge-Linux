// Run with: node --experimental-strip-types --test src/lib/ingredients.test.ts

import assert from "node:assert/strict";
import { test } from "node:test";
import { ingredients, ingredientTitle } from "./ingredients.ts";
import type { CraftPlan, IngredientState, Requirement } from "../types/mastery.ts";

const line = (over: Partial<Requirement>): Requirement => ({
  unique_name: "/ferrite", name: "Ferrite", image_name: "ferrite.png", needed: 10, from_stock: 10, short: 0, state: "owned", ...over,
});

const plan = (over: Partial<CraftPlan> = {}): CraftPlan => ({
  requirements: [
    line({ unique_name: "/bp", name: "Frost Blueprint", image_name: null, needed: 1, from_stock: 1 }),
    line({ unique_name: "/chassis", name: "Frost Chassis", needed: 1, from_stock: 0, state: "blocked" }),
    line({}),
  ],
  builds: [{ unique_name: "/chassis", name: "Frost Chassis", crafts: 1 }],
  credits: 40_000,
  credits_short: 15_000,
  ...over,
});

test("credits lead and the rest keep recipe order", () => {
  assert.deepEqual(ingredients(plan()).map(i => [i.name, i.state]), [
    ["Credits", "partial"], ["Frost Blueprint", "owned"], ["Frost Chassis", "blocked"], ["Ferrite", "owned"],
  ]);
});

test("credits read owned when nothing is short, missing when nothing is covered or the cost is unknown, and drop out at zero", () => {
  const credits = (over: Partial<CraftPlan>) => ingredients(plan(over))[0];
  assert.deepEqual(credits({ credits_short: 0 }),
    { unique_name: "/_currency/Credits", name: "Credits", image_name: null, needed: 40_000, from_stock: 40_000, short: 0, state: "owned" });
  assert.equal(credits({ credits_short: 40_000 }).state, "missing");
  const unknown = credits({ credits: null, credits_short: 0 });
  assert.equal(unknown.state, "missing");
  assert.equal(ingredientTitle(unknown), "Credits\nCost unknown");
  assert.equal(ingredients(plan({ credits: 0, credits_short: 0 }))[0].name, "Frost Blueprint");
});

test("the title is the name, owned over required, and a third line only for the states that need one", () => {
  const icon = (state: IngredientState): Requirement => line({ unique_name: "/bolto", name: "Bolto", image_name: null, needed: 2, from_stock: 0, state });
  assert.equal(ingredientTitle({ ...icon("owned"), from_stock: 2 }), "Bolto\n2 / 2");
  assert.equal(ingredientTitle(icon("partial")), "Bolto\n0 / 2");
  assert.equal(ingredientTitle(icon("missing")), "Bolto\n0 / 2");
  assert.equal(ingredientTitle(icon("buildable")), "Bolto\n0 / 2\nBuildable");
  assert.equal(ingredientTitle(icon("blocked")), "Bolto\n0 / 2\nBlocked by a missing ingredient");
  assert.equal(ingredientTitle(icon("master_first")), "Bolto\n0 / 2\nMaster first");
  assert.equal(ingredientTitle(icon("building")), "Bolto\n0 / 2\nBuilding in the Foundry");
  assert.equal(ingredientTitle(ingredients(plan({ credits_short: 15_000 }))[0]), "Credits\n25,000 / 40,000");
});
