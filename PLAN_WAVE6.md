# Wave 6 Plan — Wang-tile Road Variety, Foreground Transparency, Audio Polish

Bevy 0.19 isometric car game. Native + web/WASM. Post-Wave-5: 2D city-block grid
(world.rs) with a road on every block's -X/-Z edge (uniform Manhattan grid), 4-directional
recycling, round reset. PBR rendering, critters, traffic, pickups, combos, etc. all in.

Partitioned by NON-OVERLAPPING file ownership. `main.rs` wired by orchestrator.

## T19 — Wang-tile road-network variety (world.rs)
- **OWNS:** src/world.rs. **MUST-NOT-TOUCH:** all other .rs files.
- **DEPENDS-ON:** none. **PARALLEL-GROUP:** Wave 6.
- **Goal:** replace the uniform "road on every -X/-Z edge" grid with a **Wang-tile** road
  network — a small tile set with road/no-road edge sockets, placed per-block to match the
  already-placed neighbors' shared edges. Roads stay guaranteed-connected (edge-continuity)
  but the map gains variety: parks, bigger blocks, missing roads, T-intersections, corners.
- **Why Wang tiles (not WFC):** streaming/recycling places one block at a time (can't collapse
  a global grid); Wang-tile placement only needs the placed neighbors' edges, is O(1)-ish,
  can't fail/backtrack, and edge-matching guarantees roads don't dead-end at boundaries
  (stronger drivability than WFC's local-adjacency). Deterministic per-block (seedable).
- **Design:**
  - Edge sockets: each block edge (W=-X, E=+X, S=-Z, N=+Z) is either `Road` or `None`.
  - `TileKind` enum + a `sockets(kind) -> [Edge; 4]` (W,E,S,N order). Tile set (keep it COMPLETE
    so a matching tile always exists for any fixed-edge combo):
    - `Empty` — all None (full block: buildings or a park).
    - `RoadNS` — W=None,E=None,S=Road,N=Road.
    - `RoadEW` — W=Road,E=Road,S=None,N=None.
    - `Cross` — all Road (4-way intersection).
    - `TN`, `TE`, `TS`, `TW` — T-intersections (3 Road, the named edge None).
    - `CornerWN`, `CornerNE`, `CornerES`, `CornerSW` — 2 adjacent Road (corners).
    - `Park` — all None, interior is a park (grass + trees, no buildings) — a variant of Empty
      for visual variety (pick Park vs Empty-Buildings randomly when all edges None).
  - Store the chosen kind on `Block { gx, gz, kind: TileKind }` (pub) so neighbors can read it.
  - `populate_block` gains a `fixed: [Option<Edge>; 4]` param (W,E,S,N — Some = fixed by an
    existing neighbor's matching edge, None = free). It picks a TileKind whose sockets match
    all `Some` entries (randomize among matches, weighted toward through-roads/intersections
    so the network stays connected; Park/Empty only when all-four are None or all fixed as
    None). Randomize the free edges. Then build, per the chosen tile's sockets:
    - Grass cell (always).
    - A road segment on each `Road` edge (reuse the existing Plane3d road mesh + textures.road,
      positioned on that edge: -X edge road runs along Z at local x=-half; +X at x=+half;
      -Z along X at z=-half; +Z at z=+half). Skip edges that are `None`.
    - Curbs + lane dashes on each road edge (reuse existing patterns, oriented to the edge).
    - Intersection markings where two road edges meet (optional, simple).
    - Buildings/trees/lamps/T12-obstacles in the INTERIOR, but ONLY in the region not occupied
      by roads — i.e. shrink the buildable interior away from any `Road` edge (keep the 6u
      margin from each road edge; a `None` edge can use the full half-block). Use the existing
      `try_place` overlap rejection. For `Park`/`Empty`-park: spawn trees + a park-green ground
      tint instead of buildings.
    - Coins on the road edges (only where Road).
  - Callers compute `fixed` from existing neighbors:
    - `spawn_initial_grid` / `reset_grid`: iterate blocks in ascending (gx,gz) order; for each,
      look up the already-placed W (gx-1,gz) and S (gx,gz-1) neighbors' E/N sockets -> fixed
      W/S; E/N free. (Spawn order guarantees W/S exist.)
    - `recycle_grid`: when spawning a new block on an edge, its INWARD neighbors exist (the
      existing grid) -> those edges fixed; outward edges free. Query the existing `Block` query
      for the neighbor at (gx±1, gz) / (gx, gz±1) and read the matching socket.
  - KEEP: GlobalTransform collision (car.rs unchanged), IDENTITY-skip guard, overlap-rejection,
    recursive despawn, round reset, 4-directional recycle, all tags pub, seed_for deterministic
    (fold the tile choice into the seed so a given (gx,gz,seed) always picks the same tile when
    free edges are randomized — stable across recycles).
- **Risk:** keep the tile set COMPLETE (every fixed-edge combo has a matching tile) or
  placement can deadlock. Bias toward through-roads so the network stays connected. Verify
  cargo check. The car must still be able to drive (roads connect) — the audit drives in 4
  directions; if the agent traps the car, the road set needs more through-road bias.

## T20 — Foreground buildings semi-transparent when occluding the car (transparency.rs, NEW)
- **OWNS:** src/transparency.rs (NEW). **WIRE** by orchestrator (mod transparency + TransparencyPlugin).
- **MUST-NOT-TOUCH:** all other .rs files. READ-ONLY: world.rs (`Building` tag), car.rs (`Car`), camera.rs (Camera3d), bevy::render camera types.
- **DEPENDS-ON:** none (self-contained — handle-swap, no world.rs edit). **PARALLEL-GROUP:** Wave 6.
- **Goal:** buildings between the camera and the car fade semi-transparent so you can see the
  car through them.
- **Design (handle-swap, no per-building material instances needed):**
  - `TransparencyPlugin`. A FromWorld `GhostMat` (a single semi-transparent material:
    `StandardMaterial { base_color: Color::srgba(1.0,1.0,1.0,0.25), alpha_mode: AlphaMode::Blend, depth_write_enabled: false(? check 0.19), ..default() }` — a uniform ghost; color loss is fine since they're de-emphasized).
  - `Faded { original: Handle<StandardMaterial> }` component on building entities that are
    currently faded.
  - `update_transparency` (Update, run_if in_state(Playing)): query the car `&GlobalTransform`
    (single), the camera `&GlobalTransform` + `&Projection` (single), and buildings
    `(Entity, &GlobalTransform, &Building, Option<&Faded>, &mut MeshMaterial3d<StandardMaterial>)`.
    For each building, test whether it OCCLUDES the car:
    - Simplest robust heuristic for the ortho iso camera: a building occludes the car if it's
      CLOSER to the camera than the car along the camera's view direction (depth test) AND its
      screen-space position is near the car's screen-space position (within the building's
      projected radius). Project world->screen using the camera `GlobalTransform` + ortho
      `Projection` (compute view + ortho matrices manually, or use `bevy::render::camera::
      Camera::world_to_screen` if accessible — check the 0.19 API; a manual ortho projection
      is straightforward).
    - If occluding and not already `Faded`: insert `Faded { original: current_handle.clone() }`
      + set `MeshMaterial3d` to `GhostMat` (commands + the mut query).
    - If NOT occluding and is `Faded`: restore `MeshMaterial3d` to `Faded.original` + remove
      `Faded`.
  - Only fade `Building` entities (not trees/lamps — keep it simple + cheap). Keep the count
    of faded buildings bounded (only those actually near the car in screen space).
  - Shadows/web: `depth_write_enabled` may not be a StandardMaterial field in 0.19 (we checked
    earlier it isn't) — so just use `AlphaMode::Blend` for the ghost. Verify it renders OK on
    web (no compute needed — standard alpha blend works on WebGL2).
- **Risk:** the occlusion heuristic + screen projection. A manual ortho projection is fine.
  Keep it cheap (only buildings within some world-distance of the car are even tested).
  Verify it compiles + doesn't tank perf. Do NOT edit world.rs.

## T21 — Audio polish (audio.rs)
- **OWNS:** src/audio.rs. **MUST-NOT-TOUCH:** all other .rs files. READ-ONLY: car.rs (`Car`), game/state.
- **DEPENDS-ON:** none. **PARALLEL-GROUP:** Wave 6.
- **Sketch:**
  - Engine sound: `update_engine` already modulates the looping engine `AudioSink` — improve
    the pitch+volume curve to track `car.speed` smoothly (pitch rises with speed, idle at 0,
    a believable engine curve). Read src/audio.rs + src/car.rs (Car.speed, GameConfig.max_speed)
    first.
  - Add subtle ambient: optional low-volume ambient hum/wind loop while Playing (generate a
    soft ambient wav via a Python script if needed, or skip if it needs too much).
  - UI/click already fixed (soft). Verify the coin/hit/click/crash/engine levels are balanced
    (none jarring). Lower any that are too loud.
  - Keep web-safe: AudioPlayer + PlaybackSettings; the index.html gesture-unlock handles the
    AudioContext. Verify cargo check.

## Integration order (orchestrator)
1. Merge all 3 (disjoint files). T19 (world.rs) + T21 (audio.rs) self-verify via cargo check.
   T20 (transparency.rs) is a new module — wire `mod transparency; add_plugins(TransparencyPlugin)`
   in main.rs (watch the Bevy ~12-tuple Plugins limit — the second add_plugins tuple is at 9;
   adding TransparencyPlugin = 10, still fine).
2. `cargo check` + `cargo check --target wasm32`. Fix errors (watch B0001 -> Without<>).
3. `trunk build`, browser-audit (2D drive in 4 directions — verify Wang-tile roads still
   connect + the car isn't trapped; buildings fade when occluding; audio balanced; no panic).
4. Commit Wave 6. Then Wave 7 candidates: day-night, skid/drift, more obstacle/critter
   variety, mobile/touch controls, a start-screen polish.

## Risk notes
- T19: COMPLETE tile set or deadlock; bias through-roads for connectivity; deterministic tile
  choice (seed) for stable recycles; preserve all Wave-5 gotchas.
- T20: ortho screen projection; keep it cheap; AlphaMode::Blend ghost; no world.rs edit.
- B0001: two queries same mutable component -> Without<> (recurring project bug).
- Bevy 0.19: Msaa is a Component; Plugins tuple max ~12; shadows cfg!(not wasm32); no compute
  on WebGL2.
- Commit as we go.
