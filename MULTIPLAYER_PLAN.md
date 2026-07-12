# Optional Multiplayer Extension Plan

**Status:** Optional design plan — no networking implementation committed  
**Date:** 2026-07-12  
**Target:** Bevy 0.19, Rust 1.95, `wasm32-unknown-unknown`, WebGL2, `web-sys` 0.3.103

Multiplayer is an optional extension. Offline single-player remains the zero-configuration default and must continue to build and run without a server. Networking should be added only after small, useful single-player refactors establish clean seams; this plan does not justify a speculative rewrite of the game.

## 1. Recommended First Mode

The first real-time mode should be a **2–4 player, 60-second race-to-score in a shared city**. It matches the existing round structure: players collect coins and hit chickens while avoiding obstacles and harmful critters. When the authoritative timer reaches zero, the highest score wins. Existing combo and power-up systems can remain per-player initially.

Optional later modes:

- **Asynchronous ghosts and leaderboards:** considerably cheaper than live networking and useful even if real-time multiplayer is deferred.
- **Co-op shared score:** viable later, but multiple cars require redesigning spawn and recycling logic that currently follows one car.
- **Spectating and replay:** compatible with snapshots, but not a first-release requirement.

Large public lobbies, persistent worlds, host migration, MMO features, and ranked cross-platform ladders are explicitly out of scope.

## 2. Current Architecture and Blockers

Useful foundations already exist:

- Plugins separate cars, world generation, traffic, pickups, critters, UI, audio, and effects.
- Road and block layouts are deterministic functions of grid coordinates.
- Gameplay messages decouple several collision outcomes from presentation systems.
- `GameConfig` already centralizes important tuning values.

The current code is nevertheless single-player-specific:

- `spawn_car` creates exactly one car at startup.
- Many systems use `car.single()` or `car.single_mut()`.
- Keyboard input is read directly inside movement, body-roll, and brake-light systems.
- Camera and minimap logic assume one local car.
- Chickens, critters, traffic, and pickups recycle relative to one car rather than a set of active players.
- `Score`, `TimeLeft`, health, combo state, and game-over reason are global resources rather than player- or match-owned replicated state.
- Several procedural systems use local runtime seeds instead of a shared session/round seed.
- Traffic movement is separate simplistic logic; the project does **not** currently have a reusable AI controller for the player car. Disconnect-to-bot handoff is therefore a later feature, not free infrastructure.

## 3. Phase 0: Lightweight Seams, No Netcode

These changes should be independently valuable and preserve current behavior:

1. **`Player` and identity components.** Add a local `Player` marker and stable `NetId(u32)`/`NetEntity(u64)` identity separate from Bevy `Entity`. The current player defaults to ID 0.
2. **`PlayerInput` resource/component.** One `read_local_input` system converts keyboard state into `{ throttle, steer, brake }`. Movement, body roll, wheels, and brake lights consume this data instead of reading keys independently.
3. **`NetMode` resource.** Add `SinglePlayer` as the default, with future `Client` and `Server` variants. Do not scatter networking branches through unrelated systems yet.
4. **Shared session seeds.** Introduce `SessionSeed` and a round index so procedural spawn sequences can be reproduced when required. Block geometry remains coordinate-derived.
5. **Round ownership.** Add a `RoundEntity` marker for transient gameplay entities and establish one cleanup contract.
6. **Parameterized car spawning.** Replace the hard-coded single origin spawn with configuration accepting player ID, ownership, and spawn transform while still spawning exactly one local car by default.
7. **Authoritative event application.** Gradually centralize score/timer/health mutations into systems that consume explicit gameplay messages. Avoid a large all-at-once rewrite.

Do **not** introduce a transport trait, replication dependency, fixed-point math, or a pure whole-game `(state, input) -> state` architecture during Phase 0. Snapshot reconciliation does not require bit-perfect cross-platform lockstep.

## 4. Authority and Simulation Model

Use a **dedicated authoritative native server**. Browser clients are untrusted and only submit bounded input. The server owns:

- Canonical car movement and obstacle collision outcomes
- Scores, health, combos, power-up state, and the round timer
- Gameplay entity spawn/despawn decisions
- Round start/end and final results

Reject peer-to-peer/listen-server authority for the initial version: browser NAT limitations, cheating, host advantage, and host migration are unnecessary complexity.

Reject deterministic lockstep. Bevy entity/system iteration and native-versus-WASM floating point are not guaranteed bit-identical. Instead use:

- A fixed authoritative server simulation rate, initially 20–30 Hz
- Server snapshots at roughly 10–20 Hz
- Client prediction only for the owned car
- Server reconciliation with tolerances, e.g. about 0.3 world units and 5 degrees before visible correction
- Buffered interpolation for remote cars and dynamic entities, rendering about 100 ms behind

The client stores recent `(tick, PlayerInput, predicted car state)` entries. When an authoritative snapshot arrives, it accepts small error, or rewinds the owned car to the acknowledged state and replays later inputs. Server collision push-outs always win. Wheel spin, body roll, brake lights, particles, camera shake, and audio remain local presentation derived from replicated state/events.

## 5. Replicated State

Every networked gameplay object receives a server-assigned `NetEntity`; never transmit raw Bevy `Entity` values.

Replicate:

- Cars: owner, transform, heading, speed, health, active power-ups
- Match: round ID, timer, phase, session seed
- Per-player score and combo summary
- Chicken, critter, traffic, pickup, and coin spawn/despawn deltas
- Discrete confirmed events such as hit, collect, damage, and round end

Do not replicate purely visual particles, tire marks, mesh children, audio players, camera transforms, or UI nodes.

### Multi-car world recycling

Current recycling is anchored to one car. Multiplayer must define interest around all active players. A practical first rule is:

- Keep a block/entity while it lies within the keep radius of **any** player.
- Retire it only when outside the radius of **all** players.
- The server chooses spawns using the union of player interest regions and sends stable IDs/deltas.
- Clients may visually cull by their local camera, but may not independently decide authoritative despawns.

Widely separated players increase the active world substantially; matches may therefore need a maximum separation rule or bounded arena for v1.

## 6. Transport and Protocol

Use **WebSocket for v1** because browsers support it reliably through TLS proxies. Consider WebTransport later if measured latency or head-of-line blocking justifies the operational complexity.

The native server can use Tokio plus a WebSocket library; the WASM client should use browser WebSocket APIs or a dependency confirmed compatible with Bevy 0.19 and the existing `web-sys = 0.3.103` pin. Evaluate Lightyear or Replicon only when their Bevy 0.19 and browser support is verified; do not couple Phase 0 to them.

Use a compact, versioned binary schema such as `postcard` over Serde, with explicit protocol and client-build versions. Core messages:

- C→S `Hello { protocol, build, display_name }`
- C→S `InputTick { tick, throttle_i8, steer_i8, brake }`
- C→S `Ready`, `Leave`, `Ping`
- S→C `Welcome { player_id, room, tick }`
- S→C `LobbyState { players, ready }`
- S→C `RoundStart { round_id, session_seed, duration }`
- S→C `Snapshot { tick, cars, scores, time_left, deltas }`
- S→C `Spawn`, `Despawn`, and confirmed gameplay events
- S→C `RoundEnd { reason, final_scores }`, `Pong`, and versioned errors

Validate message sizes, enum tags, input ranges, sequence numbers, and maximum receive rates.

## 7. Lobby, Security, and Deployment

Add optional `Connecting` and `Lobby` states without removing Menu, Playing, Paused, or GameOver. Menu continues to launch single-player immediately; Multiplayer connects to a room service, joins a 2–4 player lobby, performs a ready check, then waits for authoritative `RoundStart`.

Baseline anti-cheat follows from server authority. The server validates tick windows and input ranges, rate-limits messages, clamps movement through `GameConfig`, rejects incompatible builds, and never accepts client-authored scores or positions.

The server is a separate native/headless binary or feature-gated target using Bevy `MinimalPlugins` plus fixed scheduling. Rendering and asset-only plugins must be separable from simulation. Deploy it in a container behind TLS termination (`wss://`). A simple HTTP room service can return a room token and WebSocket URL. One process may host multiple small rooms after profiling; one room per process is acceptable for a prototype.

Server persistence is optional for v1. Keep local single-player best scores unchanged. Store match results or accounts only after the live mode proves worthwhile.

## 8. Testing Strategy

- Unit-test protocol round trips and rejection of malformed/oversized messages.
- Test movement from identical input sequences within tolerances, not bit-perfect native/WASM checksums.
- Run a headless authoritative app plus two simulated clients through an in-memory transport.
- Inject latency, jitter, duplication, reordering, and disconnects; verify interpolation and reconciliation converge.
- Assert scores, timers, health, and spawn/despawn sets converge to server state.
- Add two-browser-tab Playwright/manual smoke tests against a local server.
- Continue the existing single-player WASM driving audit for every networking change.
- Soak-test long rounds and widely separated players for entity/memory growth.

## 9. Phased Roadmap and Gates

1. **Phase 0 — single-player seams:** input resource, player identity, session seed, round ownership, parameterized car spawn. Gate: all current tests, WASM build, and browser audit pass with networking absent.
2. **Phase 1 — local replication prototype:** native headless server, binary messages, one browser client receiving authoritative snapshots and reconciling its car. **Go/no-go:** stable 10-minute local session, correction rate and magnitude acceptable, no regression to offline single-player, and server/client architecture does not require duplicating gameplay logic.
3. **Phase 2 — two-player competitive:** room code, lobby, authoritative 60-second timer/scoring, two browser tabs/LAN clients, N-car interest rules.
4. **Phase 3 — public 2–4 player release:** TLS deployment, reconnect/forfeit policy, monitoring, abuse limits, WAN testing.
5. **Optional later:** WebTransport, ghosts/leaderboards, replay/spectating, accounts, co-op, or purpose-built bot controllers.

Highest risks are Bevy 0.19 networking dependency compatibility, separating headless simulation from rendering, multiple-player recycling cost, collision reconciliation feel, and uncontrolled multiplayer scope. Phase 1 should not begin until Phase 0 ships cleanly as ordinary single-player and a deliberate multiplayer kickoff is approved.
