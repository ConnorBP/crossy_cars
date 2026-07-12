# Wave 5 Plan — 2D City Grid, PBR Metal, Pickups, Critters, Difficulty

Bevy 0.19 isometric car game. Native + web/WASM (trunk). Post-Wave-4 state: 1D chunk
recycling along Z (world.rs), Collider/Building/Tree/LampPost + T12 obstacles, Coin, Curb,
health/damage (health.rs), rich chickens (chickens.rs), tire trails (effects.rs), combos
(combos.rs), power-ups SpeedBoost+CoinMagnet (pickups.rs), countdown, minimap, best-score,
PBR rendering (tonemapping/bloom/MSAA/normal-maps/IBL), fixed camera (no tilt), camera
shake. Messages: ChickenHit, CoinCollected, ObstacleHit. Collision uses GlobalTransform
(chunk children). Overlap-rejection placement in populate_chunk.

Partitioned by NON-OVERLAPPING file ownership. `main.rs` wired by orchestrator (new-module
workers temp-wire to cargo check then `git checkout src/main.rs`).

## Tasks

### T14 — 2D city-block grid (the big one)
- **OWNS:** src/world.rs, src/car.rs. **MUST-NOT-TOUCH:** all other .rs files.
- **DEPENDS-ON:** none. **PARALLEL-GROUP:** Wave 5.
- **Goal:** replace the 1D-along-Z chunk system with a full 2D grid of city blocks with a
  road grid + intersections, so the car can drive side-to-side (X) as well as forward/back
  (Z). Endless in ALL directions via 2D recycling.
- **Design:**
  - `GridConfig { block: 40.0, count: 5 }` resource (5x5 window of blocks around the car).
  - `Block { gx: i32, gz: i32 }` component on block-root entities; root at world
    `(gx*block, 0, gz*block)`. Blocks centered on the origin at spawn
    (gx,gz in -2..=2 for count=5).
  - `spawn_initial_grid` (Startup) + `reset_grid` (OnEnter Playing, in SpawnSet, checks
    RoundActive) — despawn all blocks, spawn the count×count grid centered on the car's
    spawn (origin). Keep the sun (Startup-only, not re-spawned).
  - `populate_block(cmds, meshes, mats, textures, root, gx, gz, seed)`: each block is a
    city block — grass cell, buildings/trees/obstacles INSIDE the cell (reuse the existing
    overlap-rejection `try_place` + Collider + tags; keep obstacles off the road grid),
    road segments on its edges (draw the +X and +Z edge roads so adjacent blocks tile the
    full road grid), curbs along roads, lane dashes, a few coins on the roads. Intersections
    emerge where road grid lines cross. Keep the 3u seam margin.
  - `recycle_grid` (Update, run_if Playing): for EACH axis (X and Z), find the min/max
    block coordinate; when the car is past the grid edge by VIEW_MARGIN (~16), recycle the
    far row/column: despawn those block roots (recursive) and spawn fresh ones on the
    opposite side with progressed (gx,gz) + fresh seed. Keeps a continuous count×count
    window around the car in BOTH X and Z -> no gaps, car can drive endlessly in any
    direction.
  - Keep using `GlobalTransform` for collision (chunk... now block children). Keep
    `Collider`/`Curb`/`Coin`/`Building`/`Tree`/`LampPost` + T12 obstacle tags + the
    `Cone`/`Hydrant`/`Bench`/`Hedge` tags. Keep `spin_coins` + `collect_coins` (GlobalTransform).
  - car.rs `move_car`: REMOVE the X clamp `[-24,24]` (car can drive anywhere in 2D). Keep
    the heading/forward movement + curb hop + InputFrozen gate. The car already turns with
    A/D and drives W/S, so 4-direction driving works by turning — no major movement change.
  - camera.rs is NOT touched (fixed iso rotation + lerp position + look-ahead + speed-zoom
    already work for 2D driving).
- **Risk:** This is a large rewrite. Preserve the learned gotchas: GlobalTransform for
  block-child collision; skip IDENTITY GlobalTransform colliders in physics_collisions
  (stale-at-spawn guard — keep it!); overlap-rejection placement; recursive despawn;
  reset on round start; bidirectional recycle (now 4-directional). TEST: cargo check.

### T15 — Realistic PBR car metal + better grass/tree textures
- **OWNS:** src/textures.rs. **MUST-NOT-TOUCH:** all other .rs files.
- **DEPENDS-ON:** none. **PARALLEL-GROUP:** Wave 5.
- **Sketch:**
  - Car paint: make it look like real metallic car paint. Tune `metallic` ~0.7-0.9,
    `perceptual_roughness` ~0.15-0.3 (clearcoat gloss). Add a finer procedural NORMAL map
    for a subtle orange-peel / metal-flake shimmer (the existing car_paint normal map is
    weak — make it read as glossy metal under the IBL + bloom). Keep `base_color_texture`
    smooth (no crawling noise). The IBL (EnvironmentMapLight) + bloom are already on, so
    high metallic + low roughness will give real reflections.
  - Grass texture: richer — more natural color variation (multi-tone green, subtle
    yellow/brown patches), finer noise, maybe a faint mowing-stripe direction. Keep tiling
    seamless (Repeat sampler). Update the grass normal map too if it helps.
  - Tree/foliage texture: better — leaf-like variation (greens with lighter highlights +
    darker shadows), not a flat green. If foliage uses a solid color material (check
    world.rs foliage_mat — you can't edit world.rs, so if the foliage is a solid color in
    world.rs, you can only improve the GRASS/ROAD/SIDEWALK/CAR textures you own; note any
    foliage improvement that needs world.rs as an orchestrator follow-up). Improve the
    textures you own (grass/road/sidewalk/car_paint + their normal maps).
  - Road texture: better asphalt (richer gravel/noise) — user asked for better grass but
    road is adjacent; a quick road texture bump is fine too.
- **Web-safe:** no new shaders (textures only); normal maps are standard textures. Verify
  cargo check.

### T16 — More pickups: health bonus + time bonus + fun ones
- **OWNS:** src/pickups.rs. **MUST-NOT-TOUCH:** all other .rs files. READ-ONLY: health.rs (Health resource), game/resources (TimeLeft, Score, RoundActive, GameConfig), game/state, game/events, car.rs (Car), world.rs (Coin).
- **DEPENDS-ON:** none. **PARALLEL-GROUP:** Wave 5.
- **Sketch:** Extend `PickupsPlugin` with new power-up kinds alongside SpeedBoost/CoinMagnet:
  - **HealthPickup** (green cross orb): restores Health (e.g. +35). Reads `ResMut<Health>`
    (from health.rs — `Health(pub f32)`). Play a positive SFX (reuse coin.wav at low vol).
  - **TimeBonus** (clock orb): adds +5s to `TimeLeft` (ResMut<TimeLeft>).
  - **MegaCoin** (big gold orb): +5 coins to Score (ResMut<Score>) + a coin-pickup sound.
  - (Optional) **Shield**: brief invincibility — set a `ShieldTimer` resource; while active,
    health.rs apply_damage should skip... but you can't edit health.rs. So Shield would need
    health.rs to read a ShieldTimer resource (orchestrator follow-up) — SKIP Shield for now,
    do Health/Time/MegaCoin which are self-contained.
  - Spawn each kind with a distinct color + emissive glow (bloom). Reuse the existing spawn-
    near-car + collect + UI bar infrastructure. Distinct probabilities (Health rarer than
    TimeBonus). Keep counts capped (web-friendly). reset_pickups + cleanup already exist.

### T17 — AI critters you must NOT hit (pedestrian / cow / moose)
- **OWNS:** src/critters.rs (NEW). **WIRE** by orchestrator (mod critters + CrittersPlugin).
- **MUST-NOT-TOUCH:** all other .rs files. READ-ONLY: car.rs (Car, Transform), game/resources (Score, Health... Health is in health.rs — read ResMut<Health>), game/state, game/events (write a new CritterHit message OR reuse ObstacleHit? Better: a new `CritterHit` message), game/SpawnSet, chickens.rs (for patterns).
- **DEPENDS-ON:** none (T14's car/world are read-only). **PARALLEL-GROUP:** Wave 5.
- **Sketch:** `CrittersPlugin`. Wandering AI critters on/near the roads that you must AVOID.
  Hitting one = PENALTY (lose Health + lose score). Build from primitives:
  - **Pedestrian**: small capsule body + head sphere + simple legs, neutral colors. Walks
    along sidewalks / crosses roads slowly.
  - **Cow**: boxy body + head + 4 legs, black/white spots (procedural material). Slow.
  - **Moose**: taller, brown, antlers (a few thin cuboids). Slow.
  - `Critter { dir, speed, timer, bob }` component. `spawn_critters` (OnEnter Playing, in
    SpawnSet, checks RoundActive) scatters a few near the car. `wander_critters` (Update,
    run_if Playing): move along dir, periodically pick new dir, stay near the car (recycle
    ahead like chickens), waddle/bob. `hit_critters` (Update): on car distance < ~1.2,
    despawn critter, apply PENALTY: `health.0 -= 25.0` (ResMut<Health>) + `score.chickens =
    score.chickens.saturating_sub(2)` (or a score penalty) + write `CritterHit` message
    (register `app.add_message::<CritterHit>()` in CrittersPlugin) + spawn a particle burst
    (red for penalty) + play a bad-thud SFX (reuse hit.wav). `cleanup_critters` on
    GameOver/Menu. Distinct from chickens (which AWARD score) — critters PENALIZE. Keep
    counts small (web-friendly). Do NOT give critters a `Collider` (so physics_collisions
    doesn't push the car off them — the hit_critters distance check handles it, allowing the
    car to "run them over" with a penalty).

### T18 — Difficulty ramp + traffic over time
- **OWNS:** src/difficulty.rs (NEW). **WIRE** by orchestrator (mod difficulty + DifficultyPlugin).
- **MUST-NOT-TOUCH:** all other .rs files. READ-ONLY: car.rs (Car, Transform), game/resources (TimeLeft, RoundActive, GameConfig, Score), game/state, game/SpawnSet.
- **DEPENDS-ON:** none. **PARALLEL-GROUP:** Wave 5.
- **Sketch:** `Difficulty { elapsed: f32, level: u32 }` resource. `DifficultyPlugin`:
  - `tick_difficulty` (Update, run_if Playing, gated on InputFrozen like the timer): increment
    `elapsed` by dt (only when not frozen). `level = (elapsed / 10.0) as u32` (ramps every
    10s). Reset on OnEnter(Playing) in SpawnSet (checks RoundActive).
  - **Traffic**: spawn simple moving "traffic car" entities (a colored Cuboid body, no wheels
    needed) on the roads near the car; count scales with `level` (e.g. 1 + level/2, capped
    ~8). Traffic moves along the road (pick a road axis/direction, drive straight, despawn
    when far behind/ahead the car + respawn). Traffic has a `Collider` so the car crashes
    into it (ObstacleHit -> damage) — they're obstacles to avoid. Tag `Traffic`. Keep counts
    capped (web-friendly). `cleanup_traffic` on GameOver/Menu.
  - (Optional) also ramp chicken/critter spawn rates via the Difficulty resource — but those
    modules don't read it (you can't edit them). So just do traffic here; note rate-ramp as
    a future orchestrator follow-up if needed.
  - UI: a small "Lv {level}" indicator (top-right corner or near the timer). Own UI node.

## Integration order (orchestrator)
1. Merge all 5 (disjoint files). T14 (world+car), T15 (textures), T16 (pickups) self-verify
   via cargo check. T17 (critters), T18 (difficulty) are new modules — wire `mod critters;
   mod difficulty;` + `add_plugins(CrittersPlugin, DifficultyPlugin)` in main.rs (watch the
   Bevy 16-tuple Plugins limit — split add_plugins if needed).
2. `cargo check` + `cargo check --target wasm32`. Fix errors (likely B0001 Without<> issues
   + the plugin tuple split).
3. `trunk build`, browser-audit (2D grid drives in all directions, pickups spawn, critters
   penalty, traffic spawns + crashes, PBR metal looks right, no panic). The audit drives
   W+A/D — extend it to also press S (reverse) + drive in different directions to exercise
   the 2D recycling.
4. Commit Wave 5. Defer foreground-building transparency to Wave 6 (needs per-building
   material instances — cleaner after the grid lands).

## Risk notes
- T14 is LARGE — watch for stalls (flat cost). Rechunk or take over if needed. Preserve:
  GlobalTransform collision, IDENTITY-skip guard in physics_collisions, overlap-rejection
  placement, bidirectional (now 4-directional) recycle, reset on round start, recursive
  despawn.
- B0001: any new system with two queries on the same mutable component needs Without<>.
  (Recurrence this project: apply_damage magnet, update_powerup_ui.) Auditors check.
- Bevy 0.19: messages not events; Msaa is a Component; Plugins tuple max ~12 (split);
  shadows cfg!(not wasm32); no compute on WebGL2 (IBL already pre-baked).
- T17 critters: do NOT add Collider to critters (hit_critters distance check handles it,
  lets the car "run over" them for a penalty instead of bouncing off).
- T18 traffic: DO add Collider to traffic (they're obstacles to crash into).
- Foreground transparency deferred (Wave 6) — conflicts with T14's per-building materials.
