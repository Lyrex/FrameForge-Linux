// Run with: node --experimental-strip-types --test src/updateFlow.test.ts

import assert from "node:assert/strict";
import { test } from "node:test";
import { checkMessage, errorText } from "./updateFlow.ts";

test("a found update is named by version", () => {
  const update = { version: "3.10.0", currentVersion: "3.9.1", notes: null };
  assert.equal(checkMessage({ kind: "available", update }), "Version 3.10.0 is available.");
});

test("no update is reported rather than passed over in silence", () => {
  assert.match(checkMessage({ kind: "current" }), /up to date/);
});

test("a failed check reads as a failure, not as being current", () => {
  const message = checkMessage({ kind: "failed", message: "network unreachable" });
  assert.match(message, /Could not check/);
  assert.match(message, /network unreachable/);
});

test("both rejection shapes yield readable text", () => {
  assert.equal(errorText(new Error("signature mismatch")), "signature mismatch");
  assert.equal(errorText("signature mismatch"), "signature mismatch");
});
