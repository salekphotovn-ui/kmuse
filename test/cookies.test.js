import test from "node:test";
import assert from "node:assert/strict";
import { normalizeCookies } from "../src/index.js";

test("accepts only muse.ai cookies and never changes values", () => {
  const result = normalizeCookies([
    { name: "session", value: "secret", domain: ".muse.ai", path: "/", sameSite: "lax" },
    { name: "foreign", value: "nope", domain: ".example.com" },
  ]);

  assert.equal(result.accepted.length, 1);
  assert.equal(result.rejected, 1);
  assert.equal(result.accepted[0].value, "secret");
  assert.equal(result.accepted[0].sameSite, "Lax");
});

test("rejects a file without any allowed cookies", () => {
  assert.throws(
    () => normalizeCookies([{ name: "x", value: "y", domain: "example.com" }]),
    /No valid muse\.ai cookies/,
  );
});
