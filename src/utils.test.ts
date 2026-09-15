// Run with: node --experimental-strip-types --test src/utils.test.ts

import assert from "node:assert/strict";
import { test } from "node:test";
import { wfmSlugLookup } from "./utils.ts";

test("a catalogue blueprint name finds the market slug with or without the suffix", () => {
  const lookup = wfmSlugLookup([
    { id: "1", item_name: "Braton Prime Receiver", url_name: "braton_prime_receiver" },
    { id: "2", item_name: "Braton Prime Blueprint", url_name: "braton_prime_blueprint" },
    { id: "3", item_name: "Braton Prime Set", url_name: "braton_prime_set" },
  ]);
  assert.equal(lookup.get("braton_prime_receiver_blueprint"), "braton_prime_receiver");
  assert.equal(lookup.get("braton_prime"), "braton_prime_blueprint");
  // An exact name is never shadowed by another item's alias.
  assert.equal(lookup.get("braton_prime_blueprint"), "braton_prime_blueprint");
  assert.equal(lookup.get("braton_prime_set"), "braton_prime_set");
});
