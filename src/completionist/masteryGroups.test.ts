// Run with: node --experimental-strip-types --test src/completionist/masteryGroups.test.ts

import assert from "node:assert/strict";
import { test } from "node:test";
import { groupSources, masteryGroup } from "./masteryGroups.ts";
import type { MasterySource } from "../types/mastery.ts";

const source = (name: string, unique_name = `/Lotus/Weapons/Tenno/${name}`): MasterySource => ({
  unique_name, name, category: "Melee", image_name: null, mastery_req: null,
  cap: 30, earned_rank: 0, state: "missing",
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

test("groups keep the fixed order and unknown groups trail", () => {
  const groups = groupSources([
    source("Zzz Prime"), source("Kuva Karak"), source("Braton"), source("Aaa Prime"),
  ]);
  assert.deepEqual(groups.map(g => g.group), ["Standard", "Prime", "Kuva"]);
  assert.deepEqual(groups[1].sources.map(s => s.name), ["Aaa Prime", "Zzz Prime"]);
});
