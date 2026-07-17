#!/usr/bin/env node
/** Exercise the bundled Worker in Cloudflare's workerd runtime, not Node mocks. */
import { execFileSync, spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import assert from "node:assert/strict";

const root = resolve(import.meta.dirname, "..");
const wrangler = join(root, "node_modules", "wrangler", "bin", "wrangler.js");
const temp = mkdtempSync(join(tmpdir(), "roady-workerd-"));
const state = join(temp, "state");
const envFile = join(temp, "workerd.env");
const seedKey = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
writeFileSync(envFile, [
  "LB_SESSION_HMAC_KEY=local-session-key-at-least-32-bytes-long",
  "LB_IP_HASH_PEPPER=local-ip-pepper-at-least-32-bytes-long",
  "LB_ADMIN_TOKEN=local-admin-token-at-least-32-bytes-long",
  "LB_TURNSTILE_SECRET=1x0000000000000000000000000000000AA",
  "LB_CLIENT_HMAC_KEY=local-client-key-at-least-32-bytes-long",
  "LB_V3_PROOF_HMAC_KEY=local-v3-proof-key-at-least-32-bytes-long",
  `LB_V3_SEED_ENCRYPTION_KEY=${seedKey}`,
  "LB_V3_SEED_KEY_ID=v3.seed.local.1",
  "LB_V3_EVIDENCE_CAPABILITY_KEY=local-v3-capability-key-at-least-32-bytes",
  'LB_V3_CLIENT_HMAC_KEYS_JSON={"v3.client.1":"local-v3-client-key-at-least-32-bytes"}',
  "ROADY_V3_RANKED_ENABLED=false",
].join("\n") + "\n");

const freePort = () => new Promise((resolvePort, reject) => {
  const server = createServer();
  server.once("error", reject);
  server.listen(0, "127.0.0.1", () => {
    const address = server.address();
    const port = typeof address === "object" && address ? address.port : 0;
    server.close((error) => error ? reject(error) : resolvePort(port));
  });
});

let child;
try {
  execFileSync(process.execPath, [wrangler, "d1", "migrations", "apply", "roady_leaderboard", "--local", "--persist-to", state], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const port = await freePort();
  child = spawn(process.execPath, [wrangler, "dev", "--local", "--ip", "127.0.0.1", "--port", String(port), "--persist-to", state, "--env-file", envFile, "--log-level", "error"], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, CI: "true" },
  });
  let logs = "";
  child.stdout.on("data", (chunk) => { logs += chunk; });
  child.stderr.on("data", (chunk) => { logs += chunk; });
  const base = `http://127.0.0.1:${port}`;
  let health;
  for (let attempt = 0; attempt < 90; attempt += 1) {
    if (child.exitCode !== null) throw new Error(`workerd exited early (${child.exitCode})\n${logs}`);
    try {
      health = await fetch(`${base}/healthz`);
      if (health.ok) break;
    } catch { /* startup */ }
    await new Promise((resolveWait) => setTimeout(resolveWait, 250));
  }
  assert.equal(health?.status, 200, `workerd did not become ready\n${logs}`);
  assert.equal((await health.json()).ok, true);

  const capability = await fetch(`${base}/v3/capabilities`, { headers: { "Cache-Control": "no-cache" } });
  assert.equal(capability.status, 200);
  assert.equal(capability.headers.get("cache-control"), "public, max-age=60, s-maxage=300, stale-while-revalidate=600");
  assert.deepEqual(await capability.json(), {
    ranked: { enabled: false, categories: ["rotation.v2.cluck_hunt", "rotation.v2.right_of_way"] },
    protocolVersion: 3,
    protocolId: "roady-protocol.v3",
    rulesVersion: 3,
    rulesId: "roady-rules.v3",
    policyVersion: 1,
    policyId: "roady-ranked-policy.v3.1",
    mode: "rotation",
  });
  const session = await fetch(`${base}/v3/session`, { method: "POST", headers: { "Content-Type": "application/json" }, body: "{}" });
  assert.equal(session.status, 503);
  assert.equal((await session.json()).error.code, "ranked_disabled");
  const board = await fetch(`${base}/v1/leaderboard?limit=1`);
  assert.equal(board.status, 200);
  assert.ok(Array.isArray((await board.json()).entries));
  console.log("workerd runtime: health, legacy board, exact disabled v3 capability, and issuance denial passed");
} finally {
  if (child && child.exitCode === null) {
    child.kill("SIGTERM");
    await new Promise((resolveWait) => setTimeout(resolveWait, 250));
    if (child.exitCode === null) child.kill("SIGKILL");
  }
  rmSync(temp, { recursive: true, force: true });
}
