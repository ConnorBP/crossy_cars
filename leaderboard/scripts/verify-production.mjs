#!/usr/bin/env node
/** Exact, uncached production probes used by disabled deploys and guarded rollout. */
import assert from "node:assert/strict";

const expectedEnabled = process.argv.includes("--enabled");
const baseArg = process.argv.find((value) => value.startsWith("http://") || value.startsWith("https://"));
const base = (baseArg || process.env.LEADERBOARD_BASE_URL || "").replace(/\/$/, "");
if (!base) throw new Error("missing LEADERBOARD_BASE_URL or URL argument");
const expectedSha = process.env.EXPECTED_COMMIT_SHA;
const expected = {
  ranked: { enabled: expectedEnabled, categories: ["rotation.v2.cluck_hunt", "rotation.v2.right_of_way"] },
  protocolVersion: 3,
  protocolId: "roady-protocol.v3",
  rulesVersion: 3,
  rulesId: "roady-rules.v3",
  policyVersion: 1,
  policyId: "roady-ranked-policy.v3.1",
  mode: "rotation",
};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const get = async (path) => fetch(`${base}${path}`, {
  cache: "no-store",
  headers: {
    "Cache-Control": "no-cache, no-store, max-age=0",
    Pragma: "no-cache",
    "X-Roady-Release-Probe": `${Date.now()}-${crypto.randomUUID()}`,
  },
});

let health;
let healthBody;
for (let attempt = 1; attempt <= 12; attempt += 1) {
  health = await get("/healthz");
  assert.equal(health.status, 200, "legacy health probe failed");
  healthBody = await health.json();
  assert.equal(healthBody.ok, true);
  if (!expectedSha || healthBody.build === expectedSha) break;
  if (attempt === 12) assert.equal(healthBody.build, expectedSha, "deployed Worker commit SHA mismatch after propagation retries");
  await sleep(attempt * 1000);
}
// v2 never exposed a capabilities route. Freeze that absence so v3 rollout
// cannot silently repurpose a v2 URL.
const v2Capability = await get("/v2/capabilities");
assert.equal(v2Capability.status, 404, "v2 capabilities route unexpectedly changed");
const v2Body = await v2Capability.json();
assert.equal(v2Body.error?.code, "not_found", "v2 absence body changed");
const board = await get("/v1/leaderboard?limit=1");
assert.equal(board.status, 200, "legacy board probe failed");
assert.ok(Array.isArray((await board.json()).entries), "legacy board entries missing");
const capability = async (label) => {
  const response = await fetch(`${base}/v3/capabilities`, {
    cache: "no-store",
    headers: {
      "Cache-Control": "no-cache, no-store, max-age=0",
      Pragma: "no-cache",
      "X-Roady-Release-Probe": `${Date.now()}-${label}-${crypto.randomUUID()}`,
    },
  });
  assert.equal(response.status, 200, `capability probe ${label} failed`);
  assert.equal(response.headers.get("cache-control"), "public, max-age=60, s-maxage=300, stale-while-revalidate=600");
  return response.json();
};
// Cloudflare may briefly serve the preceding Worker version after deploy. Wait
// for the expected effective gate before taking the two independent final
// observations. A mismatch after the bounded window still fails and triggers
// the enable workflow's disable-first rollback trap.
let propagated = false;
for (let attempt = 1; attempt <= 12; attempt += 1) {
  const body = await capability(`propagation-${attempt}`);
  if (JSON.stringify(body) === JSON.stringify(expected)) {
    propagated = true;
    break;
  }
  if (attempt < 12) await sleep(attempt * 1000);
}
assert.equal(propagated, true, "capability tuple did not propagate within the bounded window");
for (let probe = 1; probe <= 2; probe += 1) {
  assert.deepEqual(await capability(`final-${probe}`), expected, `capability final probe ${probe} tuple mismatch`);
}
if (!expectedEnabled) {
  const issuance = await fetch(`${base}/v3/session`, {
    method: "POST",
    headers: { "Content-Type": "application/json", "Cache-Control": "no-store" },
    body: JSON.stringify({ mode: "rotation", categoryKey: "rotation.v2.cluck_hunt", turnstileToken: "release-disabled-probe" }),
  });
  assert.equal(issuance.status, 503, "disabled production unexpectedly issued a session");
  assert.equal((await issuance.json()).error.code, "ranked_disabled");
}
console.log(`production probe passed: SHA/legacy/exact v3 capability (${expectedEnabled ? "enabled" : "disabled"}), two uncached observations`);
