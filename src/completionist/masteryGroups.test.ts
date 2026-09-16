// Run with: node --experimental-strip-types --test src/completionist/masteryGroups.test.ts

import assert from "node:assert/strict";
import { test } from "node:test";
import { groupSources, masteryGroup, systemRank } from "./masteryGroups.ts";
import type { MasterySource } from "../types/mastery.ts";

const source = (name: string, unique_name = `/Lotus/Weapons/Tenno/${name}`): MasterySource => ({
  unique_name, name, category: "Melee", image_name: null, mastery_req: null,
  cap: 30, earned_rank: 0, remaining_mastery: 3000, state: "missing", unobtainable: null, excluded: false,
});

test("variant prefixes and modular paths decide the group", () => {
  assert.equal(masteryGroup(source("Braton")), "Standard");
  assert.equal(masteryGroup(source("Braton Prime")), "Prime");
  assert.equal(masteryGroup(source("Kuva Karak")), "Kuva");
  assert.equal(masteryGroup(source("Tenet Envoy")), "Tenet");
  assert.equal(masteryGroup(source("Coda Motovore")), "Coda");
  assert.equal(masteryGroup(source("Prisma Skana")), "Prisma");
  assert.equal(masteryGroup(source("Latron Wraith")), "Wraith");
  assert.equal(masteryGroup(source("Prova Vandal")), "Vandal");
  assert.equal(masteryGroup(source("Mk1-Bo")), "MK1");
  assert.equal(masteryGroup(source("Balla", "/Lotus/Weapons/Ostron/Melee/ModularMelee01/Tip/TipOne")), "Zaw");
  assert.equal(masteryGroup(source("Catchmoon", "/Lotus/Weapons/SolarisUnited/Secondary/SUModularSecondarySet1/Barrel/A")), "Kitgun");
  assert.equal(masteryGroup(source("Raplak Prism", "/Lotus/Weapons/Sentients/OperatorAmplifiers/Set1/Barrel/A")), "Amp");
});

test("Intrinsic tracks group by system, Railjack first, with the summed rank over the cap", () => {
  const track = (name: string, field: string, rank: number | null): MasterySource =>
    ({ ...source(name, field), category: "Intrinsics", cap: 10, earned_rank: rank });
  const groups = groupSources([track("Combat", "LPS_DRIFT_COMBAT", 10), track("Piloting", "LPS_PILOTING", 9), track("Tactical", "LPS_TACTICAL", 10)]);
  assert.deepEqual(groups.map(g => g.group), ["Railjack", "Drifter"]);
  assert.equal(systemRank(groups[0].sources), "19/20");
  assert.equal(systemRank([track("Piloting", "LPS_PILOTING", null)]), "?/10");
});

test("star chart rows group by planet in the order they arrive, junctions and mode pairs intact", () => {
  const node = (name: string, key: string, planet: string, mode: "normal" | "steel_path", junction = false): MasterySource => ({
    ...source(name, mode === "normal" ? key : `${key}/steel_path`), category: "Star Chart", cap: 1, node: { key, planet, mode, junction, amount: 18 },
  });
  const rows = [
    node("Mercury Junction", "VenusToMercuryJunction", "Venus", "normal", true),
    node("Mercury Junction", "VenusToMercuryJunction", "Venus", "steel_path", true),
    node("Aphrodite", "SolNode2", "Venus", "normal"),
    node("Aphrodite", "SolNode2", "Venus", "steel_path"),
    node("Apollodorus", "SolNode94", "Mercury", "normal"),
  ];
  const groups = groupSources(rows);
  assert.deepEqual(groups.map(g => g.group), ["Venus", "Mercury"]);
  assert.deepEqual(groups[0].sources.map(s => s.unique_name),
    ["VenusToMercuryJunction", "VenusToMercuryJunction/steel_path", "SolNode2", "SolNode2/steel_path"]);
});

test("groups keep the fixed order and unknown groups trail", () => {
  const groups = groupSources([
    source("Zzz Prime"), source("Kuva Karak"), source("Braton"), source("Aaa Prime"),
  ]);
  assert.deepEqual(groups.map(g => g.group), ["Standard", "Prime", "Kuva"]);
  assert.deepEqual(groups[1].sources.map(s => s.name), ["Aaa Prime", "Zzz Prime"]);
});
