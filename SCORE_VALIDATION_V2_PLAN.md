# Roady Score Validation v2 Plan

## Goals

- Never reject a legitimate run because a remaining-clock cap was mistaken for an elapsed-round cap.
- Validate against the exact shipped condition, event, combo, objective, coin, pickup, and penalty rules.
- Keep Turnstile, one-time sessions, proof/HMAC, rate limits, origin checks, and replay protection strict.
- Share deterministic mechanics between the game, native tests, game WASM, and Worker validation.
- Improve moderation evidence with a bounded event ledger without claiming client telemetry is authoritative.

## Validation layers

1. **Hard schema/protocol checks**
   - bounded body and strings;
   - exact integer types and safe ranges;
   - known rules/condition/event/terminal IDs;
   - aggregate arithmetic, e.g. total equals score buckets;
   - canonical encoding is unambiguous.
2. **Authentication and replay checks**
   - exact allowed origin;
   - Turnstile action/hostname;
   - signed Worker proof;
   - client nuisance HMAC;
   - condition-bound, unexpired, unused session;
   - atomic one-time claim;
   - per-IP rate limits.
3. **Rules-version event replay**
   - replay bounded score/time events through the shared Rust rules core;
   - require resulting aggregate score/time/objective state to match submission.
4. **Plausibility moderation**
   - duration over 30 minutes;
   - near provisional condition cap;
   - high peak combo with low terminal total;
   - exceptional rates/timing gaps;
   - internally consistent exceptional runs remain live but flagged.
5. **Hard policy rejects**
   - only impossible arithmetic/schema/auth/replay or extreme protocol/storage-safety violations;
   - provisional score caps should migrate toward soft moderation unless a real mathematical bound exists.

## Shared Rust rules core

Workspace crate:

```text
crates/roady-score-rules
```

It has no Bevy, Worker, D1, WebCrypto, clock, random, or rendering dependency. It owns:

- stable rules version;
- condition/event IDs;
- combo window, thresholds, cap, and integer bonus formula;
- direct chicken awards by condition/event;
- ordinary coin score/time transition;
- MegaCoin score and one-event semantics;
- Time pickup transition;
- objective targets/reward;
- critter score penalty;
- terminal aggregate reconciliation;
- explicit checked/saturating arithmetic semantics.

A generated and byte-compared interoperability artifact lives at:

```text
rules/roady-rules.v1.json
```

Empirical deployment policy—rate thresholds, moderation thresholds, and provisional score caps—is versioned separately from immutable mechanics.

## Worker WASM adapter

Follow-up crate:

```text
crates/roady-score-rules-wasm
```

Compile to `wasm32-unknown-unknown` with `wasm-bindgen`. Expose a narrow synchronous ABI:

```text
rules_abi_version() -> u32
validate_round_record_json(payload, policy) -> result JSON
canonical_score_bytes_json(validated) -> bytes
replay_event_ledger_json(record) -> result JSON
```

The TypeScript Worker keeps HTTP, CORS, Turnstile, D1, WebCrypto/HMAC, rate limits, rankings, and moderation. Instantiate the small rules WASM once at module scope. Use shared golden fixtures against native Rust, generated WASM, and workerd/Vitest before switching enforcement.

## Versioned round record

```text
RoundRecord v2
- protocol_version
- rules_version
- policy_version
- session/proof/name
- condition
- terminal aggregate score buckets
- objective kind/target/completed/reward
- peak combo
- active elapsed milliseconds
- remaining milliseconds
- terminal reason
- build/platform
- event ledger
- final ledger hash
```

`chickens` and `coins` must be documented as score-source buckets, not hit/collection counts. Objective bonus goes into the chicken bucket; MegaCoin adds five coin points but one combo/objective event.

## Bounded score event ledger

Each event uses a monotonically non-decreasing active-play timestamp and sequence number:

```text
ScoreEvent
- seq
- active_ms
- kind
- context: condition, active run event, combo before/after
- score buckets before/after
- remaining_ms before/after
- event-specific fields
- previous_hash
- event_hash
```

Initial variants:

- `ChickenHit`
  - base points;
  - condition direct bonus;
  - event direct bonus;
  - combo bonus/factors;
  - resulting chicken bucket.
- `CoinCollected`
  - ordinary vs MegaCoin;
  - base points;
  - one combo step/bonus;
  - ordinary +1.5s clock transition when applicable.
- `TimePickup`
  - +5s and resulting remaining time.
- `ObjectiveCompleted`
  - kind, target, one-time +10 reward.
- `CritterPenalty`
  - saturating -2 chicken bucket and cooldown evidence.
- `RunEventChanged`
  - planned event identity/window.
- `Terminal`
  - reason, final score/time/health summary.

Use integer milliseconds and points only in the ledger. Cap event count and encoded bytes. A compact canonical binary/length-prefixed format is preferable for signing; JSON may be used for debugging and early compatibility.

Hash chaining detects accidental truncation/reordering after signing, not fabrication by an attacker. The client, game WASM, and embedded nuisance HMAC are public. A fabricated but internally consistent ledger remains possible. The ledger improves:

- arithmetic consistency;
- rules-version validation;
- moderation evidence;
- debugging denied legitimate scores;
- future migration toward authoritative servers.

It does not replace server-side simulation.

## Timing and leeway

- Remaining time hard maximum follows shipped mechanics: 99,000 ms plus a small serialization tolerance if necessary.
- Elapsed play has no mechanics-derived finite ceiling because streamed coins and repeated Time pickups can replenish the clock.
- 30 minutes is a soft review threshold only.
- Hard elapsed maximum is JavaScript safe-integer/protocol exactness.
- Timestamp ordering allows bounded frame quantization and same-frame reorder tolerance.
- Validation compares before/after transitions with integer tolerances, not an assumed human hit-rate ceiling.
- Rate plausibility is a moderation signal, never a sole hard reject.

## MODZ incident

Screenshot-proven fields:

```text
name MODZ
condition ChickenFrenzy (2)
terminal total 1614
chicken score bucket 1179
coin score bucket 435
objective completed: Reach combo 3/3, +10
terminal reason: wrecked
```

The screenshot does not prove duration, remaining time, peak combo beyond at least 3, session, proof, IP attribution, build, or platform. Production D1 has no rejected payload/audit row and no matching orphan session. An administrative restoration must mark every inferred/synthetic field, include the screenshot SHA-256, use a synthetic `admin_restore:` session, and write an audit record. It must never be represented as a normal Turnstile-verified player submission.

## Migration sequence

1. Immediate: duration and combo plausibility become moderation reasons; strict protocol/auth/replay remains.
2. Add authenticated, idempotent administrative restoration records.
3. Land pure Rust rules crate and generated manifest; migrate game scoring helpers.
4. Add WASM adapter and TS/Rust/workerd golden parity tests.
5. Add RoundRecord v2 and event ledger behind a versioned client feature.
6. Shadow-validate ledgers without enforcement; compare against aggregate validator.
7. Enable rules-aware enforcement only after telemetry proves tolerance boundaries.
8. Eventually use an authoritative gameplay server for truly trusted competitive scores.
