# TODO: integrate the isometric world asset kit

A validated first-pass environment kit is available under:

```text
assets/models/world/isometric/
```

This handoff intentionally does **not** modify runtime code. Integrate incrementally and retain the procedural roots/components used by recycling, collisions, and occlusion fading.

## Coordinate contract

- Format: glTF 2.0 binary (`.glb`)
- Load the default scene: `#Scene0`
- Origin: ground center
- Blender up `+Z` exports to Bevy up `+Y`
- Blender front/entrance `+Y` exports to Bevy forward `-Z`
- Root transforms are identity and every asset has exact ground contact.
- Palette materials are embedded; there are no external textures or real light entities.

```rust
SceneRoot(asset_server.load("models/world/isometric/house_cottage_gabled.glb#Scene0"))
```

## Buildings

| File | Root node | Footprint / height | Suggested collider half extents | Tris | Meshes | Materials |
|---|---|---:|---:|---:|---:|---:|
| `house_cottage_gabled.glb` | `Building_Cottage_Gabled` | 4.2 × 4.0 × 4.85 | 2.10 × 2.00 | 1,052 | 9 | 9 |
| `house_porched_blue.glb` | `Building_House_Porched_Blue` | 4.6 × 4.4 × 6.15 | 2.30 × 2.20 | 1,508 | 8 | 8 |
| `townhouse_brick.glb` | `Building_Townhouse_Brick` | 4.7 × 4.3 × 7.25 | 2.35 × 2.15 | 1,972 | 9 | 8 |
| `apartment_modern_balconies.glb` | `Building_Apartment_Modern_Balconies` | 4.95 × 4.8 × 8.55 | 2.48 × 2.40 | 2,504 | 10 | 9 |

### Building integration task

In `src/world.rs`, replace the procedural cuboid **visual children** created for a building with one deterministic GLB scene variant.

Keep these on the existing procedural building root:

- `Building`
- `Collider`
- block/recycling parentage
- the root `Transform`
- any metadata used by `transparency.rs`

Use the fixed footprint table above instead of randomized `w/d` when choosing collider extents. The GLB should be a visual child of that root. This allows `transparency.rs` to continue gathering and fading elevated descendants.

Recommended first mapping:

- low height bucket → cottage or blue porch house
- medium height bucket → brick townhouse
- tall height bucket → balcony apartment
- choose variant deterministically from the existing block seed

Do not spawn the old body/roof/window-strip meshes when a GLB visual is used.

## Props

| File | Root node | Dimensions | Suggested collider half extents | Tris | Meshes |
|---|---|---:|---:|---:|---:|
| `tree_urban_blocky.glb` | `Prop_Tree_Urban_Blocky` | 1.45 × 1.45 × 3.4 | 0.30 × 0.30 (preserve current gameplay value) | 64 | 3 |
| `streetlamp_classic.glb` | `Prop_Streetlamp_Classic` | 0.95 × 0.95 × 3.25 | 0.15 × 0.15 (preserve current gameplay value) | 120 | 2 |
| `bench_park.glb` | `Prop_Bench_Park` | 1.8 × 0.65 × 1.05 | 0.90 × 0.33, rotated with visual if orientation varies | 132 | 2 |
| `mailbox_residential.glb` | `Prop_Mailbox_Residential` | 0.78 × 0.60 × 1.48 | 0.30 × 0.24 | 92 | 4 |
| `hydrant_city.glb` | `Prop_Hydrant_City` | 0.72 × 0.72 × 1.01 | 0.25 × 0.25 (preserve current gameplay value) | 252 | 2 |

### Prop integration tasks

1. Replace tree visual children in `world.rs`; keep `Tree` and the existing collider on the procedural root.
2. Replace lamp visual children; keep `LampPost`, collider, placement, and arm-facing logic only if still needed.
3. Replace bench visual children; keep `Bench` and collider. Ensure collider orientation matches any randomized yaw.
4. Add the mailbox as a new low-frequency residential/roadside scatter variant.
5. Replace hydrant visual children; keep `Hydrant` and existing collider.

## Loading/performance suggestion

Load the nine scene handles once into a resource at startup, then clone handles while populating/recycling blocks. Do not call `AssetServer::load` repeatedly inside every block spawn if the handle table can be reused.

All building assets stay below 2,600 triangles; props stay below 260. Assets use 2–10 mesh primitives grouped by material. This is appropriate for the current WebGL2 target, but retain existing spawn-count caps.

## Validation checklist

- Confirm each scene loads from `#Scene0`.
- Verify front entrances face the intended road; rotate building roots in 90° increments as needed.
- Confirm roots touch ground at local `Y = 0` in Bevy.
- Confirm colliders match fixed footprints and remain on procedural roots.
- Confirm `transparency.rs` fades imported building descendants.
- Confirm block recycling despawns imported scene hierarchies recursively.
- Check WebGL2 draw calls and frame time in a dense five-by-five block window.
- Verify warm window materials are visible but do not act as real lights.

## Source and audits

The main asset directory includes:

- `manifest.json` — source topology and dimensions
- `audit.json` — final GLB structure/ground-contact summary

Authoring/review files remain in the isolated recovery worktree under `review/world-assets/`.

## Next asset loop backlog

After this kit is integrated and scaled in-game, the next modeling pass should prioritize:

1. corner shop / café with striped awning
2. small service station or garage
3. farm barn and silo set for Field blocks
4. park trash bin and planter
5. traffic barrier and roadwork sign
6. dumpster and utility cabinet
7. bus shelter
8. playground set
9. low industrial warehouse
10. two additional tree canopy variants
