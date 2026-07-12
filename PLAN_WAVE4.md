# Wave 4 Plan — Combos, Power-ups, Obstacle Variety, Camera Shake

Bevy 0.19 isometric car game. Native + web/WASM (trunk). Post-Wave-3 state: infinite
bidirectional chunk recycling + reset_chunks (world.rs), Collider/Building/Tree/LampPost,
Coin, health/damage (health.rs), rich chickens (chickens.rs), tire trails (effects.rs),
countdown, minimap, best-score, PBR rendering (tonemapping/bloom/MSAA/normal-maps/IBL),
fixed camera (no tilt). Messages: ChickenHit, CoinCollected, ObstacleHit (all registered).

Partitioned by NON-OVERLAPPING file ownership for parallel workers. `main.rs` is wired by
the orchestrator (new-module workers may temp-wire to `cargo check` then `git checkout`).

## Tasks

### T10 — Combo multiplier (gameplay)
- **OWNS:** src/combos.rs (NEW). **WIRE** by orchestrator (mod combos + CombosPlugin).
- **MUST-NOT-TOUCH:** all other .rs files. READ-ONLY: game/events (ChickenHit, CoinCollected), game/resources (Score), game/state (GameState), game/SpawnSet.
- **DEPENDS-ON:** none. **PARALLEL-GROUP:** Wave 4.
- **Sketch:** `Combo { multiplier: u32, timer: f32 }` resource. `register_hit` (Update,
  run_if Playing): read MessageReader<ChickenHit> AND MessageReader<CoinCollected>; on any
  hit, increment combo count, reset `timer` to ~2.5s; `multiplier` scales with consecutive
  hits (e.g. 1x, 2x at 5, 3x at 10, capped 5x). Decrement timer each frame; when it hits 0,
  reset multiplier to 1. On a hit, add `multiplier` to Score (chickens or coins) instead of
  +1 — i.e. `score.chickens += multiplier` (so combos multiply score). `reset_combo` on
  OnEnter(Playing) in SpawnSet, checks RoundActive (skip on resume). UI: a `ComboRoot` Node
  (top-center-ish, below the timer) showing "x{multiplier}" + a depleting timer bar; hide
  when multiplier==1. Own UI (no ui.rs edit). Follow persist.rs/health.rs/minimap.rs UI
  patterns (px(), NodeBundle, PositionType::Absolute).

### T11 — Power-ups (gameplay)
- **OWNS:** src/pickups.rs (NEW). **WIRE** by orchestrator (mod pickups + PickupsPlugin).
- **MUST-NOT-TOUCH:** all other .rs files. READ-ONLY: car.rs (Car), world.rs (Coin), game/resources (GameConfig, RoundActive, Score), game/state (GameState), game/SpawnSet, game/events (CoinCollected).
- **DEPENDS-ON:** none. **PARALLEL-GROUP:** Wave 4.
- **Sketch:** `PickupsPlugin`. Power-up kinds: SpeedBoost (faster for ~4s), CoinMagnet (pull
  coins to the car for ~6s). Spawn a power-up entity (a glowing emissive icosahedron/sphere
  on a short pedestal) near the car periodically (every ~8-12s, within radius ~25 ahead of
  the car so it's reachable as you drive). On car distance < ~1.2, despawn + activate.
  Effects (all via queries/resources OWNED here, NO edit to car.rs/world.rs):
  - SpeedBoost: a `SpeedBoostTimer(f32)` resource; while >0, a system queries `&mut Car` and
    scales `car.speed` up (or sets a flag the move_car... NO — don't edit car.rs; instead
    directly boost: while boost active, add a boost velocity by mutating car.speed each
    frame, OR simpler: temporarily raise the effective max by clamping car.speed higher).
    Simplest robust approach: while boost active, `car.speed *= 1.0` no — just add a
    constant forward nudge via &mut Car each frame (car.speed += boost_accel * dt, capped at
    max_speed*1.6). Decrement timer.
  - CoinMagnet: a `MagnetTimer(f32)` resource; while >0, a system queries `&mut Transform`
    of Coin and moves each coin toward the car (lerp translation toward car xz) so they're
    collected by the existing collect_coins. Decrement timer.
  UI: small icons/timers bottom-center-ish (above the health bar) showing active power-ups;
  own UI node (no ui.rs edit). `reset_pickups` on OnEnter(Playing) in SpawnSet, checks
  RoundActive. Clean up power-up entities + timers on OnEnter(GameOver)/OnEnter(Menu).
  Emissive glow so they pop with bloom. Keep counts small (web-friendly).

### T12 — Obstacle variety (polish)
- **OWNS:** src/world.rs (ONLY). **MUST-NOT-TOUCH:** all other .rs files.
- **DEPENDS-ON:** none. **PARALLEL-GROUP:** Wave 4.
- **Sketch:** In `populate_chunk`, add 2-3 NEW obstacle types alongside buildings/trees/
  lamp-posts, each with a `Collider` + a tag, built from primitives:
  - **Traffic cone** (Cone/Cylinder body + base) — narrow collider (~0.25).
  - **Fire hydrant** (short Cylinder + dome) — collider ~0.3.
  - **Bench** (long thin Cuboid seat + legs) — collider ~0.8 x 0.3.
  - **Hedge** (green box row) — collider ~1.0 x 0.4.
  Scatter ~2-4 of these per chunk (in addition to existing ~3 buildings/3 trees/2 lamps),
  on the grass/sidewalk edges (NOT in the road lane — keep the road drivable). Reuse the
  existing shadow_mat / make per-type. Add pub tag components (Cone, Hydrant, Bench, Hedge)
  if useful (or just use Collider + the mesh). DO NOT touch `recycle_chunks`,
  `reset_chunks`, `spawn_initial_chunks`, or the chunk-root structure — only ADD obstacle
  spawns inside `populate_chunk`'s existing `with_children` block. Keep the 3u seam margin
  (E12). Verify `cargo check` passes.

### T13 — Camera shake on collision (juice)
- **OWNS:** src/camera.rs (ONLY). **MUST-NOT-TOUCH:** all other .rs files. READ-ONLY: game/events (ObstacleHit), car.rs (Car), game/state (GameState).
- **DEPENDS-ON:** none. **PARALLEL-GROUP:** Wave 4.
- **Sketch:** Add a `Shake { trauma: f32 }` resource (or Local in the system). A system reads
  `MessageReader<ObstacleHit>`; on a hit, `trauma = (trauma + impact_speed*scale).clamp(0,1)`.
  In `follow_camera` (or a chained shake system), while trauma > 0, add a decaying random
  OFFSET to `cam_t.translation` (translation ONLY — NEVER rotation, to preserve the T7
  fixed-rotation no-tilt fix) — e.g. `cam_t.translation += random_offset * trauma^2`. Decay
  `trauma` by `exp(-dt*rate)`. CRITICAL: preserve T7's fixed rotation (no look_at), T9's
  tonemapping/Bloom/Msaa components, the lerp-only-translation follow, and the speed-zoom.
  Only ADD a translational shake offset on top. Verify `cargo check` passes. Run on both
  Playing states (shake only matters during play).

## Integration order (orchestrator)
1. Merge all 4 (disjoint files). T12 (world.rs) + T13 (camera.rs) self-verify via cargo check.
   T10 (combos.rs) + T11 (pickups.rs) are new modules — wire `mod combos; mod pickups;` +
   `add_plugins(CombosPlugin, PickupsPlugin)` in main.rs in one edit.
2. `cargo check` + `cargo check --target wasm32`. Fix errors.
3. `trunk build`, browser-audit (combos multiply score, power-ups spawn/activate, new
   obstacles visible + collidable, camera shakes on crash, no panic).
4. Commit Wave 4. Then Wave 5 scout (deferred: day-night, skid/drift, difficulty ramp,
   traffic, weather, mobile controls, audio polish).

## Risk notes
- Messages not events (MessageReader/MessageWriter). Multiple readers of the same message
  are fine (T10 reads ChickenHit/CoinCollected alongside chickens.rs/audio.rs).
- T10/T11 UI: own nodes, no ui.rs edit; follow persist.rs/health.rs/minimap.rs patterns.
- T12: do NOT alter chunk recycling/reset/spawn_initial — only add obstacles in
  populate_chunk. Keep road lane clear. Seam margin 3u.
- T13: translation-only shake; preserve T7 (no look_at, fixed rotation) + T9 (tonemap/bloom/
  msaa). NEVER rotate the camera.
- WebGL2: keep entity counts bounded (pool/reuse). No new custom shaders (so no 16-byte
  uniform issue). Shadows cfg!(not wasm32).
