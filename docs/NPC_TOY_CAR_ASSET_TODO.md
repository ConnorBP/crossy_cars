# TODO: integrate the NPC toy-car GLB kit

Five validated traffic visuals are available at:

```text
assets/models/traffic/toy/
```

This is an asset-only handoff. Runtime traffic code was not modified.

## Asset contract

- Format: glTF 2.0 binary (`.glb`)
- Scene label: `#Scene0`
- Origin: ground center
- Blender `+Z` up → Bevy `+Y` up
- Blender `+Y` front → Bevy `-Z` front
- No additional model-root rotation should be required.
- Root transform is identity.
- Wheel axles are local `X`; roll wheel pivot nodes around local `X`.
- Glass is opaque; there is no alpha and no real light entity.
- Every GLB contains a consistently named `Toy_Paint` material for optional deterministic color override.

## Variants

| File | Root | Actual render bounds (W × L × H) | Tris | Meshes | Suggested collider half extents |
|---|---|---:|---:|---:|---:|
| `npc_toy_sedan.glb` | `NPC_Toy_Sedan` | 1.349 × 2.270 × 0.890 | 1,184 | 14 | Preserve current `0.50 × 1.00`, or use `0.54 × 1.04` |
| `npc_toy_city_van.glb` | `NPC_Toy_City_Van` | 1.379 × 2.270 × 1.200 | 1,196 | 15 | Preserve current `0.50 × 1.00`, or use `0.55 × 1.04` |
| `npc_toy_hatchback.glb` | `NPC_Toy_Hatchback` | 1.329 × 2.188 × 1.015 | 1,208 | 14 | Preserve current `0.50 × 1.00`, or use `0.53 × 0.98` |
| `npc_toy_pickup.glb` | `NPC_Toy_Pickup` | 1.409 × 2.350 × 1.060 | 1,228 | 14 | `0.57 × 1.08` if collision should match the longer body |
| `npc_toy_suv.glb` | `NPC_Toy_SUV` | 1.429 × 2.333 × 1.155 | 1,288 | 14 | `0.57 × 1.06` if collision should match the wider body |

The larger bounds include tires, hubs, and small bumper/detail overhangs. The painted bodies remain close to the current `1 × 2` gameplay footprint.

## Current code context

`src/difficulty.rs` currently supports only:

```rust
TrafficKind::Sedan
TrafficKind::Van
```

It procedurally creates body, cabin, windshield, lamps, wheels, hubs, and shadow meshes. Traffic movement, collider behavior, recycling, difficulty scaling, and wheel animation are already correct and should remain on the procedural traffic root.

## Integration tasks

### 1. Extend deterministic visual variants

Add variants corresponding to:

```rust
Sedan
CityVan
Hatchback
Pickup
Suv
```

Extend `traffic_kind(seed)` without consuming another LCG value. Keep visual selection derived from the existing seed/hash so vehicle appearance cannot perturb movement, lane, speed, or spawn rolls.

### 2. Preload scene handles once

Create a traffic visual asset resource loaded once at startup:

```rust
asset_server.load("models/traffic/toy/npc_toy_sedan.glb#Scene0")
```

Do not repeatedly call `AssetServer::load` for every recycled car if handles can be cloned from a resource.

### 3. Preserve the procedural gameplay root

Keep these on the existing top-level traffic entity:

- `Traffic`
- `Collider`
- movement transform and heading
- speed/speed-roll data
- recycling/despawn behavior
- difficulty/modifier/event behavior

Spawn the chosen GLB as a **visual child** of that root. Remove the old procedural body/cabin/window/lamp visual children when using a GLB.

### 4. Wheel animation hookup

Each GLB contains four variant-specific pivot nodes:

```text
<asset_id>_Wheel_FL
<asset_id>_Wheel_FR
<asset_id>_Wheel_RL
<asset_id>_Wheel_RR
```

Examples:

```text
npc_toy_sedan_Wheel_FL
npc_toy_pickup_Wheel_RR
```

Each pivot is at the wheel center and owns its tire/hub children. After asynchronous scene instantiation:

1. Find these entities by Bevy `Name`.
2. Attach a lightweight traffic-wheel marker or map them to the traffic owner.
3. Rotate the pivot around local `X` using the existing distance/radius rule.

All variants use wheel radii near `0.18–0.20`; either retain one stylized spin radius or record the per-variant value from `manifest.json`.

The current procedural `TrafficWheel` query assumes direct wheel children of the traffic root. Imported wheel nodes are deeper under a scene root, so update ownership lookup accordingly rather than assuming direct `ChildOf<Traffic>`.

### 5. Toy paint color variation

Every asset uses the material name `Toy_Paint`.

Options:

- keep the embedded glossy red for the first integration;
- clone the imported `StandardMaterial` per traffic entity and set `base_color` from the existing deterministic five-color palette;
- or create authored color variants later.

Do not mutate one shared material handle globally unless all traffic cars should change color together.

The authored material is intentionally toy-like: glossy saturated paint, metallic component, low roughness, and clearcoat. Preserve those properties when recoloring.

### 6. Lights and glass

- Windows are complete on all four sides and use opaque `Toy_Glass`.
- Headlights and taillights are separate named material groups.
- No real point/spot lights are included.
- Leave emissive material behavior lightweight for WebGL2.

### 7. Collider decision

For a drop-in visual replacement, retain the existing traffic collider:

```text
half width = 0.5
half length = 1.0
```

This keeps gameplay unchanged despite visual tire/bumper overhang.

If collider fidelity is preferred, use the suggested table values and preserve the existing axis-dependent half-extent swap when traffic drives along world X versus world Z.

## Suggested implementation order

1. Integrate sedan and city van while keeping the existing two-kind probabilities.
2. Verify model forward, ground contact, scene lifecycle, and wheel spin.
3. Add hatchback, pickup, and SUV to deterministic selection.
4. Add per-instance `Toy_Paint` recoloring.
5. Tune colliders only after driving/collision review.
6. Delete now-unused procedural traffic mesh/material handles after GLBs are stable.

## Validation checklist

- All five `#Scene0` labels load.
- Vehicle nose points along root local `-Z` in Bevy.
- Tire bottoms touch road at local `Y = 0`.
- All four side windows are visible.
- Wheel pivots rotate around local `X` without orbiting.
- Scene descendants despawn recursively when traffic recycles.
- Visual selection does not advance movement RNG.
- Existing traffic counts, speeds, lanes, collisions, and difficulty tests remain unchanged.
- Test both road axes and both travel directions.
- Test WebGL2 with maximum traffic population (`MAX_TRAFFIC = 8`).

## Authored distinctions

- Sedan: low three-box roof and separate rear deck
- City van: tall squared cargo greenhouse and rear-door seam
- Hatchback: compact greenhouse, rear hatch glass, and spoiler
- Pickup: separate cab and genuinely open bed
- SUV: tall greenhouse, roof rails, running-board cladding, and rear spare tire

## Included metadata

The asset directory also contains:

- `manifest.json` — source topology, bounds, wheel pivots, and materials
- `audit.json` — direct glTF structure and semantic-node validation

Authoring and four-angle review files remain in the recovery worktree under `review/npc-cars/`.
