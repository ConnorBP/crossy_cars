# Ranked v3 production verification and evidence runbook

This is the operator runbook and machine-readable evidence template for Ranked
v3. **A blank, partial, failed, or unauthenticated run is evidence to remain
disabled, not permission to waive a gate.** The committed Worker flag remains
`ROADY_V3_RANKED_ENABLED = "false"`.

## 1. Identify one tested release

Record one full 40-character commit SHA and one successful `CI` run ID for that
exact SHA on `master`. Download `web-release-<SHA>` and verify:

```sh
sha256sum --check v3-artifacts.sha256
(cd artifact-root && sha256sum --check release-artifacts.sha256)
python tools/check_release.py --dist artifact-root/dist
```

The CI log must show `cargo fmt`, all workspace/all-target Rust tests, immutable
v1/v2 inventory, Rust/TS v2/v3 canonical parity, wasm32 check and optimized
build, PBR/microtexture regression tests, Worker type/unit/security/replay,
workerd, D1 restoration/v3 migration, all seven browser suites, and the strict
WASM `< 25 MiB` result. Do not substitute a run from another SHA.

## 2. Disabled-first Worker deployment

Run **Deploy Cloudflare Leaderboard Worker** with that SHA/run ID and
`enable_ranked=false`. The workflow must:

1. rerun all Worker gates;
2. match source/release/migration hashes;
3. require all legacy and five v3 credentials;
4. apply only ordered additive migrations;
5. query remote `d1_migrations`, exact category rows, six `_v3` tables, and
   v3-only foreign keys/indexes;
6. deploy `ROADY_V3_RANKED_ENABLED=false` and install secrets;
7. observe two uncached exact disabled capability responses, disabled session
   issuance, `/healthz`, and `/v1/leaderboard` for the tested build SHA.

Keep `worker-production-evidence-<SHA>-<run>` for the release record. If
Cloudflare auth, D1 access, Turnstile credentials, or any runtime key is absent,
record the exact failed command/missing credential and stop. Do not request an
enable rollout.

## 3. Worker-before-Pages

Pages is triggered only by a successful Worker workflow, or manual dispatch
requires that exact Worker run ID/SHA and its evidence artifact. It embeds the
matching `ROADY_V3_CLIENT_HMAC_KEY` and exact key ID `v3.client.1`, rebuilds and
checks optimized WASM, re-probes the disabled Worker immediately before upload,
and records Pages artifact hashes.

## 4. Guarded enable rollout

Only after reviewing all evidence, land the code-level production parity latch
change in a dedicated commit. Do **not** change the committed/default Worker
environment flag. Run CI for that same commit, then manually dispatch Worker
with its successful CI run ID and `enable_ranked=true` (exact lowercase only).
The workflow first repeats the complete disabled deployment and two probes. It
refuses enablement unless the latch, tested SHA, migration query, hashes, Worker
tests, and probes all match. It then applies a runner-only exact `true` override
and probes the exact enabled capability twice.

Before accepting public traffic, separately exercise the Turnstile-backed
session/start/score/evidence flow to pending then live for both categories,
reject replay and cross-category attempts, and confirm Casual sends no writes.
If this credentialed smoke cannot be performed, roll back to disabled.

## 5. Rollback order

**Disable issuance first.** Deploy/set `ROADY_V3_RANKED_ENABLED=false` (or any
non-`true` value), then verify `/v3/session` returns `503 ranked_disabled` and
two uncached capabilities report `enabled:false`. Retain the Worker, D1 data,
seed keys, proof keys, and evidence capability keys so already-started sessions
can still submit scores/evidence with no completion TTL. Do not delete v3 data,
route v3 through v2, or disable score/evidence service before outstanding
started sessions are drained. Pages rollback is secondary.

## Machine-readable evidence template

Copy this JSON to the release ticket/artifact and replace every `null`. A
`failed`/`missing` item requires `effectiveCapability:false` and
`issuanceEnabled:false`.

```json
{
  "schema": "roady.production-gates.v1",
  "testedCommitSha": null,
  "ciRunId": null,
  "workerRunId": null,
  "pagesRunId": null,
  "defaultWorkerFlag": "false",
  "requestedRuntimeFlag": "false",
  "effectiveCapability": false,
  "issuanceEnabled": false,
  "gates": {
    "rustAndRules": {"status": "missing", "command": "cargo test --workspace --all-targets --locked"},
    "wasmAndRelease": {"status": "missing", "command": "trunk build --release --cargo-profile wasm-release && python tools/check_release.py"},
    "frozenV1V2": {"status": "missing", "command": "cd leaderboard && npm run test:frozen-v1-v2"},
    "canonicalParity": {"status": "missing", "command": "cargo test -p roady-score-rules && cd leaderboard && npx vitest run test/rules-v2.test.ts test/rules-v3.test.ts"},
    "worker": {"status": "missing", "command": "cd leaderboard && npm run test:ci"},
    "browser": {"status": "missing", "suites": ["audit", "desktop", "touch", "settings", "request-failure", "category-isolation", "replay"]},
    "remoteMigration": {"status": "missing", "command": "cd leaderboard && npm run verify:remote-v3", "migrationSha256": null},
    "disabledProductionProbes": {"status": "missing", "command": "cd leaderboard && npm run verify:production:disabled", "uncachedObservations": 0},
    "pagesAfterWorker": {"status": "missing", "workerRunId": null}
  },
  "artifactHashes": {"releaseManifestSha256": null, "v3ManifestSha256": null},
  "blocker": "missing production credentials and production probe evidence",
  "recordedAt": null
}
```
