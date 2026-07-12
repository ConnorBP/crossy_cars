# Wave 2 Plan — Infinite Road, Obstacles, Damage, Rich Chickens + Polish

Bevy 0.19 isometric car game. Native + web/WASM (trunk). Single shared repo; parallel
worker agents implement the tasks. This plan partitions work by **non-overlapping file
ownership** so agents can work concurrently without merge conflicts.

Conventions:
- `OWN` = may edit freely. `MUST-NOT-TOUCH` = never edit. `WIRE` = add a one-line
  `mod` + `add_plugins` in `main.rs` (done by the orchestrator during integration —
  workers do NOT edit `main.rs`).
- All new gameplay modules expose a `XPlugin` with `impl Plugin`.
- Bevy 0.19 messaging: `#[derive(Message)]`, `app.add_message::<T>()`,
  `MessageReader`/`MessageWriter` (NOT Event/EventReader).
- Workers develop in parallel (disjoint files). **PARALLEL-GROUP = merge wave.**
  Section D gives the exact merge/compile order.

---

## A) Infinite-road design — chosen approach

**Approach: chunk recycling along the travel axis (Z).** Variant of the user's option
(b) "procedural city-block chunks stitched together," with a bounded pool that recycles.

**Justification (2-3 sentences):**
Recycling a bounded pool of Z-axis chunks as the car advances gives a seamless endless
feel with a **constant entity count** (web-friendly: no streaming, no asset spikes),
needs **no teleport/camera-jump to hide** (the ortho camera just follows the car), and
lets each chunk vary its building/tree/lamp layout for visual variety — whereas a modulo
wrap repeats the exact same 100×100 cell and requires snapping the camera on each wrap to
hide the discontinuity. It's the standard endless-runner technique and fits the existing
Z-axis road (`forward = -Z` at heading 0) with zero changes to `camera.rs::follow_camera`.

**Module:** `world.rs` (owned by T1, rewritten). No new module — chunk logic stays in
`world.rs` since it IS the environment. Optionally extract pure populate helpers into
`src/chunks.rs` later, but it remains T1-owned.

**Concrete chunk model:**
- `ChunkConfig { length: 40.0, count: 5 }` resource (defined in `world.rs`). Covers 200u of Z.
- `Chunk { index: i32 }` component on chunk-root entities; root transform at
  `z = -index * CHUNK_LENGTH` (car drives toward -Z).
- `populate_chunk(cmds, meshes, mats, textures, chunk_root, index, seed)` builds, per chunk:
  grass strip, road segment (8×40), two sidewalk `Curb`s, lane dashes, ~3 buildings/side,
  ~3 trees/side, ~2 lamp posts/side, ~4 coins. Deterministic seed from `index` for variety.
- `recycle_chunks` (Update, run_if Playing): when the car is > `CHUNK_LENGTH` ahead of the
  trailing chunk, move that chunk root to the front (`z -= count*length`), `despawn()`
  children (recursive — safe in 0.19), re-`populate_chunk` with a fresh seed.
- Car X clamp stays (`x ∈ [-24, 24]`); Z clamp removed. Ground/skydome recenter on car.

---

## B) Task list

### T1 — Infinite road + obstacle collision (features #1 + #2)
- **OWNS:** `src/world.rs`, `src/car.rs`, `src/shaders.rs`, `src/game/mod.rs`, `src/game/events.rs`
- **MUST-NOT-TOUCH:** `src/chickens.rs`, `src/health.rs`, `src/minimap.rs`, `src/persist.rs`,
  `src/countdown.rs`, `src/ui.rs`, `src/audio.rs`, `src/textures.rs`, `src/palette.rs`,
  `src/camera.rs`, `src/game/resources.rs`, `src/game/state.rs`, `src/main.rs`, `Cargo.toml`
- **DEPENDS-ON:** none
- **PARALLEL-GROUP:** Wave 1
- **Sketch:**
  - `world.rs`: replace `spawn_environment` with `spawn_initial_chunks` (Startup) that
    builds `count` chunks covering `z ∈ [0, -count*length)`. Move coin spawning INTO
    `populate_chunk` (coins become environment, not round-scoped). **Delete all chicken
    code** (`Chicken`, `ChickenAssets`, `Feather`, `spawn_chickens`, `wander_chickens`,
    `hit_chickens`, `spawn_one_chicken`, `feathers`, `cleanup_chickens`, `cleanup_feathers`,
    `CHICKEN_ARENA`, `rand`, `rand_unit_xz`) — T3 provides these in `chickens.rs`. Remove
    `spawn_coins`/`cleanup_coins` from `WorldPlugin` (coins now live in chunks); keep
    `spin_coins` + `collect_coins` (collect despawns on pickup, recycled chunks respawn).
    Add `Collider { half_x, half_z }` + `Building`/`Tree`/`LampPost` tag components (pub);
    tag buildings/trees/lamps with `Collider` in `populate_chunk`. Define `ChunkConfig`,
    `Chunk`, `SpawnSet` (pub system set, see below). Define `populate_chunk` +
    `recycle_chunks` (Update). Add `Skydome`-follow by tagging the skydome (see shaders.rs).
  - `car.rs`: in `move_car`, **remove the Z clamp** (`tf.translation.z.clamp(...)`), keep an
    X clamp `[-24.0, 24.0]`. Rewrite `physics_collisions` to iterate `&Collider` generically
    (buildings + trees + lamp posts all collide via the existing circle-vs-AABB pushout)
    instead of `&Solid`; keep the `&Curb` hop-up loop. On a pushout where the car was moving
    into the wall, write `MessageWriter<ObstacleHit { impact_speed: f32 }>` with
    `impact_speed = car.speed.abs()`. Make `physics_collisions` `pub` and expose a pub
    `InputFrozen(pub bool)` resource (init in `CarPlugin`); `move_car` early-returns if
    `InputFrozen.0` (gate for T6 countdown). Remove `use crate::world::{Curb, Solid}` →
    `use crate::world::{Curb, Collider}`. `Solid` is deleted.
  - `shaders.rs`: in `spawn_sky`, add a `Skydome` tag component (pub). Add `update_skydome`
    (Update) that sets the skydome `Transform::translation` = car xz (y=0). Leave
    `WaterMaterial`/`spawn_water`/`update_water` as-is (static landmark the car passes once).
  - `game/mod.rs`: remove `use crate::world::{spawn_chickens, spawn_coins}` (coins no longer
    OnEnter-spawned; chickens moved to `chickens.rs`). Add `app.add_message::<ObstacleHit>()`
    and `app.init_resource::<InputFrozen>()` (or register in CarPlugin — pick one, T1 owns
    both files). Introduce `pub struct SpawnSet;` (SystemSet) and change `reset_run` to run
    `.after(SpawnSet)` instead of `.after(spawn_coins).after(spawn_chickens)`. Gate
    `tick_timeleft` with `if input_frozen.0 { return }` (so countdown doesn't burn the 60s).
    Keep `tick_timeleft`→GameOver and `end_round`.
  - `game/events.rs`: add `#[derive(Message)] pub struct ObstacleHit { pub impact_speed: f32 }`.
- **Contracts other tasks rely on:** `Collider`/`Building`/`Tree`/`LampPost` are pub in
  `world.rs`; `ObstacleHit` is pub in `game/events.rs`; `InputFrozen` + `SpawnSet` are pub;
  `Coin` stays pub in `world.rs`; chicken code is GONE from `world.rs`.

### T2 — Car health & damage (feature #3)
- **OWNS:** `src/health.rs` (new)
- **MUST-NOT-TOUCH:** everything except `src/health.rs` (+ asset files under `assets/audio/`)
- **DEPENDS-ON:** T1 (needs `ObstacleHit` in `game/events.rs`)
- **PARALLEL-GROUP:** Wave 2
- **Sketch:** Define `Health(f32)` resource (max 100, init 100) + `HealthPlugin`
  (`init_resource::<Health>()`). `apply_damage` (Update, run_if Playing): read
  `MessageReader<ObstacleHit>`, subtract `impact_speed * DAMAGE_K` (tune DAMAGE_K≈4 so a
  full-speed ~12 hit ≈ 48 dmg, two-three hard hits wreck). Clamp ≥0; if `Health.0 ≤ 0` →
  `next.set(GameState::GameOver)` (reuse existing state). `reset_health` on
  `OnEnter(Playing)` checks `RoundActive.0` (skip on resume) and sets `Health = 100`.
  `health_bar` UI: spawn a `HealthBarRoot` `Node` (bottom-center, absolute) with a colored
  bar `Node` whose width scales with `Health.0/100`; update each frame (own UI, does NOT
  edit `ui.rs`). `damage_flash`: on damage, spawn a red `DamageFlash` vignette `Node` that
  fades + despawns after ~0.25s. Audio: play a thud on damage and a crash on destroy —
  load `audio/hit.wav` (reuse for thud, lower volume) and add `assets/audio/crash.wav`
  (generate via a tiny Python script, or reuse `hit.wav` if no gen tooling); spawn
  `AudioPlayer` + `PlaybackSettings::DESPAWN`. Crash also: leave `GameOver` to show the
  existing game-over screen (see Risk E4).

### T3 — Rich chickens + particle burst (feature #4 + polish)
- **OWNS:** `src/chickens.rs` (new)
- **MUST-NOT-TOUCH:** `src/world.rs`, `src/car.rs`, everything else non-asset
- **DEPENDS-ON:** T1 (T1 removes chicken code from `world.rs`; defines `SpawnSet`)
- **PARALLEL-GROUP:** Wave 2
- **Sketch:** Move ALL chicken logic here. Define `Chicken { dir: Vec3, timer: f32, bob: f32 }`
  (pub) + `Feather`/particle components + `ChickenAssets` (FromWorld, rich geometry: body
  ellipsoid via scaled `Sphere`, head sphere, red `comb` (3 small cuboids), orange `beak`
  wedge, 2 `Cylinder` legs, 2 black `Sphere` eyes). `spawn_one_rich_chicken` builds a parent
  + children hierarchy. `ChickensPlugin`: `OnEnter(Playing)` `spawn_chickens` in
  `SpawnSet` (queries car `Transform` — origin at round start — scatters ~14 chickens within
  radius 40 of the car; checks `RoundActive.0` to skip on resume). `wander_chickens` (Update):
  move + clamp to a **moving radius** of the car (e.g., 50u); chickens that fall behind
  despawn + respawn ahead — so chickens stay near the car as it drives forever. Face heading.
  Add a **waddle/bob**: animate `bob` with `sin(t*speed)`, set body child
  `Transform::translation.y = base + bob*0.05` + slight z-rotation sway. `hit_chickens`
  (Update): on car distance < 1.0, despawn chicken, `score.chickens += 1`, write
  `MessageWriter<ChickenHit>` (existing message — `audio.rs` already reads it), spawn an
  enhanced **particle burst** (~8 feather spheres + a few "puff" quads with gravity +
  despawn after ~0.5s), respawn ahead of the car. `cleanup_chickens`/`cleanup_particles` on
  `OnEnter(GameOver)` + `OnEnter(Menu)`. Register `app.add_message::<ChickenHit>()` is
  already done in `game/mod.rs` (T1 keeps it) — do NOT re-register. Reference
  `crate::car::Car`, `crate::game::resources::{Score, RoundActive}`, `crate::game::events::ChickenHit`,
  `crate::game::SpawnSet`.

### T4 — Minimap (polish)
- **OWNS:** `src/minimap.rs` (new)
- **MUST-NOT-TOUCH:** all other `.rs` files
- **DEPENDS-ON:** T1 (`Coin`, `Car` pub), T3 (`Chicken` pub)
- **PARALLEL-GROUP:** Wave 3
- **Sketch:** `MinimapPlugin`. Spawn a `MinimapRoot` `Node` (top-right under the timer,
  ~120×120px, semi-transparent panel). Maintain a fixed pool of dot children
  (`MapDot { kind: DotKind }`): yellow=coin, white=chicken, red=car. Each frame, query
  `&Transform` of `Car`/`Coin`/`Chicken`, compute each entity's offset from the car in XZ,
  scale into the 120px box (car-centered, north = car forward), set each dot's
  `Node { left: px(..), top: px(..) }`. Hide dots whose source entity was despawned (reuse
  pool — repurpose, don't respawn). Optionally also plot `Collider` obstacles if T1's
  `Collider`/`Building`/`Tree`/`LampPost` are pub (read-only). No edits to `ui.rs`.

### T5 — Best-score localStorage persistence (polish)
- **OWNS:** `src/persist.rs` (new), `Cargo.toml` (add `web-sys` deps — ONLY this task touches Cargo.toml)
- **MUST-NOT-TOUCH:** all other `.rs` files
- **DEPENDS-ON:** none
- **PARALLEL-GROUP:** Wave 1
- **Sketch:** `BestScore(u32)` resource (init from storage at startup). `PersistPlugin`:
  `Startup` system loads best score (web: `web_sys::window().unwrap().local_storage()`,
  `#[cfg(target_arch="wasm32")]`; native: `std::fs::read_to_string("best_score.txt")`,
  `#[cfg(not(target_arch="wasm32"))]`). `update_best` (Update, run_if in GameOver or on
  transition): if `Score.chickens + Score.coins > BestScore`, update + persist. UI: spawn a
  `BestScoreRoot` `Node` (small "BEST: N" text, top-center or menu corner) updated each
  frame — own UI node, does NOT edit `ui.rs`. `Cargo.toml`: add
  `web-sys = { version = "0.3", features = ["Window", "Storage"] }` (match Bevy 0.19's
  web-sys version — see Risk E5). On native, `cfg`-gated `std::fs` (tiny file, blocking OK).

### T6 — Countdown "3-2-1-GO" intro (polish)
- **OWNS:** `src/countdown.rs` (new)
- **MUST-NOT-TOUCH:** all other `.rs` files
- **DEPENDS-ON:** T1 (`InputFrozen` in `car.rs`, `SpawnSet` in `game/mod.rs`, `RoundActive`)
- **PARALLEL-GROUP:** Wave 2
- **Sketch:** `Countdown` resource `{ t: f32 }` (3.0). `CountdownPlugin`: `start_countdown`
  in `OnEnter(Playing)` **inside `SpawnSet`** (so it runs before `reset_run` flips
  `RoundActive`); checks `RoundActive.0` — skip on resume from Paused. On fresh round, set
  `Countdown.t = 3.0` + `InputFrozen.0 = true` + spawn a `CountdownRoot` UI overlay
  (big centered text). `tick_countdown` (Update, run_if Playing): decrement; update the
  text to "3"/"2"/"1"/"GO!"; when `t ≤ 0`, set `InputFrozen.0 = false`, despawn overlay.
  Because T1 gates `move_car` + `tick_timeleft` on `InputFrozen`, the car is frozen and the
  60s timer doesn't burn during the countdown. Own UI node; no `ui.rs` edit.

---

## C) Additional polish improvements (partitioned)

(Listed above as T4 minimap, T5 best-score, T6 countdown. T3 also folds in a particle
burst on chicken hit. Two more candidates deferred to a future wave to keep scope tight:
**day-night tint** — a `src/daynight.rs` modulating `DirectionalLight` + `GlobalAmbientLight`
+ `ClearColor` over time (needs a `SunLight` tag in `world.rs` → T1 dependency); **skid
marks / drift** — needs `car.rs` drift detection + a decal/trail pool → conflicts with T1's
`car.rs` ownership, so defer to a post-T1 wave.)

---

## D) Integration order (orchestrator)

All workers develop concurrently in disjoint files. Merge/compile in this order:

1. **Wave 1:** Merge **T5** (persist) + **T1** (road+obstacles). After T1, the game
   compiles and runs but has **no chickens** (chicken code removed from `world.rs`,
   `chickens.rs` not yet present) — that's expected and fine (no dangling refs: T1 cleaned
   `game/mod.rs` imports). Wire in `main.rs`: `mod persist; mod world; ...` already there;
   add `add_plugins(PersistPlugin)`. Compile.

2. **Wave 2:** Merge **T3** (chickens), **T2** (health), **T6** (countdown) — these are
   mutually independent (different new files) but each depends on T1 contracts.
   - T3 adds `mod chickens; add_plugins(ChickensPlugin)` — chickens return.
   - T2 adds `mod health; add_plugins(HealthPlugin)` — reads `ObstacleHit`.
   - T6 adds `mod countdown; add_plugins(CountdownPlugin)` — uses `InputFrozen`.
   Wire all three `mod`+`add_plugins` lines in `main.rs` in one edit. Compile.

3. **Wave 3:** Merge **T4** (minimap) — depends on T1 (`Coin`/`Car`) + T3 (`Chicken`).
   Wire `mod minimap; add_plugins(MinimapPlugin)` in `main.rs`. Compile.

4. **Final checks:** `cargo build` (native) + `trunk build` (web). Playtest: drive
   forever, hit trees/lamps/buildings (take damage), wreck → GameOver, collect coins,
   hit chickens (burst + waddle), watch minimap + best-score + countdown.

`main.rs` is edited ONLY by the orchestrator, ONLY to add `mod`/`add_plugins` lines. No
worker touches `main.rs`.

---

## E) Risk notes (Bevy 0.19 / web gotchas workers must watch)

- **E1 — Messages, not Events.** Use `#[derive(Message)]`, `app.add_message::<T>()`,
  `MessageReader`/`MessageWriter`. T1's `ObstacleHit` and T3's reuse of `ChickenHit` must
  follow this (the existing `audio.rs` already reads `ChickenHit` via `MessageReader` —
  don't break it; T3 only *writes* `ChickenHit`, never re-registers it).
- **E2 — Recursive despawn.** `EntityCommands::despawn()` is recursive in 0.19. T1's chunk
  recycle (`commands.entity(chunk_root).despawn()` then re-parent children) and T3's
  particle/chicken cleanup rely on this — safe, but despawning a parent nukes all children.
- **E3 — Double-borrowing Assets.** When building mesh+material handles together (T1 chunk
  populate, T3 `ChickenAssets` FromWorld), use `world.resource_scope::<Assets<Mesh>, _>`
  exactly like the existing `textures.rs::FromWorld` / `world.rs::ChickenAssets` do — never
  hold `&mut Assets<Mesh>` and `&mut Assets<StandardMaterial>` without scoping.
- **E4 — GameOver reuse for "wrecked".** T2 sends `NextState(GameState::GameOver)` on
  destroy; the existing `ui.rs::spawn_gameover` shows "Time's up!" Acceptable for this
  wave. Do NOT edit `ui.rs` to customize the message (conflict-free rule). A
  `GameOverReason` enum + one-line `ui.rs` read is a future-wave tweak.
- **E5 — web-sys version (T5).** Bevy 0.19's `webgl2` feature already pulls `web-sys`
  transitively. Adding `web-sys` directly: match the major/minor Bevy uses
  (`cargo tree -i web-sys` in the workspace) to avoid two versions. Gate all
  `web_sys::window()` calls behind `#[cfg(target_arch="wasm32")]`; native uses `std::fs`.
  Never call `localStorage` before a user gesture on web (it's fine here — best-score
  loads at Startup, saves on GameOver after the player has interacted).
- **E6 — Uniform alignment (if any custom shaders).** T2/T3/T4/T5/T6 add NO custom shaders
  (UI + primitive meshes only), so the WebGL2 16-byte uniform rule doesn't bite. If anyone
  adds a shader, carry scalars in `Vec4` (see `shaders.rs::WaterMaterial::time`).
- **E7 — Shadows on web.** `const SHADOWS: bool = cfg!(not(target_arch = "wasm32"))` stays.
  New lights (none planned this wave) must respect this.
- **E8 — AudioContext gesture unlock.** New SFX (T2 thud/crash) play via `AudioPlayer` +
  `PlaybackSettings::DESPAWN` and will be unlocked by the existing `index.html` JS shim
  (first keypress resumes the context). T2 must not call audio before the player has
  pressed a key — fine, since damage happens during gameplay.
- **E9 — New audio assets.** T2's `crash.wav` (and optional `thud.wav`) go under
  `assets/audio/`. `AssetPlugin.meta_check` is `Never`, so they load with no `.meta` files.
  If no `gen_audio.py` exists in the repo, T2 may reuse `hit.wav` (lower `Volume`) for the
  thud and generate `crash.wav` with a short Python script (any WAV writer) — asset files
  only, no code conflict.
- **E10 — Camera needs NO change.** `camera.rs::follow_camera` already lerps toward
  `car.translation + cam_offset` with no arena clamp, so infinite Z works out of the box.
  T1 must NOT touch `camera.rs`. The only "follow" T1 adds is the skydome recenter
  (`shaders.rs::update_skydome`).
- **E11 — Spawn ordering.** `reset_run` (in `game/mod.rs`) flips `RoundActive` to true, so
  all `OnEnter(Playing)` spawns that check `RoundActive.0` must run BEFORE it. T1's
  `SpawnSet` system set + `reset_run.after(SpawnSet)` enforces this. T3 (`spawn_chickens`)
  and T6 (`start_countdown`) MUST add their `OnEnter(Playing)` systems to
  `crate::game::SpawnSet` — do not register them bare, or resume-from-Paused / fresh-round
  logic breaks.
- **E12 — Chunk seam.** T1's `populate_chunk` should not place a building/tree half-in /
  half-out of a chunk boundary (keep decorations at least `half_extent` inside the chunk's
  Z range) so recycling doesn't pop a half-obstacle into the road.

---

## Addendum — user-requested Wave 2 additions (after Wave 1 integrates)

### T7 — Camera nausea/tilt fix
- **OWNS:** src/camera.rs
- **MUST-NOT-TOUCH:** all other .rs files
- **DEPENDS-ON:** none (camera.rs is disjoint from T1)
- **DIAGNOSIS:** follow_camera calls `cam_t.look_at(look_target, Y)` every frame, where
  look_target is from the LIVE car transform but cam_t.translation is LERPED (lagging). The
  view direction = look_target - cam_t.translation therefore wobbles each frame as the car
  moves/turns and the camera lags -> perceived tilt, most visible when speed (and thus the
  speed-zoom) changes. The `fwd*1.5` look-ahead in both the position target and the look
  target (live target vs lagging position) amplifies it on turns.
- **FIX:**
  - Set the iso camera rotation ONCE at spawn (Transform::from_xyz(...).looking_at(ZERO, Y)
    already gives a fixed rotation; do NOT recompute it per frame).
  - In follow_camera: REMOVE the per-frame `cam_t.look_at(...)`. Only lerp
    `cam_t.translation` toward `desired` (car.translation + cam_offset + look-ahead). Leave
    `cam_t.rotation` untouched (fixed iso angle -> can never tilt).
  - KEEP the speed-zoom (viewport_height lerp 10..12 by speed ratio) — ortho zoom does not
    tilt. Optionally reduce SMOOTH a touch or clamp the zoom delta if still jittery.
  - Result: car stays roughly centered with a slight look-ahead; rotation is rock-steady.

### T8 — Tire trails, tire marks on fast turns, particles
- **OWNS:** src/effects.rs (NEW). WIRED by orchestrator in main.rs (mod effects + EffectsPlugin).
- **MUST-NOT-TOUCH:** all other .rs files. READ-ONLY on crate::car::Car + Transform (no edit).
- **DEPENDS-ON:** T1 integrated (Car struct stable: speed, heading).
- **SKETCH:** EffectsPlugin with FromWorld mesh/material handles (dark flat quad for tire
  marks, soft sphere/quad for smoke) — all procedural, no assets.
  - Track previous heading per car (Local<f32> or a small component) to derive angular
    velocity. "Fast turn" = |angular_vel| high AND car.speed.abs() above a threshold.
  - Tire marks: when turning fast, spawn flat dark quads (blob-shadow-style, on the ground
    at the rear wheel contact points) oriented along travel; fade alpha over ~3-4s then
    despawn. Pool a fixed cap (e.g. 240 marks) to bound entity count (web-friendly) — when
    full, recycle the oldest.
  - Smoke/dust particles: on fast turns (and maybe on hard obstacle hits later), spawn a
    few small grey semi-transparent spheres at the wheels that rise + expand + fade, despawn
    after ~0.4s. Pool/cap these too.
  - Read car rear-wheel world positions from car.translation + car.rotation (wheel offset
    constants — derive from the car's known dimensions; do NOT import car.rs internals,
    just use Transform + Car).
  - All systems run in Update, run_if in_state(GameState::Playing).
- **RISK:** WebGL2 — keep particle counts small and capped; reuse materials (one handle,
  many entities). No custom shaders (UI + primitives only) so no 16-byte uniform issue.

### Updated Wave 2 (run after Wave 1 integrates to a clean, compiling tree)
PARALLEL (all disjoint files): T2 (health.rs), T3 (chickens.rs), T6 (countdown.rs),
T7 (camera.rs), T8 (effects.rs).
- T7 self-verifies via `cargo check` (camera.rs already wired).
- T2/T3/T6/T8 are new modules — orchestrator wires all `mod`+`add_plugins` lines in main.rs
  in ONE edit, then `cargo check` + fix.
