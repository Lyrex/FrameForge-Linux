import assert from "node:assert/strict";
import { matchesSearchTerms, splitSearchTerms } from "./search.ts";

const terms = splitSearchTerms(" Braton, , SOMA ");
assert.deepEqual(terms, ["braton", "soma"]);
assert.equal(matchesSearchTerms(terms, "Braton Prime"), true);
assert.equal(matchesSearchTerms(terms, "Axi S1", "Soma Prime Barrel"), true);
assert.equal(matchesSearchTerms(terms, "Paris Prime"), false);
assert.equal(matchesSearchTerms(splitSearchTerms(" , "), "Paris Prime"), true);
