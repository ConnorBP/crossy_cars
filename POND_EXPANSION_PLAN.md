# Pond and Terrain Expansion Plan

Status: implementation follows the atomic W1/W2 road-topology wave.

## Scope boundary

Todo #12 adds deterministic terrain districts and visually readable ponds. Ponds are non-hazardous in this wave:

- no `Collider`;
- no `Curb`;
- no `ObstacleHit`;
- no speed, health, score, or Game State changes;
- the car may cross water without gameplay consequences.

Todo #14 later adds a dedicated pond-entry/hazard event and terminal reason. It must not reuse generic obstacle collision, which would incorrectly report pond entry as `Wrecked`.

## Prerequisite

Land and stabilize W1/W2 first. Pond integration depends on:

- `Block` retaining authoritative topology kind;
- bounded `is_road_edge` segments;
- pure `RoadSurfacePlan` pad/arm geometry;
- road-aware traffic/chicken placement;
- props/coins excluded from center-connected asphalt.

Do not implement ponds against infinite `is_road_line` or legacy boundary-road margins.

## District layer

Topology and terrain must be independent. Add:

```rust
enum District {
    DenseUrban,
    LowRise,
    Park,
    Field,
    Orchard,
    WaterPark,
}
```

Store both topology kind and district on `Block`; review metadata serializes stored values rather than recomputing them.

Initial target frequencies:

| District | Target |
|---|---:|
| DenseUrban | 30% |
| LowRise | 28% |
| Park | 14% |
| Field | 12% |
| Orchard | 10% |
| WaterPark | 6% |

Use domain-separated hashes and 4x4 macro-cell correlation so districts form readable patches without altering road sockets. Spawn-adjacent backbone cells deterministically remap away from WaterPark.

## Pond families

```rust
enum PondFamily {
    GardenOval,
    ReedMarsh,
    FarmReservoir,
}
```

WaterPark mix:

- GardenOval: 50%
- ReedMarsh: 30%
- FarmReservoir: 20%

Suggested footprints:

- GardenOval: 5.0x3.5 radii, pale 1.25-unit shore.
- ReedMarsh: 6.5x4.0, 1.5-unit mud/grass shore, at most 12 reeds.
- FarmReservoir: 7.0x3.0, geometric earth/stone bank, at most 6 edge props.

Use cached low-poly unit meshes scaled/rotated per layout, not unique meshes per block.

## Pure placement contract

Add a deterministic `PondFootprint` and helpers for:

- rotated ellipse containment;
- conservative AABB;
- road-pad/arm clearance;
- block-boundary clearance;
- ordered candidate selection;
- pond-family selection.

Expand candidate footprint by shore width, two units of road clearance, and the visible car half-footprint plus 0.5 units. Reject intersections with W2 asphalt. If no candidate fits, render a pondless Park-style fallback while retaining WaterPark metadata with `ponds: 0`.

Register the expanded pond footprint before placing trees, benches, buildings, lamps, farm props, or obstacles. Pickups, chickens, and critters use fixed-retry point rejection; traffic needs no pond behavior when roads clear ponds.

## Water material

Replace the fixed standalone pond at `(30, 0.03, -10)` with streamed block-owned water.

Split material registration from full atmosphere behavior so review mode can register water materials without gameplay sky systems. Use vec4-aligned WebGL2-safe uniforms:

```rust
struct WaterMaterial {
    deep: LinearRgba,
    shallow: LinearRgba,
    motion: Vec4, // time, amplitude, frequency, speed
    detail: Vec4, // family seed, foam, contrast, reserved
}
```

Create exactly three shared material handles. The fragment shader uses two low-amplitude UV waves, bounded deep/shallow color mixing, and a narrow static edge highlight. Keep opaque alpha; avoid compute, storage buffers, texture arrays, vertex displacement, high-frequency shimmer, and transparent sorting.

Reduced Motion freezes phase at zero immediately. Shore silhouette, reeds/rocks, and value contrast must communicate water without animation or blue hue alone.

## Review metadata and images

Bump world review schema to v2 when district/pond fields land. Add actual spawned counts:

- ponds;
- pond shores;
- pond props;
- district;
- optional pond family.

Add a forced pond atlas containing all three families, a road-adjacent clearance case, and a no-fit fallback. Increase representative production review to 11x11 so a 6% pond target is visible.

Capture modes:

- representative world;
- topology atlas;
- pond atlas.

Review native/WebGL2 at 1440x900, 844x390, and 960x480, normal and Reduced Motion, under each atmosphere modifier.

## TDD order

1. District determinism/frequencies and patch correlation.
2. Pond family determinism/frequencies.
3. Rotated footprint containment/AABB.
4. Road and block clearance for each topology/family.
5. Dedicated WaterPark branch emits no overlapping urban/farm props.
6. Pond entities carry neither Collider nor Curb.
7. Pickup/chicken/critter fixed-retry rejection.
8. Water uniform alignment, bounded parameters, and Reduced Motion freeze.
9. Metadata v2 determinism and actual entity counts.
10. Native tests, WASM check, release build, map atlas, browser QA.

Acceptance bounds:

- districts within +/-2 percentage points over a large sample;
- WaterPark 4-8% in representative samples;
- family mix within +/-3 points of 50/30/20 over a large sample;
- one water and one shore per emitted pond;
- no pond/road or pond/prop overlap;
- all pond entity budgets respected;
- no gameplay event or health/state change from pond crossing;
- all families distinguishable by silhouette/edge dressing in static reduced-motion images.

## Todo #14 follow-up

Later hazard work should introduce `PondHazard`/`PondEntered`, fair entry warning semantics, and a distinct Game Over reason. A new terminal reason requires coordinated UI, leaderboard canonicalization/validation, signing tests, and likely backend schema work.
