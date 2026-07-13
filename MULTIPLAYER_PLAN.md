# Roady Car Online Mode Architecture

**Status:** optional implementation plan; no online gameplay code is implemented or approved for kickoff yet.
**Updated:** 2026-07-13
**Target:** Bevy 0.19, Rust 1.95, `wasm32-unknown-unknown`, WebGL2, `web-sys` 0.3.103. Reconfirm these pins at kickoff.
**Primary constraint:** single-player remains the zero-configuration default and must stay fully functional when every network feature is absent or disabled.

A deliberate multiplayer kickoff is required before M0/M1 work begins. The kickoff must name an owner, budget, hosting target and success/stop criteria; this plan does not justify a speculative rewrite or adding networking dependencies to ordinary single-player.

## 1. Goals and non-goals

### Goals

- Optional 2-player, 60-second-baseline competitive shared-world score attack, with a path to 4 players after explicit scale gates.
- Native and WebAssembly clients.
- Server-authoritative driving, collisions, claims, health, score, objectives, match time, events, and results.
- Cloudflare-hosted lobby/control plane informed by Ghost protocol-v3 lifecycle reference patterns.
- Reconnect, version rejection, bounded abuse controls, independent rollout, and rollback.
- Early source refactors that improve ownership and determinism without requiring a server.

### Non-goals for the first release

- Replacing or delaying the current offline Enter/Space/tap flow.
- Peer authority, listen servers, host migration, or Ghost's WebRTC/GGRS gameplay data plane.
- Deterministic lockstep across native and WASM.
- Dynamic fleet allocation before usage justifies it.
- Bot takeover, accounts/OAuth, voice chat, spectators, replay, or online scores on the existing single-player leaderboard.
- Large public lobbies, persistent worlds, MMO features, ranked cross-platform ladders, and mandatory server persistence.

If live multiplayer fails its dependency, hosting, cost, or feel gates, asynchronous ghosts plus a separate authoritative leaderboard are the preferred cheaper fallback. Snapshot-based replay/spectating can be considered later; neither should expand the first live release.

## 2. User experience and state flow

The current Menu action remains unchanged:

```text
Enter / Space / ordinary Menu tap -> offline single-player
```

A separate visible `MULTIPLAYER` action opens an optional flow:

```text
Menu
  -> MultiplayerMenu
  -> Connecting
  -> Lobby { create | join code }
  -> Ready
  -> OnlineCountdown
  -> OnlinePlaying
  -> OnlineResults
  -> Lobby or Menu
```

The lobby shows room code, 2-player roster, ready state, version errors, latency, reconnect status, and Leave. Settings remain available before Ready. Online Pause cannot stop the authoritative match; it becomes a local overlay with Resume and Leave confirmation.

Touch layouts use the current responsive breakpoints and must fit 844x390 and 960x480. The local-player camera and HUD remain primary; remote players have distinct non-color identity markers and minimap shapes.

## 3. Architecture decision: split control plane and gameplay plane

### Cloudflare Worker plus Durable Object

Owns:

- exact-origin admission filtering;
- room configuration;
- player identity and reconnect-token rotation;
- roster, profile, presence, ready state, queued joiners;
- immutable active epoch/round lifecycle;
- alarms and reconnect expiry;
- authoritative-server endpoint selection;
- short-lived signed match-ticket issuance;
- control WebSocket status/errors.

It does **not** run per-frame gameplay or relay snapshots.

### Dedicated native Roady server

Owns:

- fixed-tick car simulation;
- static/dynamic world state;
- traffic, creatures, pickups, collisions, damage and cooldowns;
- score, combo, power-ups, objectives, match clock and events;
- authoritative claims and results;
- snapshots, acknowledgements and reconnect full state.

### Client

Owns:

- keyboard/touch input collection;
- local prediction and reconciliation;
- remote interpolation;
- camera, UI, audio, particles and presentation;
- immutable terrain regeneration after seed/version agreement.

This preserves a clean trust boundary: clients submit input, never position, damage, score, pickups, objective completion, or results as authority. Peer-to-peer/listen-server authority is rejected for v1 because browser NAT/connectivity constraints, cheating exposure, host advantage and host-migration failure modes add complexity without helping the two-player mode.

## 4. Ghost protocol-v3 evidence and reuse boundary

Historical snapshot: the prior networking review recorded the Ghost `ghost-network-lifecycle` worktree at commit `1db3942442cd95cb8765b99451087ce0cbdcc99c`. At that snapshot, `npm test` reproduced **44/45 passing**, with `selection requires both profile and ready` failing. Treat this as historical evidence, not current/proven green status. A deliberate Roady kickoff must pin the intended Ghost revision, confirm worktree cleanliness, rerun its complete Worker suite, and resolve or explicitly supersede every red test before reducer code is shared.

The historical review identified these Ghost paths at that revision:

- `cloudflare-worker/src/index.js`
- `cloudflare-worker/src/protocol.js`
- `cloudflare-worker/src/epoch-lobby.js`
- `cloudflare-worker/src/epoch-state.js`
- `cloudflare-worker/vendor/cloudflare-game-common/lifecycle.js`
- `docs/lobby-v3.md`

The snapshot supports an architectural reference, not a claim that current Ghost code is proven, green, or directly reusable. At kickoff, record the current repository/revision and clean-worktree state, re-read the files and migration history, and rerun the lifecycle tests. A red or unavailable suite blocks reducer code reuse but not independent use of the documented concepts.

Reuse the **concepts and intentionally versioned shared artifacts**, not repository imports:

- persistent control WebSocket;
- immutable active epoch and round;
- fixed room configuration;
- incumbent roster continuity and queued mid-round joiners;
- ready/profile flow;
- reconnect identity with hashed, rotating bearer token;
- superseded-socket closure;
- hibernating WebSocket attachments;
- Durable Object storage and alarm expiry;
- epoch-scoped messages;
- idempotent terminal decisions as a reference pattern, subject to Roady-specific authenticated result rules.

Do not copy:

- WebRTC signaling as Roady's gameplay transport;
- GGRS rollback session construction;
- Ghost duel/deathmatch mode labels;
- Ghost outcome/score rules;
- Ghost palette/profile schema;
- seed-derived match identity;
- Ghost room/storage key naming.

The historical review indicates that Ghost's lifecycle reducer was not exported by the shared npm package at the reviewed point; re-verify this at the pinned revision. If Roady adopts reducer code, first make it an explicit versioned shared artifact or a clearly versioned Roady vendor module with provenance and parity tests. Never import source from the Ghost repository.

## 5. Shared Cloudflare package boundary

Canonical source:

```text
@segfault-site/cloudflare-game-common@0.1.0
E:/DEVELOPER/PROJECTS/audit-worktrees/cloudflare-game-common
```

Roady standalone CI imports through:

```text
leaderboard/vendor/cloudflare-game-common/src/index.ts
```

Do not import a sibling workspace and do not assume npm publication; the package is currently local/unpublished.

Every new matchmaking Worker input uses the shared primitives for:

- `parseExactOrigins` / `isExactOriginAllowed`;
- UTF-8 byte bounds and `boundedString`;
- bounded JSON and plain-object validation;
- SHA-256, canonical base64url, cryptographic random IDs/tokens;
- fail-closed Cloudflare rate limiting.

Before matchmaking implementation, synchronize Roady's adapter with package 0.1.0 and add its missing exports:

- `BoundedJsonError`;
- `parseBoundedJson`;
- `readBoundedJsonObject`;
- exported bounded-string option type;
- exported rate-limit logger type.

Package/adapter parity tests are mandatory. Availability policy remains explicit at the endpoint boundary; it must not weaken the fail-closed shared primitive.

## 6. MVP server allocation and tickets

Do not build a dynamic allocator first. Configure one static TLS WebSocket authoritative endpoint per environment/region.

When the roster is full and ready, the Durable Object:

1. selects the configured server endpoint;
2. creates `match_id`, epoch, round, seed and expiry;
3. issues one short-lived signed ticket per player;
4. broadcasts `start { endpoint, ticket, match metadata }`.

Ticket fields:

```text
protocol_version
build_version
ruleset_version
topology_version
match_id
room_id
epoch
round
player_id
ordered_roster
session_seed
server_endpoint / audience
issued_at
expires_at
nonce
```

The Worker and native server share a dedicated ticket-signing key. The server rejects bad audience, expiry, signature, roster, version, nonce replay, and duplicate live identity. Ticket key rotation and overlap policy must be documented before preview.

Dynamic regional allocation can later replace endpoint selection without changing the client gameplay protocol.

## 7. Authority, rates, prediction and interpolation

Initial targets:

- authoritative simulation: 30 Hz;
- client input send: 30 Hz;
- snapshots: 15-20 Hz;
- rendering/interpolation: display rate;
- bounded remote extrapolation followed by hold/resync.

Input frame:

```text
match_id
player_id
client_tick
sequence
last_server_tick_seen
throttle [-1,1]
steer [-1,1]
brake bool
handbrake bool
```

The owned car predicts with the shared car-motion kernel and stores input history. Snapshots acknowledge input sequence. Initial tuning thresholds are approximately **0.3 world units** of position error and **5 degrees** of heading error: accept/smooth smaller error, and rewind to authoritative state plus replay unacknowledged inputs above threshold. These are starting hypotheses, not protocol constants; M2/M4 fault tests must tune and record them. Static collision may be predicted, but server collision/damage wins. Presentation effects respond to confirmed event IDs and may use harmless predicted previews.

Remote cars and dynamic entities initially render from an approximately **100 ms** interpolation buffer. Extrapolation is short and bounded, then holds or requests resync; it never creates authoritative interactions.

Lockstep is rejected because current native/WASM floating point, ECS/query ordering, streamed dynamic entities, and several system-local RNG streams are not a proven cross-platform deterministic simulation contract.

## 8. World seed and topology synchronization

Introduce:

```rust
SessionSeed(u128)
TopologyVersion(u32)
RulesetVersion(u32)
RoundId
NetEntityId(u64)
```

Use domain-separated deterministic streams, e.g. topology, urban props, rural props, pickups, traffic, chickens, critters, modifiers and run events. Never derive authoritative randomness from ECS entity IDs, query iteration order, or process-local counters.

Clients regenerate immutable terrain from `(SessionSeed, TopologyVersion, coordinate)`. Server snapshots carry dynamic entities only, plus sampled topology hashes/manifests for mismatch detection. Reject incompatible topology/build/ruleset before play.

Do not freeze the current infinite-line road topology as multiplayer v1. The planned edge-segment/center-arm road rewrite and top-down render validation should land before `TopologyVersion = 1` is declared stable.

For multiple players, the exact interest invariant is:

- retain a block/entity while it is within the authoritative keep radius of **any** active player;
- retire it only while it is outside that radius for **all** active players;
- choose spawns from the server's bounded union of player interest regions, using stable IDs;
- allow clients to cull visuals locally, but never to authoritatively spawn/despawn.

The union must be bounded by a measured entity/memory cap plus either maximum player separation or an explicit regroup policy. The separation value and cap remain pre-preview decisions; silently violating the invariant under load is not allowed.

## 9. Online gameplay semantics

Initial mode: **2-player competitive shared-world race-to-score**, starting from a 60-second authoritative clock. Players score through versioned rules based on the existing coin/chicken loop while avoiding obstacles and harmful critters; the highest server-confirmed score wins. Any time-extension behavior and tie result must be fixed in the versioned ruleset before M3. Co-op shared score remains a later mode, not an unresolved MVP presentation choice. Authority rules:

- Match clock, road condition and scheduled events are shared.
- Score, health, combo, power-ups and personal objective progress are per-player.
- Server decides simultaneous pickup/target claims by authoritative tick, then stable `PlayerId` tie-break.
- Chicken/critter/traffic events carry actor and stable event/entity IDs.
- Car-to-world and car-to-traffic collisions are authoritative.
- Car-to-car collisions start non-damaging or tightly impulse-bounded to limit griefing.
- Coin/time extensions are applied by server rules only.
- Confirmed events are idempotent by stable event ID.
- Online results are excluded from the current single-player leaderboard until a separate authoritative ruleset/board is designed.

## 10. Current foundations, blockers and required refactors

Useful foundations already present in the historical/current source assessment:

- plugins separate cars, world generation, traffic, pickups, critters, UI, audio and effects;
- road/block layouts are coordinate-derived;
- gameplay messages already separate several outcomes from presentation;
- `GameConfig` centralizes important tuning;
- keyboard/touch collection is already centralized into global `PlayerInput` and `Handbrake` state.

Do not regress that input seam or repeat the stale claim that movement, body-roll and brake-light systems still read keys independently. The remaining input work is to make buffers player-addressed and tick-addressed, with keyboard/touch writing only the local player's buffer.

Current blockers are the one-car spawn and `single()` assumptions; local-only camera/HUD/minimap ownership; global score/time/health/combo/game-over state; one-car world recycling; process-local random streams; mixed simulation/presentation scheduling; and broad cleanup ownership. Traffic has separate simplistic movement, not a reusable player-car AI controller, so disconnect-to-bot is not free infrastructure.

| Current area | Required seam | Offline compatibility |
|---|---|---|
| `src/main.rs`, plugin tuple | reusable client/server app construction | existing client is default |
| `game/state.rs` | optional multiplayer/lobby states | offline state flow unchanged |
| global mode assumptions | `NetMode::{SinglePlayer, OnlineClient, AuthoritativeServer}` | defaults to SinglePlayer |
| `car.rs` single car queries | stable `PlayerId`, `LocalPlayer`, parameterized car spawn, per-player input | spawn one local player offline |
| global `PlayerInput`, `Handbrake` | player-addressed input buffers | keyboard/touch write local player only |
| gameplay messages | actor-bearing messages with stable entity/event IDs; gradually centralize authoritative score/timer/health mutation through their consumers | offline actor is local player and existing values remain exact |
| `Score`, `Health`, `Combo`, power-ups/objective | per-player component/resource records | offline accessors expose local record |
| `TimeLeft`, condition, events | match-owned state | same values offline |
| `world.rs` one-car recycling | server interest union / separation policy | current one-player window offline |
| mixed simulation/presentation systems | fixed authoritative simulation sets and client-only presentation sets | both installed in offline client |
| cleanup via broad state hooks | explicit `RoundEntity` (or equivalent match-ownership) marker and one match-owned cleanup contract | existing round lifecycle preserved |

Refactor one seam at a time with the existing test and browser gates green. The historical **Phase 0**, now M1, explicitly prohibits a transport trait, replication/network dependency, fixed-point conversion, or pure whole-game `(state, input) -> state` rewrite. Do not introduce transport abstractions before multi-car-safe ownership exists; snapshot reconciliation does not require bit-perfect lockstep.

## 11. Control protocol

Control plane uses strict bounded JSON schemas over WebSocket. Version 1 messages include:

Client:

```text
profile
ready
leave
ping
```

Server:

```text
welcome { player_id, reconnect_token, room config }
status { roster, ready, presence, queue }
presence
profile_accepted
start { epoch, round, match_id, seed, endpoint, ticket, versions }
round_commit / round_abort
pong
error
```

Room codes are identifiers, not passwords. Exact Origin filters browsers but is not authentication.

Clients can request leave but can never submit `round_commit`, `round_abort`, or final results. Only the assigned authoritative server may call the Durable Object's authenticated server-only terminal endpoint. The DO verifies server credential/signature, `match_id`, room, epoch, round, assignment and idempotency before atomically committing the authoritative result or abort reason, then broadcasts `round_commit`/`round_abort` to clients. Invalid, stale, conflicting or client-originated terminal requests are rejected and audited without sensitive fields.

## 12. Replication, transport and gameplay protocol

Every networked gameplay object has a server-assigned `NetEntityId`; raw Bevy `Entity` values never cross the wire.

Explicitly replicate:

- cars: player owner, transform, heading, speed, health and active power-ups;
- match: identity, round, phase, authoritative time, seed and version tuple;
- per-player score, combo and objective summary;
- traffic, creatures, pickups and coins as stable spawn/despawn/state deltas;
- stable, idempotent confirmed hit/collect/damage/claim and round events.

Explicitly exclude purely visual particles, tire marks, mesh children, audio players, camera transforms, UI nodes, screen shake and other presentation-only state.

Use WebSocket for v1 because native and browsers can operate it through common TLS termination; expose gameplay only as `wss://` outside local development. WebTransport remains a measured later option because its deployment complexity is not justified yet. The native server may use Tokio plus a WebSocket crate, while WASM must use browser APIs or a crate verified against Bevy 0.19 and the pinned `web-sys`. Lightyear/Replicon are evaluation candidates only after native, WASM and Bevy-version support is proven; neither belongs in Phase 0.

Use a versioned binary gameplay protocol:

```text
ClientHello / ServerHello
InputBatch
FullSnapshot
DeltaSnapshot
Spawn / Despawn
ConfirmedEvent
InputAck
Ping / Pong
ResyncRequest
RoundResult
Error / Disconnect
```

`postcard` over Serde is an initial codec candidate, not an assumed dependency: first spike round trips, malformed-enum behavior, allocation limits, schema evolution and native/WASM compatibility. Pin the chosen codec and document compatibility rules and golden vectors. Every frame includes protocol/build/match identity and sequence/tick as applicable. Deltas name their baseline; a missing baseline requests a full snapshot. Stale/duplicate input and confirmed events are rejected idempotently. Apply explicit payload, batch, tick-window, enum-tag and message-rate limits. Start uncompressed; add compression only after measurements and bounded decompression limits. Head-of-line blocking, crate compatibility and codec evolution remain measured risks, not reasons to weaken authority.

## 13. Reconnect and failure behavior

- Web reconnect credential: session storage; native: memory or OS-appropriate local secure storage.
- Prefer delivering reconnect credentials in bounded protocol messages rather than URLs. If browser WebSocket reconnection requires a query credential, it must be short-lived and single-use; Worker/proxy/access-log configuration must drop or redact the full query string, observability must never record the URL, and a deployment test must prove the policy before preview.
- Token rotates on accepted reconnect; superseded socket closes.
- Grace target: approximately 30 seconds.
- Reconnect obtains a fresh gameplay ticket and full snapshot.
- No host migration because clients are never hosts.
- No initial bot takeover.
- During grace the car uses a documented safe policy (coast/brake and no scoring input).
- After expiry, mark disconnected/forfeit according to mode; replacement may enter only a later epoch.
- Server loss yields a clear terminal error and Menu return; never silently accepts client results.

## 14. Security, abuse and observability

- Validate ticket, identity, versions and input sequence before simulation.
- Enforce per-IP upgrade, per-room socket, per-player message, payload, idle and room-creation limits.
- Never log IPs, raw rate-limit keys, bearer tokens, reconnect query values or URLs containing them, tickets, or signing material.
- Exact Origin is filtering, not proof of client identity.
- Detect impossible input rate/value/tick windows server-side.
- Score/collision farming detection uses authoritative telemetry only.
- Metrics: connections, room creation, ready latency, ticket rejects, reconnect success, RTT/loss, reconciliation magnitude, snapshot bytes, simulation overruns, disconnect reason, entity count, match completion.

## 15. Deployment and rollback

Keep four independent products:

1. Cloudflare Pages game client;
2. leaderboard Worker/D1;
3. matchmaking Worker/Durable Object;
4. native authoritative Roady server.

Matchmaking workflow gates:

- vendored-adapter/package parity tests;
- typecheck and unit/protocol/lifecycle tests;
- Wrangler dry-run;
- deployment;
- WebSocket origin/rate/schema/start-ticket smoke tests;
- explicit Worker version rollback procedure.

Server workflow gates:

- a separate native/headless app using Bevy `MinimalPlugins` plus fixed scheduling, with rendering and asset-only plugins excluded;
- pinned Rust artifact/container behind TLS termination (`wss://`);
- protocol tests;
- convergence/fault/soak tests;
- health/readiness endpoint;
- staged deployment and previous-image rollback.

One process may host multiple isolated rooms only after profiling proves tick fairness, room caps and failure isolation; one room per process is acceptable for the prototype. Server persistence is optional for v1: keep single-player best scores unchanged and do not store accounts/results until a separate authoritative board is approved.

Every matchmaking deployment that adds or changes a Durable Object class or storage schema must include explicit Wrangler migration declarations, schema-version handling, clean-create and upgrade-from-previous tests, rollback/forward-fix analysis, and preview validation that alarms, hibernating attachments, active epochs and reconnect expiry survive as intended. Never assume code rollback alone reverses a DO storage migration.

Feature flags:

- hide Multiplayer UI;
- disable matchmaking globally;
- disable new rooms while allowing active matches to finish;
- restrict preview room prefix/build allowlist.

## 16. Milestones and go/no-go gates

### M0 - shared Cloudflare boundary

- Record deliberate kickoff approval, pinned Ghost evidence revision, target-version recheck, owners and stop criteria.
- Synchronize Roady's vendored adapter with package 0.1.0.
- Add missing JSON/type exports and parity tests.
- Decide/version the lifecycle shared artifact.

**Gate:** package, adapter, leaderboard Worker tests/typecheck/dry-run green in standalone checkout.

### M1 - offline ownership seams

- Add `NetMode`, IDs, local ownership, actor events and multi-car-safe queries.
- Parameterize car spawn and separate per-player/match state.

**Gate:** no transport dependency; every existing offline Rust/browser/release test green.

### M2 - local authoritative loopback and early browser transport spike

- Extract fixed simulation kernel and headless native server.
- Connect one then two in-memory/loopback simulated clients.
- Before transport architecture is locked, connect one WASM/browser client to the loopback server using the selected browser WebSocket API and candidate binary codec. This early spike must test the pinned `web-sys` boundary, TLS/local-development shape, codec vectors and malformed-frame behavior rather than deferring browser risk to M5.

**Gate:** stable 10-minute local session; authoritative convergence; exact convergence of scores, timers, health and authoritative spawn/despawn sets; measured correction rate/magnitude within recorded tolerances; bounded entity growth; no duplicated gameplay logic; identical offline feel within tolerances; and one browser client completing the loopback protocol spike.

### M3 - Cloudflare lobby and static server ticket

- Add Roady protocol-v1 Durable Object and one configured server endpoint.
- Signed tickets and two-player roster/ready flow.

**Gate:** origin/schema/rate/ticket/replay/reconnect tests; authenticated server-only commit/abort tests; DO clean-create, migration and rollback/forward-fix validation; no public UI yet.

### M4 - two native clients

- Prediction, reconciliation, interpolation and authoritative gameplay semantics.

**Gate:** latency/loss/reordering fault matrix and long soak pass.

### M5 - native plus WASM

- Browser WebSocket client, touch lobby/gameplay UX and reconnect.

**Gate:** native/native, native/WASM and two-browser-tab tests pass; topology hashes agree.

### M6 - opt-in two-player preview

- Feature-flagged production preview with observability and rollback.

**Gate:** completion/reconnect/crash/entity/latency metrics within written thresholds.

### M7 - four-player expansion

Only after separation, world-interest, entity-growth, bandwidth, abuse and 4-client soak gates pass.

### M8 - dynamic allocation

Only if measured usage/cost/region demand justifies replacing static endpoint selection.

## 17. Test matrix

Mandatory new tests:

- shared-package/adapter API and behavior parity;
- exact origin, UTF-8, bounded JSON/plain objects, crypto and fail-closed rates;
- lifecycle roster/epoch/queue/expiry/token rotation;
- ticket expiry, audience, tampering, replay and key rotation;
- malformed, duplicate, stale and oversized protocol frames plus codec golden-vector/schema-evolution tests;
- explicit replication include/exclude checks and rejection of raw Bevy entity IDs;
- missing delta baseline/full resync;
- seed/topology hash agreement over coordinate samples;
- stable entity/event IDs and idempotent confirmed events;
- simultaneous claims and collision tie order;
- movement from identical input sequences within stated tolerances, never a native/WASM bit-perfect checksum gate;
- an in-memory authoritative server plus simulated clients;
- exact convergence of scores, timers, health and authoritative spawn/despawn sets;
- server car-state convergence under latency, jitter, loss, duplication and reordering;
- prediction rewind/replay and remote interpolation/extrapolation;
- reconnect full snapshot, credential-query log redaction and grace expiry;
- authenticated server-only DO commit/abort, conflicting terminal decisions and client-result rejection;
- DO clean-create, upgrade migration, alarm/attachment restoration and rollback/forward-fix drills;
- player separation, the any-player/all-players interest invariant, entity caps and long soak;
- native/native, native/WASM, two tabs, mobile touch;
- abuse/rate/idle/version rejection;
- server/Worker failure and rollback drills.

Existing offline gates remain mandatory:

```text
cargo fmt --all -- --check
cargo test --locked
cargo check --locked
cargo check --locked --target wasm32-unknown-unknown
trunk build --release --cargo-profile wasm-release
python tools/check_release.py --dist dist
strict runtime, desktop, touch, Settings and Turnstile browser suites
production browser audit
```

## 18. Risk register

| Risk | Required mitigation / gate |
|---|---|
| Bevy 0.19/headless separation duplicates or changes gameplay | `MinimalPlugins` loopback prototype; shared simulation systems; offline and 10-minute convergence gates |
| Native/WASM WebSocket or codec dependency incompatibility | pinned-version spike, golden vectors, malformed-input tests and two-target CI before adoption |
| WebSocket head-of-line blocking harms feel | measure RTT/loss/correction and snapshot size under WAN faults; consider WebTransport only from evidence |
| Prediction/collision correction feels poor | start near 0.3 units/5 degrees and 100 ms interpolation, then tune with recorded M2/M4 thresholds |
| Multi-player separation explodes entities, memory or bandwidth | exact interest invariant, separation/regroup rule, hard measured caps and long soak |
| Topology/RNG diverges across native and WASM | version tuple, domain-separated streams and coordinate hash tests before topology v1 |
| Ghost reference or shared package differs from assumptions | pin/reinspect revision, reproduce upstream tests, provenance/parity tests; do not repository-import |
| DO lifecycle/storage migration corrupts rooms | authenticated terminal transition tests, schema migrations, preview upgrades and rollback/forward-fix drill |
| Tickets/reconnect tokens leak through logs | rotation, short expiry, query redaction/drop policy and deployment log test |
| Hosting cost, abuse or scope grows uncontrolled | static endpoint, room/rate caps, feature kill switches, explicit budget and stop/fallback decision |

## 19. Open decisions before implementation

- Native server hosting target, static MVP region and operating budget.
- Whether lifecycle reducer becomes a new shared-package export or separately versioned vendor artifact.
- Match-ticket signing algorithm, secret storage, rotation and overlap procedure.
- Maximum permitted player separation and regroup policy.
- Safe disconnected-car behavior during grace.
- Whether online mode later receives a separate authoritative leaderboard.
- Preview capacity, metrics thresholds and rollback owner.
