# Roady Car World Generation and Visual Review Plan

**Status:** diagnosis complete; implementation pending.
**Updated:** 2026-07-13

## 1. Confirmed root cause

The current rendered world cannot produce true L-turns, T-junctions, or dead ends even though `TileKind` names and socket tests include them.

Current topology uses deterministic infinite road-line decisions:

```text
W/E = selected vertical world lines
S/N = selected horizontal world lines
```

A selected line remains selected for every coordinate along that axis. Lines therefore never turn or terminate. Every selected horizontal/vertical crossing is visually a plus.

Current rendering compounds the semantic mismatch: each Road socket spawns a full 40-unit road along the corresponding block boundary, not a half-road arm from the edge to the block center. Adjacent blocks double-spawn coplanar road surfaces on shared boundaries.

## 2. Why terrain looks uniform

- `LINE_ROAD_DENSITY = 0.7` heavily concentrates Cross and T logical socket combinations.
- Rural eligibility requires all four lines to be absent: approximately `0.3^4 = 0.81%` of blocks.
- That rare all-None set is divided among Empty, Park, Field, and Orchard.
- Approximately 99.4% of blocks use the same urban branch: grass, three buildings, trees, lamps, and obstacles.
- The deterministic nearest Orchard is around 141 units from spawn; nearest Park around 962; nearest Field around 1,078.
- Spawn forces both zero lines to roads, guaranteeing a plus intersection.
- The fixed camera initially shows less than one 40-unit block.

Existing reachability tests prove logical socket variants over large coordinate ranges. They do not prove distinct production-mesh silhouettes or visible distribution near spawn.

## 3. Required topology model

Replace infinite lines with deterministic shared edge segments:

```text
W = vertical_edge(gx, gz)
E = vertical_edge(gx + 1, gz)
S = horizontal_edge(gx, gz)
N = horizontal_edge(gx, gz + 1)
```

Each edge decision hashes both coordinates and is shared exactly by its two adjacent cells. This allows roads to turn and terminate while preserving local continuity.

Do not use independent `p=0.7` as the final connectivity policy. Start with a measured candidate around 0.5, then enforce a deterministic spawn backbone/connectivity policy so the player is not trapped in tiny components.

Store selected `TileKind` on `Block`; do not recompute diagnostics from an algorithm that may later change.

## 4. Required road rendering

Sockets mean “a road enters from this side.” Render:

- one central junction pad;
- one 8-unit-wide half-block arm from center to each Road socket;
- exposed-side curbs;
- end caps for stubs;
- center/edge lane markings appropriate to straight, corner, T, Cross, and stub silhouettes;
- one authoritative surface per cell/segment with no coplanar duplicate ownership.

Expected silhouettes:

- Cross: 4 arms
- T: 3 arms
- Corner/L: 2 adjacent arms
- Straight: 2 opposite arms
- Stub: 1 arm
- all-None: no asphalt

Urban prop/building placement must use center-arm exclusion geometry rather than old boundary-road margins.

## 5. District and terrain diversity

Decouple visual district selection from accidental all-None topology.

Introduce deterministic district/biome fields with explicit target frequencies and patch correlation:

- dense city blocks;
- low-rise commercial/residential blocks;
- parks;
- fields/farms with furrows, fences, hay bales, crates/sheds;
- orchards;
- ponds/water parks;
- future biome themes.

Road topology and district selection are separate deterministic domains. District generation may reserve patches, constrain roads around ponds/farms, or select road-compatible visual variants.

Initial acceptance targets must be stated and tested over representative windows. Do not tune by screenshot impression alone.

## 6. Different city-block types

Add bounded deterministic urban visual families while reusing cached meshes/materials:

- tall downtown tower block;
- low-rise mixed commercial block;
- residential houses/yards;
- parking/service block;
- construction/storage block;
- plaza/landmark block.

Each family declares placement footprints, collider footprints, maximum entity count, road-facing entrances, and occlusion-height budget.

## 7. Pond and water roadmap

Ponds are a subsequent milestone after topology and capture tooling.

Required stages:

1. deterministic pond/shore geometry and collider/driveability contract;
2. improved water material/shader with reduced-motion behavior;
3. visual/readability review in overhead and isometric captures;
4. fair pond-entry warning and collision semantics;
5. future death/wreck-by-falling-in-pond behavior with explicit tests and GameOver reason.

Do not add death behavior before water boundaries are visually clear and collision geometry is validated.

## 8. Deterministic top-down production render harness

Create a test/debug mode that renders actual production meshes/materials.

### World atlas capture

- Direct overhead orthographic camera.
- Fixed lighting, clear color, resolution, seed, topology version, and scale.
- Fixed 11x11 or 21x21 coordinate window.
- Disable HUD, follow camera, shake/zoom, moving entities, timer, recycling, and time-dependent animation.
- Wait for asset loading and at least two transform/render frames.
- Capture PNG and sidecar JSON.

Sidecar JSON per block:

```text
gx, gz
TileKind
W/E/S/N sockets
district/biome
road mesh count
curb/marking count
building/tree/farm/pond prop counts
```

### Forced topology atlas

Render every TileKind once in a labeled deterministic atlas, independent of generator frequency. Verify each silhouette visually and arithmetically.

### Automated invariants

- asphalt exists at each socket midpoint;
- asphalt connects to center for every Road socket;
- no asphalt at None socket midpoint;
- expected center occupancy by topology;
- one road surface owner per intended segment;
- no duplicate coplanar roads;
- adjacent sockets agree;
- representative distribution bounds;
- deterministic PNG metadata/JSON across reruns (pixel-perfect image equality is optional and renderer-dependent).

## 9. Image-driven review loop

For each generation wave:

1. implement one bounded topology/visual stage;
2. run pure distribution/continuity/geometry tests;
3. capture forced atlas and representative world PNG+JSON;
4. send images and metadata to a separate harsh visual reviewer;
5. revise once per concrete defect;
6. repeat until topology readability, diversity, and clutter pass;
7. run normal isometric desktop/mobile browser screenshots;
8. commit only after native/WASM/release/browser gates pass.

## 10. Milestones

### W0 - capture harness

No generator change. Add deterministic overhead and forced-atlas captures plus JSON metadata.

### W1 - edge-segment topology

Replace line decisions with shared coordinate-pair edge decisions and retain `TileKind` on blocks.

### W2 - center-connected production roads

Render arm/junction/curb/marking geometry and remove duplicate boundary planes.

### W3 - connectivity and distribution

Add spawn backbone/component policy and measured topology distribution gates.

### W4 - district frequency and urban families

Make fields, farms, parks, orchards, and varied city blocks visible at stated rates near normal play.

### W5 - ponds and improved water

Add pond production geometry and shader, then separately implement fair hazard/death behavior.

## 11. Release gates

- deterministic edge continuity and topology catalog tests;
- forced-atlas geometry invariants;
- representative distribution tests;
- field/orchard/fence/hay/pond footprint tests;
- bounded entity counts and asset reuse;
- overhead PNG+JSON harsh review;
- desktop 1440x900 and mobile 844x390/960x480 review;
- full native/WASM/release/browser QA;
- production audit.
