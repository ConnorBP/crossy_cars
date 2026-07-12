# Roady Car — Work Restart Plan

Last updated: 2026-07-12  
Repository: `https://github.com/ConnorBP/crossy_cars`  
Branch: `master`  
Live site: `https://roady-car.pages.dev`

## Purpose

This is the durable handoff for resuming work after context compaction. It distinguishes implemented work from verified work, records everything left unfinished by the primary agent and prior swarm agents, and defines the order in which work should restart.

## Current checkpoint

Wave O was committed and pushed as:

- `e1041d2 Wave O: add settings and rebuild car running gear`

That checkpoint contains:

- A versioned persistent `Settings` resource and settings overlay
- Master-volume, mute, and reduced-motion settings
- Keyboard and touch settings interaction
- Settings-modal input gating for Menu and Paused states
- Live audio settings applied to global volume and existing/new sinks
- Reduced-motion behavior for camera shake, damage flash, combo presentation, and countdown presentation
- A connected player-car undercarriage, chassis, rockers, axles, valances, bumpers, repositioned wheels, and composite body/wheel shadow
- Pure tests for settings encoding/migration/input behavior, reduced motion, and car/shadow geometry

Validation completed before the checkpoint:

```text
cargo fmt --all -- --check                         PASS
cargo test --locked                                PASS — 175 tests
cargo check --locked                               PASS — 0 warnings
cargo check --locked --target wasm32-unknown-unknown PASS — 0 warnings
git diff --check                                   PASS
```

## Important: Wave O is implemented but not fully browser-verified

The source checkpoint is safe and compiles on both targets, but the following interactive/visual checks were deliberately left pending rather than overstated as complete:

### Settings browser QA

- Open Settings from the main menu with the keyboard
- Adjust master volume, mute, and reduced motion with keyboard controls
- Confirm the modal prevents Enter/Space/Escape from accidentally starting a round
- Reload the page and confirm settings persistence
- Open Settings while paused
- Confirm closing it returns to Pause without resuming, restarting, or returning to Menu
- Exercise settings with touch at desktop and mobile viewport sizes
- Confirm touch settings interaction does not leak into driving or state-transition controls
- Confirm master volume and mute update live audio already playing as well as newly spawned audio
- Confirm reduced motion visibly suppresses shake/flash/punch without disabling gameplay information

### Player-car visual QA

- Confirm wheels are seated within the body footprint and do not float
- Confirm chassis, axles, rockers, valances, and bumpers form one connected silhouette
- Confirm no body detail clips badly during steering, pitch, or roll
- Confirm the composite shadow matches the body and tire footprint
- Inspect the car on both desktop and mobile/WebGL2

Any browser or visual defect found here should be fixed in a small follow-up commit before moving to the next gameplay wave.

## No agents are currently active

All prior swarm work is stopped or finished. Do not assume an agent is still running. Start a fresh swarm only after the checkpoint verification below.

## Exact restart procedure

Run these commands first from `E:/DEVELOPER/PROJECTS/car_game_ai`:

```bash
git status --short
git log -5 --oneline
git pull --ff-only origin master
cargo fmt --all -- --check
cargo test --locked
cargo check --locked
cargo check --locked --target wasm32-unknown-unknown
```

Expected baseline:

- Clean working tree
- `e1041d2` and this handoff commit in history
- 175 tests passing
- Zero native and WASM warnings

Then build the browser release:

```bash
trunk clean
rm -rf dist
trunk build --release --cargo-profile wasm-release
python tools/check_release.py dist
```

On Windows, if Trunk reports a stale staging directory, run `trunk clean`, remove `dist`, and retry. Do not commit `dist`, `tools/scenarios/`, `tools/__pycache__/`, screenshots, or temporary JSON output.

Run existing browser suites against a local server or the deployed site as appropriate:

```bash
python tools/browser_audit.py
python tools/browser_scenarios.py
python tools/browser_touch_scenarios.py
```

The existing suites may not yet include explicit Settings assertions. Add a focused Settings browser scenario if manual checks cannot prove persistence and modal input isolation reliably.

After the Wave O QA/fix commit is pushed, verify:

- GitHub Actions CI passes
- Cloudflare Pages production deployment completes
- `https://roady-car.pages.dev` serves the new build
- Browser console and page-error counts remain zero

## Unfinished work inventory

### 1. Finish Wave O QA and production verification — highest priority

Status: implementation committed; browser/visual/deployment verification pending.

Acceptance criteria:

- Every Settings browser-QA item above passes
- Every car visual-QA item above passes
- Native/WASM tests and checks stay green
- Strict browser audit has no console or page errors
- Production Cloudflare Pages deployment is verified

### 2. Knockable traffic cones

Status: not implemented.

Current problem: cones behave like concrete/static obstacles.

Planned behavior:

- A car impact launches or tips a cone instead of treating it as an immovable wall
- Cone motion has bounded translation/rotation and deterministic cleanup/recovery
- Repeated impacts cannot create unbounded entities or persistent physics cost
- Collision consequences remain fair and readable
- Add pure boundary/state-transition tests where practical

Likely ownership: world/prop spawning and obstacle collision integration. Inspect callers before choosing exact files; avoid overlapping edits between workers.

### 3. Repair street-lamp models

Status: not implemented.

Current problems to verify: pole/arm/bulb hierarchy, orientation, visible gaps, and a floating bulb.

Planned behavior:

- One coherent local transform hierarchy
- Pole rooted to the ground
- Arm connected to pole
- Fixture/bulb connected and oriented toward the road
- No floating pieces or incorrect rotation after world-block parenting/recycling
- Shared cached meshes/materials; no per-frame asset allocation

Likely ownership: `src/world.rs` only, after locating the lamp-spawn helper.

### 4. Improve positive and penalty feedback

Status: not implemented.

Requested scope:

- A distinct downward-pitch penalty sound for critter/non-chicken hits
- Clear positive audio language for coins, chicken hits, objectives, and combo gains
- Clear negative audio language for damage/penalties without becoming harsh
- Score/chicken/coin HUD pop animations for increases and decreases
- Reduced-motion alternatives that preserve semantic feedback without scale/shake pulses
- Move startup instructions away from the health bar and touch controls
- General HUD/overlay hierarchy cleanup

Implementation rules:

- Reuse or procedurally generate small audio assets; document provenance
- Keep one-shot audio bounded and Settings-driven
- Do not conflate a chicken score event with critter damage
- Positive and negative changes must differ by more than color alone
- Test pure animation/formatting/state helpers
- Validate desktop and touch layouts at tight viewport sizes

### 5. Cloudflare Workers/D1 leaderboard

Status: architecture is documented; service and game client are not implemented.

Primary design document: `LEADERBOARD_ARCHITECTURE.md`.

Work still required:

1. Create separate Worker/D1 project and migrations
2. Implement cached read-only leaderboard endpoint
3. Implement Turnstile-backed session issuance
4. Issue short-lived Worker-signed session proof
5. Verify the mandatory build-injected client HMAC nuisance layer
6. Enforce one-time session use/replay protection in D1
7. Add IP/session rate limits, score plausibility checks, and moderation
8. Add 3–5 character arcade-name entry and submission UI in Roady Car
9. Add rank lookup that consumes the permitted higher rate carefully
10. Add CI/deployment workflow and production smoke tests

Required Worker/runtime secrets before production:

- `LB_SESSION_HMAC_KEY`
- `LB_CLIENT_HMAC_KEY`
- `LB_IP_HASH_PEPPER`
- `LB_ADMIN_TOKEN`
- `LB_TURNSTILE_SECRET`

The GitHub secret `ROADY_LEADERBOARD_CLIENT_HMAC_KEY` is already configured for the client build. The client key is extractable from WASM and must never be described as authentication or tamper-proof security. Turnstile, short-lived Worker proof, one-time use, replay protection, rate limiting, plausibility checks, and moderation remain mandatory.

Cloudflare deployment also needs Workers Scripts/D1 permissions and separately provisioned Worker/D1 resources.

### 6. Gameplay-loop industry/game-theory audit

Status: not started. Run after the pending gameplay and feedback waves, so the review evaluates the intended experience rather than an obsolete build.

Audit scope:

- Core arcade loop and time-to-fun
- Early/mid/late-round pacing
- Risk/reward and route choice
- Dominant or degenerate strategies
- Braking, collision, and drift incentives
- Coin-time economy and the 90-second cap
- Objectives, events, conditions, combos, medals, and record motivation
- Fairness/readability of traffic and creature spawns
- Session replayability and long-term progression
- Mobile touch ergonomics
- Leaderboard incentives, cheating pressure, and score integrity

Deliverable: a prioritized design report separating high-confidence fixes, experiments requiring telemetry/playtests, and ideas that should not be implemented without evidence.

### 7. Optional bounded arcade drift/slip

Status: deferred.

Revisit only after Settings, props, feedback, and gameplay audit. It must not destabilize current responsive controls or make touch input imprecise. Prototype behind pure tuning helpers and compare keyboard/touch behavior before adopting it.

## Recommended restart wave plan

### Wave O-QA — primary agent only initially

1. Re-establish clean baseline
2. Build release and run browser audit
3. Test Settings keyboard/touch/persistence/modal behavior
4. Capture desktop/mobile car screenshots
5. Fix only observed Wave O defects
6. Re-run 175+ tests, native/WASM checks, and browser suites
7. Commit, push, and verify production deployment

### Wave P — parallel props after Wave O-QA is stable

Spawn two non-overlapping agents:

- Agent P1: knockable cone behavior and tests; own the specific prop/collision files identified during orientation
- Agent P2: street-lamp geometry/hierarchy; preferably own `src/world.rs` if P1 can avoid it

If both tasks require `src/world.rs`, do not run them concurrently. Split by time or have one agent provide a design/report while the other edits.

Primary agent responsibilities:

- Inspect and assign exact file ownership
- Integrate sequentially
- Run all builds/tests
- Perform browser visual QA
- Commit and push the stable wave

### Wave Q — feedback and HUD

Use independent ownership where possible:

- Audio agent: penalty/positive SFX semantics and bounded playback
- HUD agent: score/coin/chicken animations and layout
- Accessibility reviewer: reduced-motion and non-color semantic checks

Integrate, test tight desktop/mobile viewports, then push.

### Wave R — leaderboard foundation

Use the architecture document as the contract. Start with Worker/D1 migrations and read-only board, then add secure session issuance/submission in dependent stages. Do not let client UI work invent a different canonical byte/HMAC protocol.

### Wave S — gameplay audit

Run the industry/game-theory review on the deployed build. Convert accepted recommendations into small measured experiments, not one unbounded rewrite.

## Swarm operating protocol on restart

- Use strong workers for complex Bevy systems
- Give each worker one focused task and explicit file ownership
- Workers use `read`/`edit`; the primary agent owns builds, Git integration, commits, pushes, and deployment verification
- Parallelize only independent files/tasks
- Use async `swarm_spawn` plus one `swarm_watch` for long work
- Review every agent diff; an agent report is not proof of integration
- Guard against Bevy B0001 query conflicts with `Without<>` or disjoint query design
- Remember fresh child `GlobalTransform` can be `IDENTITY` before propagation
- Preserve bounded pools and avoid per-frame mesh/material allocation
- Add pure tests before or alongside behavior changes where practical

## Stable design constraints to preserve

- Bevy 0.19, Rust 1.95.0, edition 2024
- Native and `wasm32-unknown-unknown`/WebGL2 support
- No external 3D models; use procedural primitives/meshes/textures
- Orthographic fixed-isometric camera
- Real PBR IBL environment maps and smooth ellipsoid car normals
- Single-player remains the default
- Touch input order remains `KeyboardInputSet -> TouchInputSet -> DrivingSet`
- Completed terminal rounds, not abandoned/peak scores, update records
- Coin bonus remains `+1.5s`, capped at `90s`
- Traffic/creature placement remains deterministic, safe, and heading-relative
- Settings remains the single source of truth for mute/master/reduced-motion behavior

## Definition of the next fully stable checkpoint

The next checkpoint is complete only when:

- Wave O browser and visual QA passes
- Any Wave O defects are fixed and tested
- CI is green
- Cloudflare Pages production contains the checkpoint
- The working tree is clean
- This document is updated if observed reality differs from the plan
