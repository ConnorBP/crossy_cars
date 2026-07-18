# Roady Car Seamless Tile Blending Plan

Status: queued, not implemented. Disabled by default.
Baseline: production `7c6e45511736078f48e724822625cd52bc03176f`.
Source brief: `C:\Users\connor\Downloads\seamless-tile-blending-bevy.md` (read in full 2026-07-18).

## Objective

Hide distracting boundaries between compatible procedural ground families while preserving Roady's tile topology, authoritative gameplay roots, roads, routing, colliders, finite recycling, material identities, toy-town scale, WebGL2 support, and deterministic asset bounds.

This is a separate visual wave. It must not alter gameplay generation or resurrect the rejected broad-normal/warped-PBR approach.

## Non-negotiable constraints

- Bevy 0.19 and WebGL2; verify APIs against installed 0.19 sources rather than copying the source brief's 0.14–0.16 snippets.
- Preserve block topology, road sockets, lane connectors, routing, colliders, ponds, building pads, world coordinates, and recycling ownership.
- Preserve authored GLB materials and protected glass, metal, emissive, traffic, and player materials.
- No runtime pairwise world scans, per-block mesh/material asset allocation, unbounded control textures, or raw high-resolution PBR sources.
- Use cached deterministic derivatives and globally plateauing caches.
- No generated broad normal maps, directional furrows, parallax, silhouette deformation, dark fringes, shimmer, or texture-scale drift.
- Keep production on ordinary `StandardMaterial` until an isolated shader prototype independently proves safe.

## Stage 0 — freeze and inventory

1. Capture fixed-camera baselines for urban, park, farm, orchard, pond, sidewalk, curb, and road boundaries.
2. Inventory terrain mesh/material ownership in:
   - `src/world.rs`
   - `src/textures.rs`
   - `src/toy_shading.rs`
3. Record current entity, mesh, image, material, draw-call, startup, frame-time, and WASM-size baselines.
4. Define exact exclusions and compatible-family groups before implementation.

Acceptance: documented baseline with no code-path ambiguity about ownership or cleanup.

## Stage 1 — cached CPU edge/corner overlays

First implementation task:

> Prototype cached CPU terrain edge/corner overlays in isolated seam-review mode.

### Design

- Derive deterministic cardinal and corner masks from neighboring block terrain families.
- Freeze a precedence table for multi-family junctions.
- Initially blend only compatible soft-ground pairs:
  - grass ↔ soil
  - grass ↔ park
  - soil ↔ farm
- Explicitly exclude:
  - roads and markings
  - curbs and sidewalks
  - ponds, shore rocks, reeds, and water
  - building pads and authored buildings
  - props, traffic, creatures, pickups, and player surfaces
- Generate a small catalog of edge/corner ribbon meshes slightly above the owning terrain plane.
- Reuse existing small isotropic albedo/ORM derivatives through ordinary `StandardMaterial`.
- Create global cached mesh/material handles. Block spawning may select handles but must not create assets.
- Parent overlays beneath each authoritative streamed block root so despawn/recycling cleanup remains automatic.
- Resolve shared boundaries from deterministic coordinate-pair data so adjacent blocks agree, including negative coordinates.

### Required tests

- Cardinal/corner mask exhaustiveness and determinism.
- Shared-edge agreement for positive and negative coordinates.
- Frozen precedence behavior at three- and four-family corners.
- Exact compatible/excluded family tables.
- Bounded overlays per block and per streamed window.
- Global mesh/material cache plateau.
- No stationary-frame asset/entity growth.
- Exact recursive cleanup during normal recycling, teleport retarget, and fresh reset.
- No changes to topology, socket, lane, collider, pond, or building-placement signatures.

### Visual/performance review

Add an isolated seam-review mode with a fixed camera atlas and feature-off/on A/B captures. Require:

- no z-fighting or depth flicker;
- no black/bright boundary lines;
- no texture stretching or directional streaks;
- no silhouette or collider changes;
- no road, curb, sidewalk, pond, or pad contamination;
- stable material scale and identity;
- bounded draw calls/entities and acceptable native/WebGL frame time.

## Stage 2 — tiny deterministic transition masks

Proceed only if Stage 1 is mechanically correct but visually too geometric.

- Generate or author a small versioned edge/corner mask set.
- Masks must be low-resolution, seamless on the required axis, isotropic, deterministic, and reproducible.
- Apply masks to color and roughness/AO only.
- Do not add generated terrain normals.
- Keep the same cached overlays, precedence rules, ownership, exclusions, and cleanup.

Acceptance requires pixel hashes, source review, visual review, and unchanged cache bounds.

## Stage 3 — isolated Bevy 0.19 shader prototype

Deferred and unapproved until Stage 1 evidence is reviewed.

- Use a dedicated worktree and review-only startup mode.
- Verify Bevy 0.19 `ExtendedMaterial`, bind-group, WebGL2, and shader-import behavior from installed sources/examples.
- Start with exactly two layers and albedo+ORM only.
- Use nearest index sampling or `textureLoad`; never interpolate terrain indices.
- Keep control data bounded to the isolated review chunk.
- Prohibit prepass, SSAO, storage-texture, compute, or unsupported WebGL2 dependencies.
- Require identical non-transition appearance with the feature disabled and outside blend bands.
- Prove delayed-load behavior, recycling cleanup, cache plateau, and WebGL2 stability before considering production integration.

Production `StandardMaterial` must not be replaced merely because the review shader compiles.

## Deferred experiments

These each require separate approval and evidence:

- height-based blending;
- normal blending;
- texture arrays;
- stochastic/hex sampling;
- Wang variants;
- parallax.

Height work may use only deterministic small derivatives with registered physical scale. Broad generated normals and parallax remain prohibited by default. Never repeat the rejected apartment `ConcreteMicro` replacement or broad warped-PBR strategy on terrain.

## Promotion gates

1. Exact feature-off/on A/B captures.
2. Independent source and image review.
3. Full Rust/workspace and frozen-artifact tests.
4. Native and optimized WASM builds.
5. Desktop, touch, Settings, audit, request-failure, category-isolation, and menu browser suites.
6. WebGL2 console/page/network audit.
7. Entity/asset/cache plateau and recycling tests.
8. Startup, frame-time, draw-call, and WASM-size comparison.
9. Clean commit, exact-SHA CI, disabled-first deployment, and canonical production verification.
10. Disable/rollback immediately for warping, shimmer, black seams, material identity loss, unbounded resources, or gameplay ownership changes.

## Queue

- [ ] Stage 0: capture baseline and ownership inventory.
- [ ] Stage 1: prototype cached CPU terrain edge/corner overlays in isolated seam-review mode.
- [ ] Stage 1 tests and fixed-camera A/B atlas.
- [ ] Independent Stage 1 review and go/no-go decision.
- [ ] Stage 2 masks only if Stage 1 needs softer shapes.
- [ ] Stage 3 shader prototype remains deferred/unapproved.
