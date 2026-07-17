# Normative Rules-v3 / Drowned Protocol Addendum

**Status:** normative addendum proposed for review; documentation only  
**Baseline:** repository HEAD `ddf6745c76f6eb450378e8acc74c489ef1b56b04`  
**Normative language:** MUST, MUST NOT, SHOULD, and MAY are used as requirements.

## 1. Scope, precedence, and immutable baseline

This document adds rules v3 solely to make `Drowned` a ranked terminal outcome. It does not reopen any gameplay decision in `docs/APPROVED_GAMEPLAY_MODES_CONTRACT.md` (the **v2 contract**). Except where this document explicitly replaces a v2 identifier or canonical layout with a v3 identifier/layout, every gameplay rule, arithmetic rule, ordering rule, bound, security control, lifecycle, moderation state, API error, and deployment gate in the v2 contract is incorporated unchanged.

The v2 contract hard-binds `/v2` to exactly `protocolVersion=2`, `rulesVersion=2`, `policyVersion=1`, `mode="rotation"`, and the two approved v2 categories, and requires rejection of every other tuple. Therefore:

1. `/v2` MUST continue accepting only its exact v2 tuple. It MUST NOT advertise or accept rules v3 or `drowned`.
2. Rules v3 and Drowned MUST use additive `/v3` routes and the exact tuple frozen below. No `/v2` body, response, canonical byte, fixture, table, board, or behavior may change.
3. Every tracked v1/v2 JSON, schema, golden, migration 0001--0004, v1/v2 route fixture, and public-board artifact existing at the baseline commit is frozen byte-for-byte. Existing v2 migration 0005, if subsequently materialized from the approved contract, is likewise immutable once reviewed and merged.
4. This chunk is documentation only. Implementation requires normative sign-off and a later commit.

## 2. Frozen identity and board isolation

The only accepted v3 tuple is:

| Field | Exact value |
|---|---|
| protocol version | integer `3` |
| protocol ID | `roady-protocol.v3` |
| rules version | integer `3` |
| rules ID | `roady-rules.v3` |
| policy version | integer `1` |
| policy ID | `roady-ranked-policy.v3.1` |
| mode | `rotation` |
| client signature key ID at initial launch | `v3.client.1` |

The only v3 category keys are `rotation.v2.cluck_hunt` and `rotation.v2.right_of_way`. Their display names are `Cluck Hunt` and `Right of Way`. Category-key suffix `v2` is the category/board epoch, not the protocol version. The exact allowed tuples are:

```
(3, "roady-protocol.v3", 3, "roady-rules.v3", 1,
 "roady-ranked-policy.v3.1", "rotation", "rotation.v2.cluck_hunt")
(3, "roady-protocol.v3", 3, "roady-rules.v3", 1,
 "roady-ranked-policy.v3.1", "rotation", "rotation.v2.right_of_way")
```

Every other tuple or category MUST be rejected with `422 unknown_version_tuple` before conduct-specific parsing. v1, v2, and v3 sessions, scores, evidence, moderation, rank queries, caches, categories, and leaderboards MUST be isolated. A v3 query MUST never read a v1/v2 score; a v1/v2 query MUST never read a v3 score. A v2 proof, capability, signature, seed/schedule commitment, restoration key, or canonical byte sequence MUST be unusable in v3.

## 3. Additive routes and capability gate

The exact v3 route list is:

- `GET /v3/capabilities`
- `POST /v3/session`
- `POST /v3/session/:id/start`
- `POST /v3/scores`
- `POST /v3/evidence`
- `GET /v3/leaderboard`
- `GET /v3/me/rank`
- `POST /v3/admin/scores/restore`
- `POST /v3/admin/scores/:id/hide`
- `DELETE /v3/admin/scores/:id`

`OPTIONS` is supported for each route under the v2 CORS rules. No `/v3` alias may be added under `/v1`, `/v2`, or `/api`.

`GET /v3/capabilities` takes no body or authentication and MUST return exactly this key set and key order (insignificant transport whitespace):

```json
{"ranked":{"enabled":BOOL,"categories":["rotation.v2.cluck_hunt","rotation.v2.right_of_way"]},"protocolVersion":3,"protocolId":"roady-protocol.v3","rulesVersion":3,"rulesId":"roady-rules.v3","policyVersion":1,"policyId":"roady-ranked-policy.v3.1","mode":"rotation"}
```

It MUST set `Cache-Control: public, max-age=60, s-maxage=300, stale-while-revalidate=600`. The body cached at the edge MUST be origin-agnostic; CORS headers MUST be reapplied per request. Capability cache keys MUST include route version and deployment environment and MUST NOT collide with `/v2/capabilities`.

The sole production enable switch is the Worker environment variable `ROADY_V3_RANKED_ENABLED`. Ranked is enabled only when its trimmed value is exactly lowercase ASCII `true` **and** every gate in section 9 has passed. Missing, empty, placeholder, mixed-case, or any other value means disabled. The variable is an upper bound, not an override: failed config, migration, artifact parity, evidence replay, or deployment verification forces `enabled=false`. Non-production environments MUST NOT make production capabilities true. Disabling MUST stop new v3 session issuance without invalidating already-started sessions; those retain no completion TTL and may submit/evidence under the tuple with which they started.

The client MUST enable a Ranked menu action only if a fresh successful `/v3/capabilities` response matches the JSON above exactly in every ID/version/mode/category and `ranked.enabled === true`. Extra/missing categories, reordered categories, extra tuple values, unknown fields, stale v2 data, parse/fetch failure, or any mismatch MUST disable Ranked for that client session. Cached stale-while-revalidate data MAY render non-interactive status but MUST NOT independently unlock Ranked after a failure. The contract default remains Ranked Cluck Hunt when the exact live tuple permits; otherwise only Casual is actionable.

## 4. Drowned terminal discriminant and ordering

The v3 terminal-reason enum is stable and additive:

| Reason | Ordinal | JSON |
|---|---:|---|
| TimeUp | `1` | `time_up` |
| Wrecked | `2` | `wrecked` |
| Drowned | `3` | `drowned` |

TimeUp and Wrecked MUST NOT be renumbered. Ordinal `0`, ordinals `4..255`, and unknown JSON values MUST be rejected. Drowned is eligible for Ranked submission in **both** `cluck_hunt` and `right_of_way`; it MUST NOT be silently suppressed, converted to Wrecked, or forced Casual.

At a terminal edge, all same-ms expirations, segment changes, activation actions, gameplay transitions, and any final objective completion/reward MUST be processed first. Exactly one Terminal event with reason Drowned=3 is then appended. Terminal remains the last ledger event. Only after the canonical terminal aggregate, Terminal event hash, and final root have been finalized may the immutable GameOver snapshot be created; that snapshot MUST expose the same terminal reason and aggregates. Thus the total order is:

```
final gameplay transition -> final objective edge -> objective reward
-> Terminal(reason=3) -> final root -> GameOver snapshot
```

If pond entry races another terminal request in the same active-play millisecond and the authoritative game resolves the run as Drowned, there MUST still be only one Terminal event and one GameOver snapshot, both Drowned. No score transition occurs merely because drowning starts; only the completed terminal edge is canonical.

## 5. Canonical v3 encoding

### 5.1 Primitives, domains, and bounds

All v2 primitive rules carry forward: integers are big-endian; enum/flag/version fields are u8; count/sequence/nonnegative aggregate fields are u32; timestamps are u64; RightOfWay accumulators are i64; schedule count is u16; hashes are raw 32 bytes; `lp1` is a u8 length followed by 1--255 UTF-8 bytes; `lp4` is a u32 length followed by bytes. Hex is lowercase and base64url is canonical unpadded.

The exact v3 domains, always encoded as `lp1(domain)` at the head of their block, are:

- `roady.v3.session`
- `roady.v3.score`
- `roady.v3.event`
- `roady.v3.root`
- `roady.v3.schedule`
- `roady.v3.seed`
- `roady.v3.proof`
- `roady.v3.evidence`

The PRNG domains are exactly:

- `roady.rotation.v3.rotation`
- `roady.rotation.v3.scheduled_events`
- `roady.rotation.v3.frenzy.interval`
- `roady.rotation.v3.frenzy.roll`
- `roady.rotation.v3.frenzy.kind`
- `roady.rotation.v3.frenzy.position`
- `roady.rotation.v3.frenzy.relocation`

All v2 limits remain exact in v3: 4096 events; 262144 canonical ledger bytes; 192 bytes per event record (excluding its stored 32-byte event hash); 16 schedule segments; 32 activations; 16384-byte score body; 524288-byte evidence body; build 1--64 UTF-8 bytes; name 3--5 ASCII alphanumeric; remaining time 0--99000 ms. Session IDs and challenges are 1--255 bytes. Overflow or oversize MUST reject; no truncation is permitted.

### 5.2 Version prefix and layouts

Define the exact `V3` prefix:

```
u8(3)||u8(3)||u8(1)
||lp1("roady-protocol.v3")||lp1("roady-rules.v3")
||lp1("roady-ranked-policy.v3.1")||lp1("rotation")||lp1(category)
```

The unstarted session header is:

```
lp1("roady.v3.session")||V3||lp1(sessionId)||lp1(challenge)
||seedCommitment32||scheduleHash32||u64(issuedAt)||u64(startByExpiry)
||u8(0)||u64(0)
```

The started header replaces the suffix after `issuedAt` with `u64(0)||u8(1)||u64(startedAt)`. Proof input is `lp1("roady.v3.proof")||header`; proof is HMAC-SHA-256 with the server proof key and unpadded base64url.

Seed commitment is `SHA-256(lp1("roady.v3.seed")||seed32)`. Schedule bytes are:

```
lp1("roady.v3.schedule")||V3||seed32||u16BE(16)||records
record = u8(effect)||u64(telegraphStart)||u64(activeStart)
         ||u64(activeEnd)||u64(cooldownEnd)
```

The v2 PRNG algorithm, cadence, pool, anti-repeat rule, schedule/event selection, and mechanics are unchanged, but the v3 PRNG domains above intentionally produce a new schedule. Reusing v2 draws or schedule bytes is a protocol failure.

An event record is exactly `lp1("roady.v3.event")||u32(seq)||u64(activeMs)||u8(kind)||payload`. Event kind ordinals and every non-Terminal payload are exactly v2. Terminal stays event kind `7`. Terminal payloads are exactly the v2 conduct layouts, with the reason byte now permitting ordinal 3:

```
Cluck: u8(0)||u8(reason)||u32(total)||u32(chickens)||u32(coins)
       ||u8(objectiveCompleted)||u8(maxCombo)||u64(duration)
       ||u64(remaining)||lp1(build)||u8(platform)

RightOfWay: u8(1)||u8(reason)||u32(total)||i64(accumulator)
       ||u32(premium)||u32(packages)||u32(courtesy)||u32(hits)
       ||u32(maxDeliveryChain)||u8(objectiveCompleted)||u64(duration)
       ||u64(remaining)||lp1(build)||u8(platform)
```

Here `objectiveCompleted` is the u8 boolean `0` or `1`. `eventHash=SHA-256(previousHash32||eventRecord)`, with `h0=SHA-256(startedHeader)`. Stored records are `eventRecord||eventHash32`.

Evidence bytes are:

```
lp1("roady.v3.evidence")||lp1(sessionId)||u32(eventCount)
||lp4(concatenatedStoredRecords)
```

`evidenceHash=SHA-256(evidenceBytes)`. `conductAggregates` is the Terminal payload without event kind. The root and score input are:

```
finalRoot = SHA-256(lp1("roady.v3.root")||h0||hN||conductAggregates)

scoreInput = lp1("roady.v3.score")||V3||lp1(sessionId)||finalRoot32
             ||scheduleHash32||seedCommitment32||conductAggregates
```

The client signature is HMAC-SHA-256 over `scoreInput` under the key selected by the accepted v3 key ID. Event count appears only in the evidence envelope. No v2 domain or omitted v3 identity string may be substituted.

### 5.3 Required seed01 and canonical goldens

Before implementation is accepted, immutable Rust-generated JSON/schema/golden artifacts and TypeScript `BigInt` encoders MUST publish and byte-assert the complete schedule bytes, all maximum event variants, both session states/proofs, both terminal conducts for all three reasons, evidence, roots, and score HMACs. Rust native, game WASM, Worker TypeScript/BigInt, Worker WASM, and workerd MUST agree.

For seed01 `0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20`, the required anchors `(FNV,state)` are:

```
rotation          f38f47eb336d159f f9206d8135e006ba
scheduled_events  51fa35922273f6fc ca4c289e7f958983
frenzy.interval    bef5f50e87dc0d73 59c3bc2e951471ee
frenzy.roll        49e4a30aea7376e5 6e4617a88010657c
frenzy.kind        af786247a3f4322c 30a497535ccbedc5
frenzy.position    b6f8ae67223b3f91 de1c6a56c9a52307
frenzy.relocation  6af4bc34a20deff6 2a650ffd231c7922
```

The first four rotation draws are `d3125a31889536c3`, `bc71303b840acaab`, `bf60e20d2bfe1df0`, `5b79dab05e8f257a`; the 16 effects are `[GlassCannon,RushHour,Stampede,GlassCannon,RushHour,Stampede,GlassCannon,RushHour,Stampede,RushHour,Stampede,GlassCannon,RushHour,GlassCannon,Stampede,RushHour]`. Scheduled-event draws are `459f2c53c9469695`, `fd66d1e067ba96ea`, yielding `[ChickenBurst,ChickenBurst]`. Seed commitment is `80ee4d608c35a33c20eb6b6dea7dc4004e5a0e3a8c6f5fa6b8d941d900aaafc7`. Schedule hashes are Cluck `39fd9de32608adbd69430dd239e2605d75c88fa68cf5a5d66e9cafe0197696f4` and RightOfWay `ee734cdc4d4625c2eb78bbbe4aedffc302263df1fa8567a1bb2743d2b25b7f30`.

The minimal Drowned goldens use session `S03`, challenge `C03`, issued=1000, expiry=301000, started=2000, build `dev`, platform Web=1, duration=60000, remaining=5000, one Terminal at activeMs=60000/seq=0, proof key `roady-v3-test-proof-key`, and client key `roady-v3-test-client-key`:

| Conduct aggregate | h0 | terminal hash | evidence hash | final root | started proof | score HMAC |
|---|---|---|---|---|---|---|
| Cluck: reason3,total42,chickens35,coins7,objective1,maxCombo5 | `4dcfa380fd1e55c2831dfe99efd72ffb37cf775f1d58aa7ea7d68e92882f1199` | `5d98d8d94ceb1f6bebd568fda59406cb7cc6b72c7f728d9b1e71bdb3c63c6e1f` | `47fa4af8218fc508a65efb353761e2c71ba6aa2969aca7e70b941eccf556b015` | `482c533be51bed3a9af2c6f021a0d964c689ee771d840eb596e4ae21ef475733` | `NyZ1kR40f8_pio7_AQT847YhLR1a8wVu5bd8A3lPO4A` | `-8lpNLggQe3aAroCoV7Lofys06iEz8fKxxvnwmVa4v4` |
| RightOfWay: reason3,total17,acc17,premium9000,packages3,courtesy2,hits1,maxChain3,objective1 | `94c4515a0d6693bb78a2d399cb1395970f068f71092d0eb2afa267392a8ac0fb` | `60cfbddd727ee008bc9a68d0bca2325e9416e8d3936df8386308f95e0a06755b` | `f16c78796a7251216fcd0aa5ab147b7f0d948774971500b45cc7a373eb6a7343` | `364437e7b200fad0955cc691eb343a2fbeb3877df78063867fc15eca75f57d3c` | `ru25O1jfOHh4IHeeaMM2gvS2M3nzaZETdNiwaT59npU` | `zQ5UURZGPLifMlolyIfI1zLs3H7EiOtwmBTkH8DWkso` |

The exact Drowned conduct aggregates are:

```
Cluck (37 bytes):
00030000002a00000023000000070105000000000000ea6000000000000013880364657601

RightOfWay (56 bytes):
0103000000110000000000000011000023280000000300000002000000010000000301000000000000ea6000000000000013880364657601
```

The exact one-event Drowned Terminal records (before the stored event hash) are:

```
Cluck (65 bytes):
0e726f6164792e76332e6576656e7400000000000000000000ea600700030000002a00000023000000070105000000000000ea6000000000000013880364657601

RightOfWay (84 bytes):
0e726f6164792e76332e6576656e7400000000000000000000ea60070103000000110000000000000011000023280000000300000002000000010000000301000000000000ea6000000000000013880364657601
```

The corresponding unstarted proofs are Cluck `R9L4OHuu0a3X7_-XS3yC0bMLm3WCcb_opJ8qUedqTQI` and RightOfWay `FaVdudLNEmUUk4caWRWA2V_JwKTEH3tLjRehamXbDfU`. These anchors are necessary but not a substitute for checking in complete canonical-byte hex fixtures.

## 6. Strict v3 API and preserved defenses

The `/v3` JSON APIs are the `/v2` APIs in v2 contract section 12 with these exhaustive substitutions: route prefix `/v3`; exact v3 tuple/IDs/categories from section 2; signature key `v3.client.1` at initial launch; `gameOverReason` additionally accepts `drowned`; and all canonical operations use section 5. Unknown fields remain rejected. `/v3/session` request stays exactly `{mode,categoryKey,turnstileToken}`; its response also MUST include exact `protocolVersion`, `protocolId`, `rulesVersion`, `rulesId`, `policyVersion`, and `policyId` fields so a client cannot start on an implicit tuple.

The following protections MUST carry forward without weakening:

- Turnstile action `roady_score_session`, allowed-origin hostname check, and rejection of the always-pass secret outside dev.
- Server proof HMAC and encrypted 32-byte seed storage; plaintext seed is returned only in the TLS session response.
- Atomic single-statement unstarted-to-started and started-unused-to-used claims; URL/body session equality; one-time score use and replay rejection.
- Exact tuple, category, rules, schedule, seed commitment, proof, and conduct binding at session, score, evidence, restoration, rank, and board boundaries.
- A five-minute start window (`issuedAt + 300000`), but **no completion TTL after start**. Cleanup MUST NOT remove/reject a started unused session because of age.
- Nuisance client-HMAC rotation by overlapping old/new accepted IDs, switching clients only after Worker acceptance, observing non-sensitive telemetry, then retiring the old ID after the supported window.
- Evidence capability bound to score ID, session ID, final root, and 24-hour expiry, with only its hash stored; exact-byte idempotency and conflict quarantine.
- New ranked scores enter `pending`; only complete successful replay makes them `live`; mismatch becomes `quarantined`; missing evidence after 24 hours becomes `unranked_missing_evidence`.
- Existing origin, privacy/IP hashing, retention, request-size, strict-JSON, CORS, error-envelope, moderation, deterministic ranking, and rate-limit rules. Exact 60-second limits remain reads 30, session 3, start/score/evidence 5, and rank 60, with write bindings fail-closed.

Drowned evidence MUST replay the complete run, including final objective-before-Terminal order. The backend MUST reject an otherwise valid ordinal 3 if the JSON reason, Terminal payload, root, aggregate, category, or GameOver-derived submitted fields disagree.

## 7. Additive D1 migration relationship

The implementation migration MUST be the next unused migration number after all deployed migrations (expected `0006` when v2 is `0005`). It MUST be additive. It MUST NOT `ALTER`, drop, rename, rewrite, backfill, trigger on, index into, or reuse a v1/v2 table. It MUST create v3-only tables with `_v3` suffix: `score_categories_v3`, `sessions_v3`, `scores_v3`, `score_evidence_v3`, `admin_restorations_v3`, and `moderation_log_v3`, plus v3-only indexes. Category rows MUST be exactly the two section-2 rows bound to rules version 3 and rules ID `roady-rules.v3`.

The v3 schema MUST carry explicit protocol/rules/policy numeric and ID columns and enforce the exact tuple. `scores_v3.game_over_reason` MUST check exactly `('time_up','wrecked','drowned')`. Conduct-specific NULL/check invariants remain those of v2. Foreign keys MUST stay within v3 tables. Migration application MUST be tested against (a) empty D1 through all migrations and (b) a populated v1/v2 snapshot, proving pre/post hashes and query bytes for frozen boards are unchanged.

D1 atomicity claims remain limited to `D1Database.batch` plus the two single conditional `UPDATE` claims. No unsupported cross-statement serializable guarantee may be asserted. The seed remains encrypted and usable; the commitment remains separate.

## 8. Mode matrix, Casual, and persistence

| Product | Competition | Conduct | Ranked category | Condition source | Submission |
|---|---|---|---|---|---|
| Ranked Cluck Hunt | ranked | cluck_hunt | `rotation.v2.cluck_hunt` | forced v3 rotation | required v3 flow |
| Ranked Right of Way | ranked | right_of_way | `rotation.v2.right_of_way` | forced v3 rotation | required v3 flow |
| Casual Cluck Hunt | casual | cluck_hunt | none | manual ID 0--4 | prohibited |
| Casual Right of Way | casual | right_of_way | none | manual ID 0--4 | prohibited |

Casual manual IDs remain exactly Standard=0, RushHour=1, ChickenFrenzy=2, Stampede=3, GlassCannon=4, with the v2 contract's conduct composition. No ID 5 may be invented. Casual MUST NOT call any `/v1`, `/v2`, or `/v3` session, start, score, evidence, or rank route; MUST NOT hold a ranked proof/capability; MUST mark records unranked; and MUST be rejected server-side if presented as Ranked. Drowned Casual runs remain local and never submit.

The exact v3 local namespaces are:

- `roady.v3.best.ranked.rotation.v2.cluck_hunt`
- `roady.v3.best.ranked.rotation.v2.right_of_way`
- `roady.v3.best.casual.cluck_hunt.{condition_id}`
- `roady.v3.best.casual.right_of_way.{condition_id}`

`condition_id` is decimal `0`--`4`. A namespace may read/write only itself. Existing `car_game_best` and every `roady.v2.best.*` key remain untouched. Only completed terminal totals, including Drowned totals, may update the matching namespace; in-progress peaks may not.

## 9. Rollout and production verification

Ranked MUST fail closed until all are true:

1. v1/v2 baseline inventory passes and no frozen byte changed.
2. Reviewed immutable v3 Rust JSON/schema/goldens exist; TS uses `BigInt` for every u64/i64 and matches Rust, WASM, Worker WASM, and workerd byte-for-byte.
3. TimeUp=1, Wrecked=2, Drowned=3 tests cover both conducts, objective reward ordering, same-ms terminal races, one Terminal, snapshot ordering, and evidence replay.
4. Additive migration tests prove v1/v2 data and route bytes unchanged and v3 board/category isolation.
5. Turnstile, proof, encrypted seed, atomic claims, no started-session TTL, HMAC rotation, rate limits, replay, evidence capability, moderation transitions, and fail-closed config tests pass without weakened assertions.
6. Browser desktop/touch/settings/request-failure tests prove all four cells work, Casual never submits, and Ranked unlocks only on the exact capability match.
7. Release artifact is under 25 MiB; all game/rules tests, WASM builds, Worker tests/typecheck/workerd tests, migrations, browser audits, and deployment checks pass.
8. Production migration and Worker deployment are verified before Pages/client rollout. With the flag absent/false, production reports `enabled:false`. After setting exact `true`, two independent uncached probes observe the exact tuple/categories/cache header; `/v2/capabilities` remains byte/semantically unchanged; v3 session/start/score/evidence smoke produces pending then live in the correct board; replay and cross-category attempts reject; Casual produces zero network submissions.
9. Monitoring shows zero unexplained native/WASM/Worker aggregate/root mismatch. Any failed probe, parity drift, migration uncertainty, or missing deployment credential leaves Ranked disabled; it is not waived.

Rollback clears `ROADY_V3_RANKED_ENABLED` (or sets it non-`true`) first, verifies new issuance stops, preserves started-session completion/evidence service and all data, and never routes v3 traffic through v2.

## 10. Baseline hash inventory procedure

Before the normative implementation commit and again in CI/release:

1. Check out baseline `ddf6745c76f6eb450378e8acc74c489ef1b56b04` in a clean worktree.
2. Enumerate tracked frozen files with `git ls-tree -r --name-only`, selecting all v1/v2 rules JSON/schema/golden artifacts; Rust/TS golden and route fixtures; migrations 0001--0005 that exist at the comparison baseline; and public-board snapshots/manifests under `docs/data/leaderboard-v1/`. Include any subsequently approved v2 fixture by explicit path. Do not use filesystem discovery that includes untracked/build output.
3. For each path, record Git blob ID, byte count, and SHA-256 of raw bytes. Sort by UTF-8 path byte order. Write a generated inventory outside frozen directories (for implementation, `docs/frozen-v1-v2-baseline.sha256.json`) containing baseline commit, generator version, path, size, blob, sha256. Review the path list manually against this section.
4. In the candidate worktree, require exactly the same path set and recompute raw hashes without newline normalization, JSON parsing/reformatting, or Git autocrlf conversion (`git show HEAD:path` is authoritative). Missing, added-to-frozen-set, or mismatched entries fail CI.
5. Separately run route fixtures against populated v1/v2 data and hash status, selected headers, and raw response body; run migration pre/post snapshots and public-board manifest verification. Dynamic request IDs/timestamps MUST use existing deterministic fixtures, never exclusions added to make a failure pass.
6. Store the command/tool version and CI log as release evidence. An intentional v1/v2 byte change is not approvable through this addendum; it requires stopping and a separate compatibility decision.

Minimum baseline paths presently known are `rules/roady-rules.v1.json`, `rules/roady-rules.v1.schema.json`, `rules/roady-rules.v2.json`, `rules/roady-rules.v2.schema.json`, `rules/roady-rules.v2.golden.json`, `crates/roady-score-rules/tests/golden.rs`, `crates/roady-score-rules/tests/golden_v2.rs`, `leaderboard/migrations/0001_init.sql` through `0004_admin_restorations.sql`, `leaderboard/test/*` route/golden fixtures, and all tracked `docs/data/leaderboard-v1/**/*.json` plus its README. The generated inventory, not this illustrative minimum, is authoritative after review.

## 11. Review and sign-off checklist

Each item requires reviewer initials and date before implementation. Final
release review was completed by the Roady implementation/review pipeline on
2026-07-17; evidence: CI `29603237483`, disabled production deployment
`29606647978`, and its `worker-production-evidence-*` artifact.

- [x] 2026-07-17 RY — Route/version decision: v2 remains exact and v3 is additive only.
- [x] 2026-07-17 RY — Exact numeric tuple, protocol/rules/policy IDs, mode, categories, board isolation, and route list.
- [x] 2026-07-17 RY — Drowned ordinal 3 and JSON `drowned`; TimeUp=1 and Wrecked=2 unchanged; eligible in both Ranked conducts.
- [x] 2026-07-17 RY — Objective reward -> Terminal -> root -> GameOver snapshot ordering and same-ms race behavior.
- [x] 2026-07-17 RY — v3 domains, PRNG domains, layouts, limits, complete-golden requirement, and published seed01 anchors.
- [x] 2026-07-17 RY — Strict API/capability JSON, cache behavior, exact-match UI gate, and `ROADY_V3_RANKED_ENABLED` fail-closed semantics.
- [x] 2026-07-17 RY — Turnstile, proof HMAC, encrypted seed, atomic start/use, tuple/category binding, key rotation, rate/replay defenses, evidence capability, moderation, and no completion TTL preserved.
- [x] 2026-07-17 RY — Additive D1 migration and complete v1/v2 table/board isolation.
- [x] 2026-07-17 RY — Four-cell matrix, manual IDs 0--4, Casual absolute no-submission, and persistence namespaces.
- [x] 2026-07-17 RY — Baseline hash inventory path set/procedure and zero frozen-byte diff.
- [x] 2026-07-17 RY — Test, deployment, production probes, rollback, release-size gate, and disabled-on-incomplete rule.
- [x] 2026-07-17 RY — Scope was documentation-only in its bounded contract commit; implementation landed separately.

**Required sign-offs:** gameplay/rules owner; protocol/canonical owner; Worker/security owner; D1/data owner; client/UI owner; release/operations owner. After all sign-offs, land this document in one bounded documentation-only commit. Implementation and generated v3 artifacts belong in separately reviewable commits.
