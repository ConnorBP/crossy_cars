# Roady Condition Rotation Plan

## Status

Design only. This plan does not change gameplay, scoring, persistence, or leaderboard behavior yet.

It incorporates independent gameplay and scoring/security reviews. The existing classic game and protocol remain the compatibility baseline until the segmented mode passes every rollout gate below.

## Why refactor

Today a single `ModifierKind` applies for an entire round:

| Legacy ID | Player-facing name | Whole-round effect |
|---:|---|---|
| 0 | Standard | Neutral baseline |
| 1 | Rush Hour | 2x traffic population and 1.35x traffic speed |
| 2 | Chicken Frenzy | About 2.5x chickens and +1 direct point per chicken |
| 3 | Stampede | 2x critter population |
| 4 | Glass Cannon | 2x damage and 2x combo bonus |

This creates long stretches with one dominant flavor and makes Chicken Frenzy disproportionately valuable. Roady already has two temporary eight-second run-event windows, so the safer evolution is a separately versioned mode with rotating effects and a rare, short Chicken Frenzy pickup.

## Product decision

### Preserve classic mode

- IDs `0–4`, their stored meaning, v1 canonical bytes, classic local bests, and classic leaderboards never change.
- Existing scores are never relabeled or mixed with new segmented scores.
- Classic remains available through a published compatibility period.

### Add a separate rotation mode

- Stable category key: `rotation.v1`.
- Rotation runs have no fabricated legacy condition ID.
- Rotation receives its own local record namespace, leaderboard, medals, rules version, protocol version, and validation policy.
- New gameplay effects layer over a Standard baseline; they do not mutate the legacy `ActiveModifier` identity during a run.

## Gameplay schedule

All boundaries use integer active-play milliseconds derived from Roady's active-play clock. Countdown, pause, and other input-frozen time do not advance this clock.

| Phase | Duration |
|---|---:|
| Initial grace | 8 seconds |
| Telegraph | 3 seconds |
| Active rotating effect | 18 seconds |
| Cooldown | 7 seconds |
| Full cadence | 28 seconds |

The first windows are:

- telegraph `[8,000, 11,000)`; active `[11,000, 29,000)`;
- telegraph `[36,000, 39,000)`; active `[39,000, 57,000)`;
- telegraph `[64,000, 67,000)`; active `[67,000, 85,000)`;
- continue by adding 28,000 ms.

Adding remaining time exposes later occurrences but never stretches, shifts, restarts, or skips existing phase boundaries.

### Rotation pool

The serialized pool is:

1. Rush Hour;
2. Stampede;
3. Glass Cannon.

Standard is the neutral baseline. Chicken Frenzy is not in the rotation pool.

### Selection

- Ranked web runs receive a random seed in a Worker-issued, signed pre-play session receipt.
- The receipt binds the exact protocol, rules, policy, mode, category key, seed commitment, schedule hash, issuance time, and start-by expiry.
- Session validity is two-phase: an unstarted receipt must be started within five minutes; the atomic start claim removes the start expiry and marks the session `started`. A started session has no wall-clock completion deadline because Roady has no mechanics-derived maximum run duration. It remains one-time, condition/category-bound, rate-limited, and replay-protected. Cleanup may remove explicitly abandoned unstarted sessions, but must never reject a completed run solely because play outlived a TTL.
- `roady-score-rules` owns the deterministic PRNG and schedule function.
- Consecutive identical effects are prohibited: perform one deterministic re-draw, then use the pool's cyclic successor if the re-draw still repeats.
- Native/offline play uses a deterministic local fallback seed and remains unranked unless it has a valid pre-play receipt.
- A restart always obtains a new receipt and seed. A pause resume never re-arms the schedule.

A Worker-issued seed prevents terminal-time seed selection. It does not make the client authoritative: a modified client can still fabricate internally consistent gameplay evidence.

## Existing run events

The existing eight-second events remain a separate layer:

- Traffic Surge;
- Chicken Burst;
- Combo Frenzy;
- Critter Burst.

Their initial windows stay `[15,000, 23,000)` and `[40,000, 48,000)`.

At round setup, derive both the rotation and event plans from the same signed seed. Exclude an event whose flavor duplicates the rotating effect active at the event's start:

- Rush Hour excludes Traffic Surge;
- Stampede excludes Critter Burst;
- Glass Cannon excludes Combo Frenzy.

Cross-flavor overlaps remain allowed.

## Effect composition and caps

Apply effects per axis. The base score point is never multiplied.

| Axis | Composition | Exact cap |
|---|---|---:|
| Traffic count | baseline × Rush Hour × Traffic Surge | 16 vehicles |
| Traffic speed | base speed × roll × Rush Hour × Traffic Surge | 11.5 units/s |
| Chicken count | 14 × Chicken Burst × Chicken Frenzy | 40 chickens |
| Chicken direct bonus | event bonus + Frenzy bonus | checked/saturating per rules path |
| Combo bonus | `(combo-1)` × Glass Cannon × Combo Frenzy | existing max combo 5 |
| Damage | base × Glass Cannon | existing health rules |
| Critter count | baseline × Stampede × Critter Burst | 16 critters |

Only one rotating effect, one scheduled run event, and one Chicken Frenzy activation may be active. Existing orthogonal timed pickups may coexist.

## Population reconciliation

Population changes must be based on active-play time, not rendered frames.

Each population owner keeps fixed-point spawn and retirement budgets:

- add `12 entities/active-second` to the spawn budget while below target;
- add `18 entities/active-second` to the retirement budget while above target;
- consume the integer part and retain the fractional remainder;
- do not advance either budget during countdown, pause, or other input-frozen time;
- consume RNG only for actual deterministic spawn attempts.

This must produce the same target transition and draw count at 30, 60, and 120 FPS in golden tests.

### Start behavior

- Keep existing entities.
- Recompute traffic speed from each vehicle's stored immutable speed roll.
- Spawn deficits on valid roads ahead of the camera using the budget.
- Do not replace the entire population.

### End behavior

Use one policy for every surplus:

1. mark surplus effect-created entities `RetirePending`;
2. select candidates by: outside the camera safety region first, then behind the car, then greatest distance, then ascending entity bits;
3. retire only eligible candidates using the 18-per-active-second budget;
4. pending entities that are currently visible remain until eligible; there is no unsupported promise that all extras disappear within a fixed number of seconds;
5. if ordinary baseline entities are still surplus after tagged extras, apply the same ordering and budget.

Effect-created extras are counted in the live target and never simultaneously governed by a conflicting natural-drain path.

## Chicken Frenzy pickup

Chicken Frenzy becomes a rare round-scoped activation rather than a whole-round scoring condition.

### Exact rebalance

| Property | Classic ID 2 | Rotation pickup |
|---|---:|---:|
| Chicken target | about 35 | 28 |
| Population multiplier | about 2.5x | exactly 2x |
| Direct bonus | +1 | +1 |
| Combo bonus multiplier | none | none |
| Telegraph | none | 2 seconds |
| Active duration | whole round | 15 active-play seconds |
| With Chicken Burst | unbounded by this design | target capped at 40 |

Frenzy's only scoring boost is its +1 direct chicken award. It never multiplies combo bonuses.

### Spawn policy

The current pickup `SpawnState` is a persistent system local and is not fresh-round seeded. Rotation mode therefore introduces a separate round-scoped seeded pickup schedule; it must not assume the existing local is reset.

- eligible after 8,000 active-play ms;
- multiple eligible probability checks may occur, but at most one Frenzy orb may spawn per round;
- the first seeded pickup opportunity occurs exactly at `8,000 + (interval_draw % 4,001)` active-play ms; each subsequent opportunity time equals the previous opportunity time plus `8,000 + (next_interval_draw % 4,001)` ms;
- each eligible check uses the Frenzy-roll stream and succeeds exactly when `roll_draw % 10,000 < 400` (4%);
- if no Frenzy orb has spawned by 55,000 ms, the next eligible check forces it;
- spawning the orb consumes the one-per-round spawn allowance, whether it is later collected or expires;
- orb lifetime: 12,000 active-play ms;
- collection uses the existing 1.2-unit pickup radius;
- collection starts a 2,000 ms telegraph, then 15,000 ms active Frenzy;
- pause freezes eligibility, lifetime, telegraph, and active duration.

Use independent deterministic PRNG domains for interval, Frenzy roll, ordinary kind, spawn position, and relocation. Each stream advances only for its named decision, so unrelated RNG consumers cannot perturb it.

At orb age 6,000 ms, relocation is evaluated once. `approached` means the car has been within 20.0 XZ units of the orb at any prior active-play tick. `invalid` means `nearest_road_segment(orb_xz, 2)` is absent or its closest point is more than 4.0 units away. `unreachable` means the orb has not been approached and is currently more than 45.0 XZ units from the car. When either predicate is true, derive exactly eight ordered candidates from the relocation stream using a fixed draw count. Select the first candidate within 4.0 units of a finite road segment and outside spawn-exclusion geometry. If none qualifies, keep the original orb position. Never use unbounded retries or data-dependent extra draws.

Existing Speed Boost and Coin Magnet timers retain their shipped behavior in the first implementation wave. They must not be described as using `Difficulty.elapsed`; any future clock unification is a separate tested change.

## Objectives

Rotation mode uses neutral fixed targets independent of transient effects:

- hit 10 chickens;
- collect 6 coins;
- reach combo 3.

The mission remains one round-wide objective with one +10 award. Classic condition-flavored objective behavior remains unchanged.

## HUD and accessibility

Do not add independently positioned panels with guessed coordinates.

- Extend the existing event/status panel into one unified rules-status panel.
- It may show two compact rows: rotating effect and scheduled event.
- Chicken Frenzy uses the existing power-up/status presentation region rather than overlapping the objective strip.
- Keep the existing objective, combo, minimap, health, touch, and event layout constants authoritative.
- Add the expanded unified panel to the existing 844×390, 960×480, and 1440×900 overlap audits before visual implementation.
- Telegraphs use name, ASCII signature, countdown, and static segmented bar; color is supplementary.
- Reduced motion removes pulsing/flashing but retains text, brackets, countdown, and bar state.

Suggested signatures:

- `>> TRAFFIC` for Rush Hour;
- `** CRITTERS` for Stampede;
- `!! GLASS` for Glass Cannon;
- `<> FRENZY` for Chicken Frenzy.

## Pause, restart, and terminal ordering

- Schedule, active effects, seeded pickup state, budgets, and ledger sequence survive pause/resume unchanged.
- Resume emits no duplicate segment-start or activation event.
- Fresh restart clears all rotation state and requests a new one-time session.
- The terminal ledger event is appended after final objective processing/reward and before the GameOver snapshot.
- Terminal is always the last ledger event.

## Protocol and event ledger

### Independent versions

Track these separately:

- `protocol_version`: envelope, canonical bytes, routes, and ledger encoding;
- `rules_version`: mechanics and deterministic schedule generation;
- `policy_version`: moderation thresholds and enforcement state.

The Worker accepts an explicit allowlist of complete tuples. Unknown protocol, rules, policy, mode, or category values are hard rejected.

### Routes

Keep `/v1/*` unchanged. Add separate `/v2/session`, `/v2/scores`, and `/v2/evidence` routes so body limits and canonical encoders are selected before parsing.

### V2 storage

Prefer additive, separate v2 tables rather than forcing segmented runs into the v1 `condition NOT NULL CHECK 0..4` schema:

- `score_categories(category_key, rules_version, display_name, active)`;
- `sessions_v2` bound to version tuple, `rotation.v1`, seed commitment, proof, expiry, and one-time use;
- `scores_v2` with category key, aggregates, schedule/root metadata, validation state, deterministic ranking fields, and submission source;
- evidence storage keyed by score/session and ledger root.

Backfill nothing as `pass`. Existing v1 rows retain their historical semantics; if exposed through a unified administrative view their validation state is `legacy_unverified`.

Classic and rotation leaderboards remain separate. Deterministic ordering within each remains score descending, submission time ascending, ID ascending.

### Canonical chain

Use one length-prefixed binary encoding with cross-language golden fixtures. Do not modify v1 LF bytes.

- `h0 = SHA-256(domain-separated canonical signed round/session header)`;
- `h(i+1) = SHA-256(domain || h(i) || canonical(event_i))`;
- the final event is `Terminal`;
- `final_root = SHA-256(domain || h0 || event_count || hN || terminal aggregates)`;
- the score HMAC commits to `final_root`, schedule hash, version tuple, category key, and all terminal aggregates.

Every string length and integer width must be frozen by fixtures before implementation. Session IDs must either be capped at 255 bytes or use at least a two-byte length; do not leave that ambiguous.

### Same-millisecond ordering

For half-open windows `[start,end)`, assign sequence numbers in this order:

1. activation/combo expirations;
2. segment ends;
3. segment starts;
4. activation spawns;
5. activation collections;
6. activation activations;
7. gameplay transitions ordered by fixed kind ordinal then stable entity bits;
8. terminal.

Thus an event at `end_ms` occurs after the old segment ends, while one at `start_ms` occurs after the new segment starts.

### Bounds

Provisional bounds to validate with real canonical fixtures before freezing:

- 4,096 ledger events;
- 256 KiB canonical ledger bytes;
- 128 bytes per canonical event;
- 16 segments;
- 32 activation records;
- 16 KiB score envelope;
- 512 KiB evidence upload.

These numbers and the string-length prefix width are not protocol commitments yet. In particular, 128 bytes may not fit events carrying complete before/after state. Freeze them only after Rust, TypeScript, and WASM fixtures prove every maximum-size variant fits with headroom.

The hot score submission carries the root and counts, not the full ledger. Evidence uploads separately.

## Validation boundary

The ledger is client-generated consistency evidence, not authority.

### Shadow phase

- Accept structurally valid v2 scores into an isolated unranked/shadow dataset.
- Full evidence is sampled or requested for moderation; it is not required for every shadow score.
- Ledger absence, truncation, root mismatch, replay mismatch, and timing anomalies are moderation/telemetry flags.
- Never block a legitimate run while cross-language parity and lifecycle coverage are still being measured.

### Enforced ranked phase

Only after native Rust, game WASM, Worker WASM/workerd, and browser lifecycle fixtures demonstrate parity:

- every ranked rotation score is inserted as `pending`, never directly `live`;
- that response issues a random one-score evidence capability bound to score ID, session ID, ledger root, and a 24-hour expiry; D1 stores only its hash;
- the client uploads the complete bounded ledger immediately after the score envelope and may retry for 24 hours from terminal submission using that capability, not the already-used round session;
- evidence upload is idempotent for an identical score/root/evidence hash and atomically rejects conflicting reuse;
- successful evidence replay transitions `pending` to `live`;
- missing evidence after 24 hours becomes `unranked_missing_evidence`; it is retained for diagnostics and is not described as an impossible gameplay run;
- evidence/root disagreement becomes quarantined with an exact consistency reason.

Hard reject or quarantine:

- malformed/oversized canonical data;
- unknown version tuple/category/event;
- invalid proof/HMAC/session, expiry, replay, or seed/schedule binding;
- impossible integer ranges or `total != chickens + coins`;
- missing required clean Terminal;
- ledger/root/replay disagreement once complete evidence is required for ranked admission.

Keep soft:

- long duration;
- exceptional score rate;
- near/over provisional caps;
- exceptional pickup frequency;
- high combo with low terminal total;
- other internally consistent plausibility anomalies.

A v2 score should remain pending until required evidence replays, then become live. Evidence mismatch transitions it to quarantined rather than rewriting history.

## HMAC/key rollout

Never replace an embedded nuisance key in a single step.

1. Worker accepts old and new key IDs.
2. Pages/game client switches to the new key ID.
3. Observe adoption and failures through non-sensitive telemetry.
4. Retire the old key only after the supported old-client window closes.

Session proof and one-time session controls remain the stronger server-held layers. Embedded client HMAC remains nuisance-only.

## Migration stages

1. **Freeze classic v1**
   - byte-lock current score/session fixtures;
   - keep classic UI, storage, and boards unchanged.

2. **Shared rules v2**
   - deterministic schedule and PRNG domains;
   - integer-ms transitions;
   - exact effect composition/caps;
   - canonical event enums/encoding/hash fixtures.

3. **Local shadow ledger**
   - record classic rounds without network enforcement;
   - validate pause, restart, terminal, objective, pickup, and boundary order.

4. **V2 infrastructure**
   - separate routes/tables/category registry;
   - pre-play Worker sessions and signed seeds;
   - exact compatibility allowlist;
   - no ranked v2 writes yet.

5. **Opt-in rotation without Frenzy**
   - schedule, reconciliation, unified HUD, separate unranked board;
   - compare score distribution, crashes, abandonment, and lifecycle parity.

6. **Frenzy activation pickup**
   - round-scoped deterministic pickup schedule;
   - exact 2×/+1/15-second rules;
   - verify max-one opportunity and pity boundaries.

7. **Evidence upload and replay**
   - moderation-gated/sample evidence first;
   - cross-language replay and root comparison;
   - fix all false mismatches without weakening assertions.

8. **Ranked rotation board**
   - require clean terminal and successful replay;
   - pending → live only after evidence validation;
   - classic remains separate.

9. **Default consideration**
   - only after at least two stable production weeks and explicit review of fairness, score distributions, abandonment, mobile layout, moderation load, and legitimate rejection rate.

## Required tests and gates

- exact 8/3/18/7 phase boundaries;
- signed seed and schedule parity across native/WASM/Worker;
- anti-repeat selector vectors;
- 30/60/120 FPS population budget equivalence;
- pause/resume emits no duplicate transitions;
- restart uses a new session and clears prior state;
- same-ms ordering at every segment boundary;
- objective reward precedes Terminal;
- Frenzy 4% check, 55-second pity, one opportunity, expiry, relocation, and stacking cap;
- no same-flavor rotation/event overlap;
- classic v1 responses remain byte-identical with v2 data present;
- category/board isolation and deterministic ties;
- canonical Rust/TS/WASM hashes and replay outputs;
- 844×390, 960×480, and 1440×900 unified-panel layout audits;
- browser errors, shader errors, request failures, and reduced-motion behavior;
- shadow mode must show zero unexplained aggregate/root mismatch before ranked enforcement.

## Rejected alternatives

- **Keep whole-round conditions:** does not solve the requested pacing problem.
- **Only randomize the two existing events:** too little variety and leaves the dominant whole-round modifier intact.
- **Rotate Chicken Frenzy normally:** makes score inflation routine rather than a rare highlight.
- **Allow multiple simultaneous rotating conditions:** produces unreadable and potentially unsafe multiplicative stacks.
- **Assign mixed runs to their final/dominant legacy condition:** misleading and makes rankings depend on run cutoff.
- **Add legacy condition ID 5:** breaks the stable 0–4 contract and forces v2 into a v1 schema that cannot represent it honestly.
- **Mix classic and rotation rankings:** scores come from different opportunity distributions.
- **Use client/day/process seed for ranked runs:** permits seed shopping and cannot be verified by the Worker.
- **Hard-enforce client ledgers immediately:** risks repeating the MODZ false-rejection failure before parity is proven.

## Approval required before implementation

This proposal changes pre-play networking and creates a new competitive category. Implementation should begin only after explicit approval of:

- the exact 8/3/18/7 cadence;
- the 2×/+1/15-second Frenzy rebalance;
- 4% opportunity chance, 55-second pity, and one opportunity per round;
- pre-play Turnstile/session behavior and offline/unranked fallback;
- separate rotation rankings;
- pending-until-replay ranked admission;
- the staged rollout and classic compatibility period.
