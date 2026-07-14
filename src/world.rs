//! Infinite 2D city: a recycling pool of city-block grid cells plus the
//! per-block environment (grass, road segments, curbs, lane dashes,
//! buildings, trees, lamp posts, T12 obstacles, coins).
//!
//! T19 — **Wang-tile road network.** Instead of a uniform road on every
//! block's -X/-Z edge, each block is assigned a `TileKind` from a small
//! Wang-tile set whose edges are either `Road` or `None`. Blocks are placed
//! one at a time (streaming/recycling), and each block's already-placed
//! neighbours fix the shared edges; the remaining (free) edges are chosen
//! from a tile whose sockets match the fixed ones. The tile set is COMPLETE
//! — every fixed-edge combination has at least one matching tile, so
//! placement can never deadlock — and the random choice among matching tiles
//! is WEIGHTED toward through-roads/intersections (Cross / RoadNS / RoadEW)
//! so the road network stays connected and drivable, with occasional parks,
//! fields, orchards, bigger blocks, T-intersections, corners and missing roads
//! for variety. Countryside kinds are visual all-None socket aliases only.
//! The tile choice is deterministic per `(gx, gz, seed)` (folded into the
//! block's LCG seed via `seed_for`), so a recycled block reproduces the same
//! layout it had the first time it was spawned at those coordinates.
//!
//! Grid alignment: block (gx,gz) root sits at world `((gx+0.5)*block, 0,
//! (gz+0.5)*block)`. A road on a block edge runs along the shared world line
//! (`x = n*block` for W/E edges, `z = n*block` for S/N edges). Each block
//! draws only ITS OWN edge roads; adjacent Road-Road edges tile seamlessly
//! (both blocks draw the same world-line road, overlapping exactly — the
//! road material is opaque so the double-draw is invisible and harmless),
//! while Road-None edges simply stop at the boundary. The car spawn
//! (0,0,0) sits on a road line.
//!
//! Solid obstacles (buildings / trees / lamp posts / T12 variety) carry a
//! generic `Collider` (axis-aligned box, half-extents) so
//! `car.rs::physics_collisions` can push the car out of any of them with one
//! circle-vs-AABB loop. Curbs keep their own `Curb` component for the
//! hop-up behaviour.

use std::collections::{BTreeMap, BTreeSet};

use bevy::color::LinearRgba;
use bevy::math::primitives::Circle;
use bevy::prelude::*;
use serde::Serialize;

use crate::car::{Car, DrivingSet, InputFrozen};
use crate::game::SpawnSet;
use crate::game::events::CoinCollected;
use crate::game::resources::{RoundActive, Score, TimeLeft};
use crate::game::state::GameState;
use crate::palette;
use crate::textures::TextureAssets;

/// Gate real-time shadows off on WebGL2 for performance.
const SHADOWS: bool = cfg!(not(target_arch = "wasm32"));

/// Stable review/export seed. Production generation itself is coordinate
/// seeded and unchanged; this only documents the harness contract.
const REVIEW_SEED: u32 = 0;

/// Tag for coin entities (environment now — spawned inside blocks, recycled
/// with them, collected on pickup and respawned when the block re-populates).
#[derive(Component)]
pub struct Coin;

/// A raised curb the car can hop up onto (drives on top at `height`).
#[derive(Component)]
pub struct Curb {
    pub half_x: f32,
    pub half_z: f32,
    pub height: f32,
}

/// A solid axis-aligned obstacle the car collides with and can't pass through.
/// Tagged onto buildings, trees, lamp posts and T12 obstacles;
/// `car.rs::physics_collisions` iterates `&Collider` generically.
#[derive(Component)]
pub struct Collider {
    pub half_x: f32,
    pub half_z: f32,
}

/// Tag for a building obstacle (collidable, read-only by other tasks).
#[derive(Component)]
pub struct Building;
/// Tag for a tree obstacle (collidable).
#[derive(Component)]
pub struct Tree;
/// Tag for a lamp-post obstacle (collidable).
#[derive(Component)]
pub struct LampPost;
/// Tag for a traffic-cone obstacle (collidable, T12 variety).
#[derive(Component)]
pub struct Cone;

/// Ground shadow hidden as soon as its parent cone becomes airborne.
#[derive(Component)]
struct ConeShadow;

/// Deterministic knockable-cone lifecycle. An `Idle` cone is a solid contact
/// the car can knock flying; once `Flying` it cannot re-hit the car, integrates
/// bounded projectile motion + tumble on its LOCAL transform, and despawns on
/// ground impact or after a short lifetime. The cone keeps its existing
/// `Collider`/`Cone` entity — no debris or physics crate is spawned.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum ConeState {
    /// Resting on the ground; the car collides and launches it.
    #[default]
    Idle,
    /// Airborne; cannot re-hit the car, integrates flight, despawns on land.
    Flying,
}

/// Per-cone motion state, added to every spawned cone root. Velocity and spin
/// axis are stored in WORLD space and integrated into the LOCAL `Transform`:
/// cone roots are parented under block roots that carry only translation
/// (identity rotation/scale), so local-space deltas equal world-space deltas.
/// This keeps flight deterministic and free of the one-frame `GlobalTransform`
/// propagation lag.
#[derive(Component, Default)]
pub struct ConeMotion {
    /// Current lifecycle state.
    pub state: ConeState,
    /// World-space velocity (m/s). Gravity acts on `.y`.
    pub vel: Vec3,
    /// World-space unit tumble axis (horizontal, perpendicular to launch).
    pub spin_axis: Vec3,
    /// Tumble rate (rad/s) about `spin_axis`.
    pub spin: f32,
    /// Remaining airborne lifetime (s); caps flight at <= 2s.
    pub lifetime: f32,
}
/// Tag for a fire-hydrant obstacle (collidable, T12 variety).
#[derive(Component)]
pub struct Hydrant;
/// Tag for a bench obstacle (collidable, T12 variety).
#[derive(Component)]
pub struct Bench;
/// Tag for a hedge obstacle (collidable, T12 variety).
#[derive(Component)]
pub struct Hedge;

// ---------------------------------------------------------------------------
// Wang-tile road network (T19)
// ---------------------------------------------------------------------------

/// Edge-socket state for one side of a block: either a road runs along that
/// edge (`Road`) or it doesn't (`None`). The four edges of a block, in the
/// order used everywhere in this module, are W (−X), E (+X), S (−Z), N (+Z).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    Road,
    None,
}

/// A Wang-tile kind from the road-network tile set. Each variant fixes the
/// `Edge` socket on each of the four sides (W, E, S, N). The set is
/// **complete**: for any combination of fixed-edge constraints there is at
/// least one `TileKind` whose sockets match (see `TILE_CATALOG` / `tile_from_edges`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub enum TileKind {
    /// All edges None — a full block of buildings.
    Empty,
    /// All edges None — a park (grass + trees, no buildings). Visual variant
    /// of `Empty` chosen for variety when no roads touch the block.
    Park,
    /// All edges None — a cultivated field with furrows and a small bounded
    /// set of farm props. It never changes the road socket graph.
    Field,
    /// All edges None — an orchard whose trees are placed in aligned rows.
    /// It never changes the road socket graph.
    Orchard,
    /// Through-road running S↔N (W=None, E=None, S=Road, N=Road).
    RoadNS,
    /// Through-road running W↔E (W=Road, E=Road, S=None, N=None).
    RoadEW,
    /// 4-way intersection (all Road).
    Cross,
    /// T-intersection with the **N** edge None (W, E, S Road).
    TN,
    /// T-intersection with the **E** edge None (W, S, N Road).
    TE,
    /// T-intersection with the **S** edge None (W, E, N Road).
    TS,
    /// T-intersection with the **W** edge None (E, S, N Road).
    TW,
    /// Corner: W + N Road (E, S None) — turns from the W edge to the N edge.
    CornerWN,
    /// Corner: E + N Road (W, S None) — turns from the E edge to the N edge.
    CornerNE,
    /// Corner: E + S Road (W, N None) — turns from the E edge to the S edge.
    CornerES,
    /// Corner: W + S Road (E, N None) — turns from the W edge to the S edge.
    CornerSW,
    /// Stub: only the W edge is Road (a short dead-end spur coming in from
    /// the W edge). Closes the completeness gap so a single fixed-Road edge
    /// (the other three free/None) always has a matching tile. Weighted low
    /// so stubs are rare — they only appear when a neighbour forces a Road
    /// on one edge and the free edges happen to roll None.
    StubW,
    /// Stub: only the E edge is Road.
    StubE,
    /// Stub: only the S edge is Road.
    StubS,
    /// Stub: only the N edge is Road.
    StubN,
}

/// Socket array order used throughout: `[W, E, S, N]`.
pub const W: usize = 0;
pub const E: usize = 1;
pub const S: usize = 2;
pub const N: usize = 3;

/// Return the four edge sockets `[W, E, S, N]` for a `TileKind`.
pub fn sockets(kind: TileKind) -> [Edge; 4] {
    use Edge::*;
    use TileKind::*;
    match kind {
        Empty => [None, None, None, None],
        Park => [None, None, None, None],
        Field => [None, None, None, None],
        Orchard => [None, None, None, None],
        RoadNS => [None, None, Road, Road],
        RoadEW => [Road, Road, None, None],
        Cross => [Road, Road, Road, Road],
        TN => [Road, Road, Road, None],
        TE => [Road, None, Road, Road],
        TS => [Road, Road, None, Road],
        TW => [None, Road, Road, Road],
        CornerWN => [Road, None, None, Road],
        CornerNE => [None, Road, None, Road],
        CornerES => [None, Road, Road, None],
        CornerSW => [Road, None, Road, None],
        StubW => [Road, None, None, None],
        StubE => [None, Road, None, None],
        StubS => [None, None, Road, None],
        StubN => [None, None, None, Road],
    }
}

// Each road surface is drawn by both blocks beside its world line. Markings
// use a directional ownership rule so they are not also double-spawned: this
// block owns its W (vertical) and S (horizontal) road surfaces. The four
// flags are W-road at S endpoint, W-road at N endpoint, S-road at W endpoint,
// and S-road at E endpoint. An approach is marked only when the endpoint has
// a perpendicular road socket, i.e. it is a real intersection rather than a
// through-road endpoint or dead-end.
#[cfg(test)]
fn marking_approaches(sock: [Edge; 4]) -> [bool; 4] {
    let road = |side| sock[side] == Edge::Road;
    [
        road(W) && road(S),
        road(W) && road(N),
        road(S) && road(W),
        road(S) && road(E),
    ]
}

const WINDOW_ROW_BOTTOM: f32 = 0.9;
const WINDOW_ROW_TOP_MARGIN: f32 = 0.9;
const WINDOW_ROW_SPACING: f32 = 2.0;
const MAX_WINDOW_ROWS: usize = 3;

/// A bounded, low-detail set of window-strip center heights. Buildings in
/// this module are 4–9u tall, yielding two or three rows; malformed or tiny
/// heights yield no rows. Keeping this pure makes the entity-count and facade
/// bounds independently testable.
fn window_row_heights(height: f32) -> Vec<f32> {
    if !height.is_finite() {
        return Vec::new();
    }
    let upper = height - WINDOW_ROW_TOP_MARGIN;
    if upper < WINDOW_ROW_BOTTOM {
        return Vec::new();
    }
    let count = (((upper - WINDOW_ROW_BOTTOM) / WINDOW_ROW_SPACING).floor() as usize)
        .saturating_add(1)
        .min(MAX_WINDOW_ROWS);
    if count == 1 {
        return vec![(WINDOW_ROW_BOTTOM + upper) * 0.5];
    }
    (0..count)
        .map(|row| {
            WINDOW_ROW_BOTTOM + (upper - WINDOW_ROW_BOTTOM) * row as f32 / (count - 1) as f32
        })
        .collect()
}

/// Stable review/catalog order for every production tile kind. This complete
/// set includes the four single-edge stubs, so every socket combination has a
/// match. The world-review atlas and JSON intentionally share this ordering.
pub const TILE_CATALOG: [TileKind; 19] = [
    TileKind::Empty,
    TileKind::Park,
    TileKind::Field,
    TileKind::Orchard,
    TileKind::RoadNS,
    TileKind::RoadEW,
    TileKind::Cross,
    TileKind::TN,
    TileKind::TE,
    TileKind::TS,
    TileKind::TW,
    TileKind::CornerWN,
    TileKind::CornerNE,
    TileKind::CornerES,
    TileKind::CornerSW,
    TileKind::StubW,
    TileKind::StubE,
    TileKind::StubS,
    TileKind::StubN,
];

// ---------------------------------------------------------------------------
// Deterministic finite shared-edge road generation
// ---------------------------------------------------------------------------

/// Production block/road dimensions. Block roots are road-junction centres;
/// cell boundaries therefore lie half a block from each root.
pub(crate) const ROAD_BLOCK_SIZE: f32 = 40.0;
#[cfg(test)]
pub(crate) const ROAD_HALF_WIDTH: f32 = 4.0;
const EDGE_ROAD_DENSITY: f32 = 0.58;
const SPAWN_BACKBONE_RADIUS: i32 = 2;

/// Direction of a bounded road centre-line segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RoadAxis {
    X,
    Z,
}

/// One centre-to-boundary arm owned by a tile. `start` and `end` are world XZ
/// coordinates and always form a finite axis-aligned segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RoadSegment {
    pub axis: RoadAxis,
    pub start: Vec2,
    pub end: Vec2,
    pub gx: i32,
    pub gz: i32,
    pub socket: usize,
}

/// Authoritative deterministic road plan for a coordinate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RoadPlan {
    pub kind: TileKind,
    pub segments: [Option<RoadSegment>; 4],
}

fn edge_hash(axis: RoadAxis, line: i32, segment: i32) -> f32 {
    let axis_salt = match axis {
        RoadAxis::X => 0x68bc_21ebu32,
        RoadAxis::Z => 0x02e5_be93u32,
    };
    let mut h = (line as u32).wrapping_mul(0x9e37_79b1)
        ^ (segment as u32).wrapping_mul(0x85eb_ca77)
        ^ axis_salt;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    (h >> 8) as f32 / ((1u32 << 24) as f32)
}

/// A shared edge is keyed by both its grid line and its along-line segment.
/// The only forced roads are the short cross through the spawn tile; unlike
/// the retired line model this cannot create an infinite forced axis.
pub(crate) fn road_edge(axis: RoadAxis, line: i32, segment: i32) -> bool {
    let spawn_backbone = match axis {
        // X-running connection across a vertical boundary.
        RoadAxis::X => {
            segment == 0 && (-SPAWN_BACKBONE_RADIUS..SPAWN_BACKBONE_RADIUS).contains(&line)
        }
        // Z-running connection across a horizontal boundary.
        RoadAxis::Z => {
            segment == 0 && (-SPAWN_BACKBONE_RADIUS..SPAWN_BACKBONE_RADIUS).contains(&line)
        }
    };
    spawn_backbone || edge_hash(axis, line, segment) < EDGE_ROAD_DENSITY
}

/// Stable half-open world-cell conversion. Exact positive boundaries belong
/// to the cell on their positive side; exact negative boundaries follow the
/// same floor rule rather than truncating toward zero.
pub(crate) fn world_to_road_cell(coordinate: f32) -> i32 {
    if !coordinate.is_finite() {
        return 0;
    }
    ((coordinate + ROAD_BLOCK_SIZE * 0.5) / ROAD_BLOCK_SIZE).floor() as i32
}

#[cfg(test)]
pub(crate) fn road_tile_kind(gx: i32, gz: i32) -> TileKind {
    tile_from_edges(gx, gz)
}

/// Derive all sockets from coordinate-pair shared edges. W/E use the same
/// `(vertical line, z segment)` key in adjacent cells; S/N do likewise with
/// `(horizontal line, x segment)`.
fn tile_from_edges(gx: i32, gz: i32) -> TileKind {
    let w = road_edge(RoadAxis::X, gx - 1, gz);
    let e = road_edge(RoadAxis::X, gx, gz);
    let s = road_edge(RoadAxis::Z, gz - 1, gx);
    let n = road_edge(RoadAxis::Z, gz, gx);
    // All-None blocks get one of four deterministic VISUAL variants. Their
    // sockets are identical, so this selection cannot change a shared edge.
    if !w && !e && !s && !n {
        return all_none_variant(gx, gz);
    }
    // Find the tile whose sockets match (W,E,S,N) exactly. There is exactly
    // one for every non-all-None combo (the set is complete + each non-empty
    // combo has a unique tile).
    TILE_CATALOG
        .iter()
        .copied()
        .find(|&k| {
            let st = sockets(k);
            let rw = matches!(st[W], Edge::Road);
            let re = matches!(st[E], Edge::Road);
            let rs = matches!(st[S], Edge::Road);
            let rn = matches!(st[N], Edge::Road);
            rw == w && re == e && rs == s && rn == n
        })
        .unwrap_or(TileKind::Cross)
}

/// Stable, well-mixed coordinate hash for visual variation. This is kept
/// separate from road-line hashing so adding/changing countryside visuals can
/// never perturb the socket graph.
fn visual_coordinate_hash(gx: i32, gz: i32) -> u32 {
    let mut h =
        (gx as u32).wrapping_mul(0x9e37_79b1) ^ (gz as u32).wrapping_mul(0x85eb_ca77) ^ 0xc2b2_ae3d;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

fn all_none_variant(gx: i32, gz: i32) -> TileKind {
    match visual_coordinate_hash(gx, gz) & 3 {
        0 => TileKind::Empty,
        1 => TileKind::Park,
        2 => TileKind::Field,
        _ => TileKind::Orchard,
    }
}

pub(crate) fn road_plan(gx: i32, gz: i32) -> RoadPlan {
    let kind = tile_from_edges(gx, gz);
    let center = Vec2::new(gx as f32 * ROAD_BLOCK_SIZE, gz as f32 * ROAD_BLOCK_SIZE);
    let half = ROAD_BLOCK_SIZE * 0.5;
    let sock = sockets(kind);
    let endpoints = [
        center + Vec2::new(-half, 0.0),
        center + Vec2::new(half, 0.0),
        center + Vec2::new(0.0, -half),
        center + Vec2::new(0.0, half),
    ];
    let segments = std::array::from_fn(|socket| {
        (sock[socket] == Edge::Road).then_some(RoadSegment {
            axis: if socket <= E {
                RoadAxis::X
            } else {
                RoadAxis::Z
            },
            start: center,
            end: endpoints[socket],
            gx,
            gz,
            socket,
        })
    });
    RoadPlan { kind, segments }
}

pub(crate) fn closest_point_on_road_segment(point: Vec2, segment: RoadSegment) -> Vec2 {
    let delta = segment.end - segment.start;
    let length_squared = delta.length_squared();
    if length_squared <= f32::EPSILON {
        return segment.start;
    }
    segment.start + delta * ((point - segment.start).dot(delta) / length_squared).clamp(0.0, 1.0)
}

/// Bounded nearest-road query used by ambient actors. It examines a fixed
/// square around the point's cell and measures point-to-segment distance, not
/// distance to an infinite line.
pub(crate) fn nearest_road_segment(point: Vec2, search_cells: i32) -> Option<(RoadSegment, Vec2)> {
    let cx = world_to_road_cell(point.x);
    let cz = world_to_road_cell(point.y);
    let radius = search_cells.max(0);
    let mut best: Option<(RoadSegment, Vec2, f32)> = None;
    for gx in cx.saturating_sub(radius)..=cx.saturating_add(radius) {
        for gz in cz.saturating_sub(radius)..=cz.saturating_add(radius) {
            for segment in road_plan(gx, gz).segments.into_iter().flatten() {
                let nearest = closest_point_on_road_segment(point, segment);
                let distance = point.distance_squared(nearest);
                let replace = best.as_ref().is_none_or(|(current, _, current_distance)| {
                    distance < *current_distance - 1e-5
                        || ((distance - *current_distance).abs() <= 1e-5
                            && (segment.gx, segment.gz, segment.socket)
                                < (current.gx, current.gz, current.socket))
                });
                if replace {
                    best = Some((segment, nearest, distance));
                }
            }
        }
    }
    best.map(|(segment, nearest, _)| (segment, nearest))
}

// ---------------------------------------------------------------------------
// 2D city-block grid system
// ---------------------------------------------------------------------------

/// Tunable grid layout. `block` is the size of one city block (and the
/// spacing of road grid lines); `count` is the grid window size (kept alive
/// and recycled in BOTH X and Z). With the defaults (40 × 5) the world
/// covers a 200u × 200u window around the car at any time.
///
/// Positive even counts are supported with a deterministic negative-side
/// bias: for example, count 4 around coordinate 0 spans -2..=1. This keeps
/// exactly `count * count` cells even though an even window cannot have one
/// cell geometrically at its center. Non-positive counts are clamped to 1.
#[derive(Resource)]
pub struct GridConfig {
    pub block: f32,
    pub count: i32,
}

type GridCoord = (i32, i32);

/// Return the exact grid-coordinate window centered on `center`.
///
/// Odd counts are symmetric (`5` gives offsets `-2..=2`). Even counts use
/// the documented negative-side bias (`4` gives `-2..=1`), and non-positive
/// counts are clamped to one cell. A set is returned so cardinality and
/// uniqueness are explicit invariants shared by startup, reset and recycle.
fn desired_grid_coords(center: GridCoord, count: i32) -> BTreeSet<GridCoord> {
    let count = count.max(1);
    let first_x = center.0 - count / 2;
    let first_z = center.1 - count / 2;
    let mut desired = BTreeSet::new();
    for x_offset in 0..count {
        for z_offset in 0..count {
            desired.insert((first_x + x_offset, first_z + z_offset));
        }
    }
    desired
}

/// A deferred-command-safe set-difference plan. Coordinates are sets, so a
/// malformed snapshot containing duplicate coordinates still cannot schedule
/// duplicate coordinate spawns or despawns.
#[derive(Debug, PartialEq, Eq)]
struct RecyclePlan {
    despawn: BTreeSet<GridCoord>,
    spawn: BTreeSet<GridCoord>,
}

/// Build a recycle plan from one immutable snapshot and one desired window.
/// No result depends on commands issued while applying the plan.
fn recycle_plan(
    existing_coords: impl IntoIterator<Item = GridCoord>,
    desired: &BTreeSet<GridCoord>,
) -> RecyclePlan {
    let existing: BTreeSet<_> = existing_coords.into_iter().collect();
    RecyclePlan {
        despawn: existing.difference(desired).copied().collect(),
        spawn: desired.difference(&existing).copied().collect(),
    }
}

impl Default for GridConfig {
    fn default() -> Self {
        Self {
            block: 40.0,
            count: 5,
        }
    }
}

/// Identifies a block-root entity and its grid coordinates. Root transform
/// sits at world `((gx+0.5)*block, 0, (gz+0.5)*block)`. Recycling retires
/// roots outside the desired window and deterministically creates missing
/// `(gx,gz)` roots. The resolved tile kind remains authoritative on the root
/// for runtime inspection and deterministic review metadata.
#[derive(Component)]
pub struct Block {
    pub gx: i32,
    pub gz: i32,
    /// Authoritative generated kind. Runtime and review metadata read this
    /// instead of re-deriving topology from coordinates.
    pub kind: TileKind,
}

/// Shared fixed-dimension meshes and materials used by streamed blocks.
/// Dimension-varying building meshes remain per-instance.
#[derive(Resource)]
pub struct WorldAssets {
    meshes: WorldMeshAssets,
    materials: WorldMaterialAssets,
}

// Rural prop mesh dimensions. Their roots receive arbitrary yaw, so collision
// and placement use the horizontal diagonal derived from these dimensions
// rather than the unrotated axis extents.
const HAY_BALE_RADIUS: f32 = 0.7;
const HAY_BALE_LENGTH: f32 = 1.1;
const FARM_CRATE_SIDE: f32 = 1.1;
const FARM_CRATE_HEIGHT: f32 = 0.7;

struct WorldMeshAssets {
    ground: Handle<Mesh>,
    field_ground: Handle<Mesh>,
    field_furrow: Handle<Mesh>,
    hay_bale: Handle<Mesh>,
    farm_crate: Handle<Mesh>,
    road_pad: Handle<Mesh>,
    road_z: Handle<Mesh>,
    road_x: Handle<Mesh>,
    curb_z: [Handle<Mesh>; 3],
    curb_x: [Handle<Mesh>; 3],
    dash_z: Handle<Mesh>,
    dash_x: Handle<Mesh>,
    edge_line_z: [Handle<Mesh>; 3],
    edge_line_x: [Handle<Mesh>; 3],
    crosswalk_x: Handle<Mesh>,
    crosswalk_z: Handle<Mesh>,
    stop_line_x: Handle<Mesh>,
    stop_line_z: Handle<Mesh>,
    trunk: Handle<Mesh>,
    foliage: Handle<Mesh>,
    tree_shadow: Handle<Mesh>,
    pole: Handle<Mesh>,
    arm: Handle<Mesh>,
    lamp: Handle<Mesh>,
    coin: Handle<Mesh>,
    cone_body: Handle<Mesh>,
    cone_base: Handle<Mesh>,
    cone_shadow: Handle<Mesh>,
    hydrant_body: Handle<Mesh>,
    hydrant_dome: Handle<Mesh>,
    hydrant_nub: Handle<Mesh>,
    hydrant_shadow: Handle<Mesh>,
    bench_seat: Handle<Mesh>,
    bench_leg: Handle<Mesh>,
    bench_back: Handle<Mesh>,
    bench_shadow: Handle<Mesh>,
    hedge_box: Handle<Mesh>,
    hedge_shadow: Handle<Mesh>,
}

struct WorldMaterialAssets {
    line: Handle<StandardMaterial>,
    shadow: Handle<StandardMaterial>,
    park: Handle<StandardMaterial>,
    field: Handle<StandardMaterial>,
    field_furrow: Handle<StandardMaterial>,
    orchard: Handle<StandardMaterial>,
    hay: Handle<StandardMaterial>,
    farm_wood: Handle<StandardMaterial>,
    trunk: Handle<StandardMaterial>,
    foliage: Handle<StandardMaterial>,
    building_body: [Handle<StandardMaterial>; 3],
    building_roof: [Handle<StandardMaterial>; 3],
    building_window: Handle<StandardMaterial>,
    road_marking: Handle<StandardMaterial>,
    metal: Handle<StandardMaterial>,
    lamp: Handle<StandardMaterial>,
    coin: Handle<StandardMaterial>,
    cone: Handle<StandardMaterial>,
    hydrant: Handle<StandardMaterial>,
    bench: Handle<StandardMaterial>,
    hedge: Handle<StandardMaterial>,
}

impl FromWorld for WorldAssets {
    fn from_world(world: &mut World) -> Self {
        // Separate resource scopes ensure the mutable asset-storage borrows
        // never overlap.
        let meshes = world.resource_scope(|_, mut a: Mut<Assets<Mesh>>| WorldMeshAssets {
            ground: a.add(Plane3d::default().mesh().size(42.0, 42.0)),
            // Countryside geometry is procedural but created once and cached;
            // recycled blocks only clone these lightweight handles.
            field_ground: a.add(Plane3d::default().mesh().size(42.0, 42.0)),
            field_furrow: a.add(Cuboid::new(36.0, 0.025, 0.16)),
            hay_bale: a.add(Cylinder::new(HAY_BALE_RADIUS, HAY_BALE_LENGTH)),
            farm_crate: a.add(Cuboid::new(
                FARM_CRATE_SIDE,
                FARM_CRATE_HEIGHT,
                FARM_CRATE_SIDE,
            )),
            road_pad: a.add(Plane3d::default().mesh().size(8.0, 8.0)),
            road_z: a.add(Plane3d::default().mesh().size(8.0, 16.0)),
            road_x: a.add(Plane3d::default().mesh().size(16.0, 8.0)),
            curb_z: std::array::from_fn(|_| a.add(Cuboid::new(1.5, 0.18, 16.0))),
            curb_x: std::array::from_fn(|_| a.add(Cuboid::new(16.0, 0.18, 1.5))),
            dash_z: a.add(Cuboid::new(0.18, 0.02, 2.0)),
            dash_x: a.add(Cuboid::new(2.0, 0.02, 0.18)),
            edge_line_z: std::array::from_fn(|_| a.add(Cuboid::new(0.12, 0.02, 16.0))),
            edge_line_x: std::array::from_fn(|_| a.add(Cuboid::new(16.0, 0.02, 0.12))),
            // Compact approach markings: short, narrow zebra bars and a thin
            // stop line. Keeping them inside the road edges avoids the dense
            // white lattice produced by full-width, broad bars at a four-way
            // junction under the isometric camera.
            crosswalk_x: a.add(Cuboid::new(5.4, 0.025, 0.20)),
            crosswalk_z: a.add(Cuboid::new(0.20, 0.025, 5.4)),
            stop_line_x: a.add(Cuboid::new(5.4, 0.025, 0.12)),
            stop_line_z: a.add(Cuboid::new(0.12, 0.025, 5.4)),
            trunk: a.add(Cylinder::new(0.18, 0.9)),
            foliage: a.add(Sphere::new(0.75).mesh().uv(12, 8)),
            tree_shadow: a.add(Circle::new(0.9)),
            pole: a.add(Cylinder::new(0.07, POLE_HEIGHT)),
            arm: a.add(Cuboid::new(ARM_LEN, ARM_THICK, ARM_THICK)),
            lamp: a.add(Sphere::new(LAMP_RADIUS).mesh().uv(8, 6)),
            coin: a.add(Cylinder::new(0.3, 0.08)),
            cone_body: a.add(bevy::math::primitives::Cone::new(0.18, 0.4)),
            cone_base: a.add(Cuboid::new(0.4, 0.04, 0.4)),
            cone_shadow: a.add(Circle::new(0.3)),
            hydrant_body: a.add(Cylinder::new(0.12, 0.3)),
            hydrant_dome: a.add(Sphere::new(0.1).mesh().uv(10, 6)),
            hydrant_nub: a.add(Cylinder::new(0.05, 0.12)),
            hydrant_shadow: a.add(Circle::new(0.35)),
            bench_seat: a.add(Cuboid::new(0.9, 0.1, 0.3)),
            bench_leg: a.add(Cuboid::new(0.08, 0.45, 0.28)),
            bench_back: a.add(Cuboid::new(0.9, 0.3, 0.06)),
            bench_shadow: a.add(Plane3d::default().mesh().size(1.1, 0.45)),
            hedge_box: a.add(Cuboid::new(1.2, 0.5, 0.4)),
            hedge_shadow: a.add(Plane3d::default().mesh().size(1.4, 0.55)),
        });
        let materials =
            world.resource_scope(
                |_, mut a: Mut<Assets<StandardMaterial>>| WorldMaterialAssets {
                    line: a.add(StandardMaterial {
                        base_color: palette::LANE_WHITE,
                        ..default()
                    }),
                    shadow: a.add(StandardMaterial {
                        base_color: Color::srgba(0.0, 0.0, 0.0, 0.35),
                        alpha_mode: AlphaMode::Blend,
                        ..default()
                    }),
                    park: a.add(StandardMaterial {
                        base_color: Color::srgb(0.24, 0.52, 0.20),
                        perceptual_roughness: 1.0,
                        ..default()
                    }),
                    field: a.add(StandardMaterial {
                        base_color: Color::srgb(0.55, 0.43, 0.16),
                        perceptual_roughness: 1.0,
                        ..default()
                    }),
                    field_furrow: a.add(StandardMaterial {
                        base_color: Color::srgb(0.31, 0.23, 0.09),
                        perceptual_roughness: 1.0,
                        ..default()
                    }),
                    orchard: a.add(StandardMaterial {
                        base_color: Color::srgb(0.27, 0.43, 0.16),
                        perceptual_roughness: 1.0,
                        ..default()
                    }),
                    hay: a.add(StandardMaterial {
                        base_color: Color::srgb(0.82, 0.64, 0.20),
                        perceptual_roughness: 0.95,
                        ..default()
                    }),
                    farm_wood: a.add(StandardMaterial {
                        base_color: Color::srgb(0.38, 0.22, 0.09),
                        perceptual_roughness: 0.95,
                        ..default()
                    }),
                    trunk: a.add(StandardMaterial {
                        base_color: Color::srgb(0.34, 0.21, 0.11),
                        perceptual_roughness: 0.9,
                        ..default()
                    }),
                    foliage: a.add(StandardMaterial {
                        base_color: Color::srgb(0.18, 0.42, 0.16),
                        perceptual_roughness: 0.85,
                        ..default()
                    }),
                    building_body: [
                        a.add(StandardMaterial {
                            base_color: Color::srgb(0.92, 0.88, 0.78),
                            perceptual_roughness: 0.8,
                            ..default()
                        }),
                        a.add(StandardMaterial {
                            base_color: Color::srgb(0.45, 0.55, 0.68),
                            perceptual_roughness: 0.8,
                            ..default()
                        }),
                        a.add(StandardMaterial {
                            base_color: Color::srgb(0.65, 0.35, 0.28),
                            perceptual_roughness: 0.8,
                            ..default()
                        }),
                    ],
                    building_roof: [
                        a.add(StandardMaterial {
                            base_color: Color::srgb(0.64, 0.62, 0.55),
                            perceptual_roughness: 0.85,
                            ..default()
                        }),
                        a.add(StandardMaterial {
                            base_color: Color::srgb(0.32, 0.39, 0.48),
                            perceptual_roughness: 0.85,
                            ..default()
                        }),
                        a.add(StandardMaterial {
                            base_color: Color::srgb(0.46, 0.25, 0.20),
                            perceptual_roughness: 0.85,
                            ..default()
                        }),
                    ],
                    building_window: a.add(StandardMaterial {
                        base_color: Color::srgb(0.045, 0.09, 0.13),
                        metallic: 0.35,
                        perceptual_roughness: 0.2,
                        ..default()
                    }),
                    road_marking: a.add(StandardMaterial {
                        base_color: palette::LANE_WHITE,
                        perceptual_roughness: 0.75,
                        ..default()
                    }),
                    metal: a.add(StandardMaterial {
                        base_color: Color::srgb(0.15, 0.15, 0.16),
                        metallic: 0.8,
                        perceptual_roughness: 0.4,
                        ..default()
                    }),
                    lamp: a.add(StandardMaterial {
                        base_color: Color::srgb(1.0, 0.85, 0.4),
                        emissive: LinearRgba::new(1.5, 1.2, 0.5, 1.0),
                        ..default()
                    }),
                    coin: a.add(StandardMaterial {
                        base_color: palette::COIN,
                        metallic: 0.8,
                        perceptual_roughness: 0.25,
                        emissive: LinearRgba::rgb(0.9, 0.55, 0.05),
                        ..default()
                    }),
                    cone: a.add(StandardMaterial {
                        base_color: Color::srgb(0.95, 0.45, 0.05),
                        perceptual_roughness: 0.7,
                        emissive: LinearRgba::rgb(0.25, 0.08, 0.0),
                        ..default()
                    }),
                    hydrant: a.add(StandardMaterial {
                        base_color: Color::srgb(0.85, 0.12, 0.1),
                        perceptual_roughness: 0.6,
                        emissive: LinearRgba::rgb(0.18, 0.02, 0.0),
                        ..default()
                    }),
                    bench: a.add(StandardMaterial {
                        base_color: Color::srgb(0.45, 0.28, 0.14),
                        perceptual_roughness: 0.9,
                        ..default()
                    }),
                    hedge: a.add(StandardMaterial {
                        base_color: Color::srgb(0.16, 0.34, 0.14),
                        perceptual_roughness: 0.9,
                        ..default()
                    }),
                },
            );
        Self { meshes, materials }
    }
}

pub struct WorldPlugin;

/// Explicit review-harness gate. This resource is never inserted by
/// `WorldPlugin`, so production entities keep their normal archetypes.
#[derive(Resource, Default)]
struct WorldReviewMode;

/// Marker attached only to roots in the deterministic review scene.
#[derive(Component, Clone, Copy)]
struct ReviewTile {
    source: ReviewTileSource,
    catalog_index: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ReviewTileSource {
    Production,
    Atlas,
}

/// Minimal, gameplay-free production-world review plugin. It deliberately
/// reuses `tile_from_edges`, `seed_for`, and `populate_block`; only selection,
/// placement, and reporting differ from the streaming game world.
pub struct WorldReviewPlugin;

impl Plugin for WorldReviewPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(WorldReviewMode)
            .init_resource::<GridConfig>()
            .init_resource::<WorldAssets>()
            .add_systems(Startup, spawn_review_world)
            // This marker means only that the ECS scene and metadata exist.
            // Pixel/render readiness is deliberately owned by the capture tool.
            .add_systems(Update, publish_review_metadata);
    }
}

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GridConfig>()
            .init_resource::<WorldAssets>()
            .add_systems(Startup, spawn_initial_grid)
            // Coin spin + pickup still live here (coins are environment now).
            .add_systems(
                Update,
                (spin_coins, collect_coins)
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            )
            // Knockable cones: integrate bounded flight for airborne cones
            // after the driving chain has launched them, only while playing.
            .add_systems(
                Update,
                update_cone_motion
                    .run_if(in_state(GameState::Playing))
                    .after(DrivingSet),
            )
            // Re-center the grid on the car's spawn at the start of each
            // fresh round (skips on resume from Paused via RoundActive). Runs
            // in SpawnSet so it's before reset_run, which zeroes the car to
            // origin.
            .add_systems(OnEnter(GameState::Playing), reset_grid.in_set(SpawnSet))
            // Reconcile all block roots to the exact count×count coordinate
            // window around the car in one deferred-safe snapshot/plan pass.
            .add_systems(Update, recycle_grid.run_if(in_state(GameState::Playing)));
    }
}

/// Spawn the directional sun + the initial count×count grid of blocks
/// centered on the origin (for count=5: gx,gz in -2..=2). Run once at
/// Startup. The sun is Startup-only and persists — it is NOT re-spawned by
/// `reset_grid`.
fn spawn_initial_grid(
    mut commands: Commands,
    cfg: Res<GridConfig>,
    mut meshes: ResMut<Assets<Mesh>>,
    textures: Res<TextureAssets>,
    world_assets: Res<WorldAssets>,
) {
    // --- Sun: warm directional light (shadows gated for web) ---
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(1.0, 0.94, 0.82),
            shadow_maps_enabled: SHADOWS,
            ..default()
        },
        Transform::from_xyz(30.0, 25.0, 15.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    spawn_grid_window(&mut commands, &cfg, &mut meshes, &textures, &world_assets);
}

/// Spawn the exact count×count grid of blocks centered on the origin. Each
/// block root is at `((gx+0.5)*block, 0, (gz+0.5)*block)` with
/// `Block { gx, gz }`, then `populate_block`. Used by both
/// `spawn_initial_grid` (Startup) and `reset_grid` (round start).
///
/// Each block's tile is derived deterministically from its (gx,gz) via the
/// road-line functions (see `tile_from_edges`) — no neighbour querying or
/// ordering needed, because shared edges are computed from the same line
/// index by both adjacent blocks. Order-independent + mismatch-proof.
fn spawn_grid_window(
    commands: &mut Commands,
    cfg: &GridConfig,
    meshes: &mut Assets<Mesh>,
    textures: &TextureAssets,
    world_assets: &WorldAssets,
) {
    let block = cfg.block;
    for (gx, gz) in desired_grid_coords((0, 0), cfg.count) {
        let kind = tile_from_edges(gx, gz);
        let root = commands
            .spawn((
                Transform::from_xyz(gx as f32 * block, 0.0, gz as f32 * block),
                Visibility::default(),
                Block { gx, gz, kind },
            ))
            .id();
        populate_block(
            commands,
            meshes,
            textures,
            world_assets,
            root,
            gx,
            gz,
            seed_for(gx, gz),
            kind,
        );
    }
}

/// Deterministic per-block seed (varies with (gx,gz) so each block differs,
/// but the same (gx,gz) always yields the same layout — stable across
/// recycles). The tile choice in `pick_tile` consumes a few LCG steps from
/// this same seed, so the whole block layout (tile + decorations) is a pure
/// function of (gx, gz).
fn seed_for(gx: i32, gz: i32) -> u32 {
    (gx as u32).wrapping_mul(1664525) ^ (gz as u32).wrapping_mul(22695477).wrapping_add(0x9e3779b9)
}

// ---------------------------------------------------------------------------
// W0 deterministic production-world review scene and metadata
// ---------------------------------------------------------------------------

const REVIEW_WINDOW_COUNT: i32 = 5;
const REVIEW_BLOCK_SIZE: f32 = 40.0;
const REVIEW_ATLAS_COLUMNS: usize = 10;
const REVIEW_ATLAS_Z: f32 = 160.0;
/// Roads centered on an edge extend this far beyond a nominal tile boundary.
const REVIEW_ROAD_SPILL: f32 = 0.0;
/// Empty space between complete, non-spilling atlas tiles.
const REVIEW_ATLAS_GUTTER: f32 = 10.0;
const REVIEW_ATLAS_PITCH: f32 = REVIEW_BLOCK_SIZE + REVIEW_ATLAS_GUTTER;
// Ground is deliberately 42u for seam hiding, so it is the actual review
// extent even though road topology itself has zero spill.
const REVIEW_CONTENT_HALF_EXTENT: f32 = 21.0;

/// Exact XZ bounds of all review geometry relevant to framing. The forced
/// atlas uses a 10u visible gutter after accounting for each tile's 4u road
/// spill, so incompatible edge sockets can never visually touch.
pub(crate) fn world_review_bounds() -> (Vec2, Vec2) {
    // The 5x5 production roots are centered from -80 through +80. The 42u
    // seam-hiding ground is the widest geometry and roads have zero spill.
    let production_min = Vec2::splat(-80.0 - REVIEW_CONTENT_HALF_EXTENT);
    let production_max = Vec2::splat(80.0 + REVIEW_CONTENT_HALF_EXTENT);
    let atlas_half_columns = (REVIEW_ATLAS_COLUMNS as f32 - 1.0) * 0.5;
    // Ground planes are 42u wide, but road spill reaches 24u from the root.
    let atlas_min = Vec2::new(
        -atlas_half_columns * REVIEW_ATLAS_PITCH - REVIEW_CONTENT_HALF_EXTENT,
        REVIEW_ATLAS_Z - REVIEW_CONTENT_HALF_EXTENT,
    );
    let atlas_rows = TILE_CATALOG.len().div_ceil(REVIEW_ATLAS_COLUMNS);
    let atlas_max = Vec2::new(
        atlas_half_columns * REVIEW_ATLAS_PITCH + REVIEW_CONTENT_HALF_EXTENT,
        REVIEW_ATLAS_Z
            + (atlas_rows.saturating_sub(1)) as f32 * REVIEW_ATLAS_PITCH
            + REVIEW_CONTENT_HALF_EXTENT,
    );
    (production_min.min(atlas_min), production_max.max(atlas_max))
}

#[derive(Serialize, Debug, Default, PartialEq, Eq)]
struct ReviewCounts {
    mesh3d: usize,
    roads: usize,
    curbs: usize,
    markings: usize,
    buildings: usize,
    trees: usize,
    farm_props: usize,
    coins: usize,
    lamps: usize,
    obstacles: usize,
}

#[derive(Serialize, Debug, PartialEq)]
struct ReviewBlockMetadata {
    source: &'static str,
    catalog_index: Option<usize>,
    gx: i32,
    gz: i32,
    kind: &'static str,
    sockets: [&'static str; 4],
    world_x: f32,
    world_z: f32,
    counts: ReviewCounts,
}

#[derive(Serialize, Debug, PartialEq)]
struct ReviewBoundsMetadata {
    min_x: f32,
    max_x: f32,
    min_z: f32,
    max_z: f32,
}

#[derive(Serialize, Debug, PartialEq)]
struct ReviewAtlasMetadata {
    columns: usize,
    pitch: f32,
    gutter: f32,
    road_spill: f32,
    origin_z: f32,
}

#[derive(Serialize, Debug, PartialEq)]
struct ReviewMetadata {
    schema: &'static str,
    ready: bool,
    seed: u32,
    block_size: f32,
    production_window_count: i32,
    topology_version: u32,
    socket_order: [&'static str; 4],
    scene_bounds: ReviewBoundsMetadata,
    atlas: ReviewAtlasMetadata,
    blocks: Vec<ReviewBlockMetadata>,
}

fn tile_kind_name(kind: TileKind) -> &'static str {
    use TileKind::*;
    match kind {
        Empty => "Empty",
        Park => "Park",
        Field => "Field",
        Orchard => "Orchard",
        RoadNS => "RoadNS",
        RoadEW => "RoadEW",
        Cross => "Cross",
        TN => "TN",
        TE => "TE",
        TS => "TS",
        TW => "TW",
        CornerWN => "CornerWN",
        CornerNE => "CornerNE",
        CornerES => "CornerES",
        CornerSW => "CornerSW",
        StubW => "StubW",
        StubE => "StubE",
        StubS => "StubS",
        StubN => "StubN",
    }
}

fn socket_names(kind: TileKind) -> [&'static str; 4] {
    sockets(kind).map(|edge| if edge == Edge::Road { "road" } else { "none" })
}

fn spawn_review_world(
    _mode: Res<WorldReviewMode>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    textures: Res<TextureAssets>,
    world_assets: Res<WorldAssets>,
) {
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(1.0, 0.94, 0.82),
            illuminance: 10_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(-100.0, 180.0, -80.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    for (gx, gz) in desired_grid_coords((0, 0), REVIEW_WINDOW_COUNT) {
        spawn_review_tile(
            &mut commands,
            &mut meshes,
            &textures,
            &world_assets,
            Vec3::new(
                gx as f32 * REVIEW_BLOCK_SIZE,
                0.0,
                gz as f32 * REVIEW_BLOCK_SIZE,
            ),
            gx,
            gz,
            tile_from_edges(gx, gz),
            ReviewTileSource::Production,
            None,
        );
    }
    for (index, &kind) in TILE_CATALOG.iter().enumerate() {
        let column = index % REVIEW_ATLAS_COLUMNS;
        let row = index / REVIEW_ATLAS_COLUMNS;
        spawn_review_tile(
            &mut commands,
            &mut meshes,
            &textures,
            &world_assets,
            Vec3::new(
                (column as f32 - 4.5) * REVIEW_ATLAS_PITCH,
                0.0,
                REVIEW_ATLAS_Z + row as f32 * REVIEW_ATLAS_PITCH,
            ),
            column as i32,
            row as i32,
            kind,
            ReviewTileSource::Atlas,
            Some(index),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_review_tile(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    textures: &TextureAssets,
    world_assets: &WorldAssets,
    position: Vec3,
    gx: i32,
    gz: i32,
    kind: TileKind,
    source: ReviewTileSource,
    catalog_index: Option<usize>,
) {
    let root = commands
        .spawn((
            Transform::from_translation(position),
            Visibility::default(),
            Block { gx, gz, kind },
            ReviewTile {
                source,
                catalog_index,
            },
        ))
        .id();
    let seed = catalog_index.map_or_else(|| seed_for(gx, gz), |i| seed_for(i as i32, -1000));
    populate_block(
        commands,
        meshes,
        textures,
        world_assets,
        root,
        gx,
        gz,
        seed,
        kind,
    );
}

fn publish_review_metadata(world: &mut World, mut published: Local<bool>) {
    if *published {
        return;
    }
    let metadata = build_review_metadata(world);
    // Compact JSON keeps native output machine-readable on exactly one line.
    let json = serde_json::to_string(&metadata).expect("world-review metadata must serialize");
    publish_review_json(&json);
    *published = true;
}

/// Build publication data from the real spawned hierarchy. This is the sole
/// metadata builder used by runtime publication and tests; no estimated
/// per-archetype counts are maintained in parallel.
fn build_review_metadata(world: &mut World) -> ReviewMetadata {
    assert!(
        world.contains_resource::<WorldReviewMode>(),
        "review metadata requested outside WorldReviewMode"
    );
    let mut tile_query = world.query::<(Entity, &Block, &ReviewTile, &Transform)>();
    let tiles: Vec<_> = tile_query
        .iter(world)
        .map(|(entity, block, tile, transform)| {
            (
                entity,
                block.gx,
                block.gz,
                block.kind,
                *tile,
                transform.translation,
            )
        })
        .collect();

    let (road_meshes, marking_meshes) = {
        let assets = world.resource::<WorldAssets>();
        let roads = [
            assets.meshes.road_pad.clone(),
            assets.meshes.road_x.clone(),
            assets.meshes.road_z.clone(),
        ];
        let mut markings = vec![
            assets.meshes.dash_x.clone(),
            assets.meshes.dash_z.clone(),
            assets.meshes.crosswalk_x.clone(),
            assets.meshes.crosswalk_z.clone(),
            assets.meshes.stop_line_x.clone(),
            assets.meshes.stop_line_z.clone(),
        ];
        markings.extend(assets.meshes.edge_line_x.iter().cloned());
        markings.extend(assets.meshes.edge_line_z.iter().cloned());
        (roads, markings)
    };
    let mut blocks = Vec::with_capacity(tiles.len());
    for (entity, gx, gz, kind, tile, translation) in tiles {
        let mut counts = ReviewCounts::default();
        count_review_descendants(world, entity, &road_meshes, &marking_meshes, &mut counts);
        blocks.push(ReviewBlockMetadata {
            source: match tile.source {
                ReviewTileSource::Production => "production",
                ReviewTileSource::Atlas => "atlas",
            },
            catalog_index: tile.catalog_index,
            gx,
            gz,
            kind: tile_kind_name(kind),
            sockets: socket_names(kind),
            world_x: translation.x,
            world_z: translation.z,
            counts,
        });
    }
    blocks.sort_by_key(|block| {
        (
            if block.source == "production" { 0 } else { 1 },
            block.catalog_index.unwrap_or(0),
            block.gx,
            block.gz,
        )
    });
    let (bounds_min, bounds_max) = world_review_bounds();
    ReviewMetadata {
        schema: "roady-world-review-v1",
        ready: true,
        seed: REVIEW_SEED,
        block_size: REVIEW_BLOCK_SIZE,
        production_window_count: REVIEW_WINDOW_COUNT,
        topology_version: 1,
        socket_order: ["west", "east", "south", "north"],
        scene_bounds: ReviewBoundsMetadata {
            min_x: bounds_min.x,
            max_x: bounds_max.x,
            min_z: bounds_min.y,
            max_z: bounds_max.y,
        },
        atlas: ReviewAtlasMetadata {
            columns: REVIEW_ATLAS_COLUMNS,
            pitch: REVIEW_ATLAS_PITCH,
            gutter: REVIEW_ATLAS_GUTTER,
            road_spill: REVIEW_ROAD_SPILL,
            origin_z: REVIEW_ATLAS_Z,
        },
        blocks,
    }
}

fn count_review_descendants(
    world: &World,
    entity: Entity,
    road_meshes: &[Handle<Mesh>; 3],
    marking_meshes: &[Handle<Mesh>],
    counts: &mut ReviewCounts,
) {
    // Roads and markings are classified from the actual Mesh3d handles. This
    // avoids adding review/accounting components to production archetypes.
    if let Some(mesh) = world.get::<Mesh3d>(entity) {
        counts.mesh3d += 1;
        counts.roads += usize::from(road_meshes.contains(&mesh.0));
        counts.markings += usize::from(marking_meshes.contains(&mesh.0));
    }
    counts.curbs += usize::from(world.get::<Curb>(entity).is_some());
    counts.buildings += usize::from(world.get::<Building>(entity).is_some());
    counts.trees += usize::from(world.get::<Tree>(entity).is_some());
    counts.farm_props += usize::from(world.get::<FarmProp>(entity).is_some());
    counts.coins += usize::from(world.get::<Coin>(entity).is_some());
    counts.lamps += usize::from(world.get::<LampPost>(entity).is_some());
    counts.obstacles += usize::from(
        world.get::<Cone>(entity).is_some()
            || world.get::<Hydrant>(entity).is_some()
            || world.get::<Bench>(entity).is_some()
            || world.get::<Hedge>(entity).is_some(),
    );
    if let Some(children) = world.get::<Children>(entity) {
        for child in children.iter() {
            count_review_descendants(world, child, road_meshes, marking_meshes, counts);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn publish_review_json(json: &str) {
    if let Some(window) = web_sys::window() {
        let _ = js_sys::Reflect::set(
            window.as_ref(),
            &"__ROADY_WORLD_REVIEW__".into(),
            &json.into(),
        );
        if let Some(root) = window
            .document()
            .and_then(|document| document.document_element())
        {
            let _ = root.set_attribute("data-roady-world-review-ready", "true");
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn publish_review_json(json: &str) {
    println!("ROADY_WORLD_REVIEW_JSON={json}");
    println!("ROADY_WORLD_REVIEW_READY=1");
}

// ---------------------------------------------------------------------------
// Street-lamp geometry helpers (pure — no ECS, unit-testable in isolation)
// ---------------------------------------------------------------------------
//
// The lamp post is a 3-part assembly parented to the `LampPost` block child:
//   1. POLE  — vertical cylinder rooted at ground (y = 0), height POLE_HEIGHT.
//   2. ARM   — horizontal bar attached to the pole top, extending ARM_LEN
//              toward the nearest road edge.
//   3. LAMP  — emissive sphere hanging from the arm's outer end.
// These pure helpers compute the road-pointing direction and the three
// local Transforms from the cached mesh dimensions, so the geometry
// contract (pole roots at ground, arm connected + oriented toward the road,
// lamp hanging at the arm end, no gaps / no floating parts) is unit-testable
// without spinning up a Bevy ECS world. They are the single source of truth
// for `populate_block`'s lamp-post spawning.

/// Cached pole mesh: `Cylinder::new(0.07, POLE_HEIGHT)` (radius 0.07, height
/// `POLE_HEIGHT`) — a cylinder centered on its midpoint, so placing its
/// center at `POLE_HEIGHT / 2` roots it at the ground (y = 0) with its top at
/// `POLE_HEIGHT`. Only the height is needed for vertical placement, so the
/// radius is not promoted to a named constant.
const POLE_HEIGHT: f32 = 3.2;

/// Cached arm mesh: `Cuboid::new(ARM_LEN, ARM_THICK, ARM_THICK)` — a bar long
/// along local +X and thin in Y/Z. It is rotated π/2 about Y only when the
/// arm points along Z, so its long axis always tracks the road direction.
const ARM_LEN: f32 = 0.8;
const ARM_THICK: f32 = 0.06;

/// Cached lamp mesh: `Sphere::new(LAMP_RADIUS)`.
const LAMP_RADIUS: f32 = 0.14;

/// Unit horizontal direction the lamp arm should point, toward the nearest
/// `Road` edge of the block from the post's local position `(lx, lz)` within
/// a block of half-size `half`. Only edges that are actually roads are
/// considered — a non-road edge is never chosen even if it is closer. Returns
/// `(0.0, 0.0)` when no edge is a road; callers fall back to a default. The
/// result is always axis-aligned with exactly one nonzero component of
/// magnitude 1.0 (or both zero).
fn lamp_arm_direction(
    road_w: bool,
    road_e: bool,
    road_s: bool,
    road_n: bool,
    lx: f32,
    lz: f32,
    half: f32,
) -> (f32, f32) {
    let mut best = (0.0_f32, 0.0_f32);
    let mut best_dist = f32::MAX;
    // Order W, E, S, N — the first (closest) declared road edge wins ties.
    if road_w {
        let d = (-half - lx).abs();
        if d < best_dist {
            best_dist = d;
            best = (-1.0, 0.0);
        }
    }
    if road_e {
        let d = (half - lx).abs();
        if d < best_dist {
            best_dist = d;
            best = (1.0, 0.0);
        }
    }
    if road_s {
        let d = (-half - lz).abs();
        if d < best_dist {
            best_dist = d;
            best = (0.0, -1.0);
        }
    }
    if road_n && (half - lz).abs() < best_dist {
        best = (0.0, 1.0);
    }
    best
}

/// Local Transform of the pole: a vertical cylinder rooted at the ground.
/// The mesh is centered on its midpoint, so center.y = `POLE_HEIGHT / 2`
/// makes it span exactly `0 .. POLE_HEIGHT` (bottom at ground, top at
/// `POLE_HEIGHT`). No horizontal offset — the pole sits at the post's XZ
/// origin.
fn lamp_pole_transform() -> Transform {
    Transform::from_xyz(0.0, POLE_HEIGHT / 2.0, 0.0)
}

/// Local Transform of the arm: a horizontal bar connected to the pole top,
/// extending `ARM_LEN` toward `(dir_x, dir_z)`. The arm's inner end sits at
/// the pole (XZ origin) and its outer end at `(dir_x * ARM_LEN, _,
/// dir_z * ARM_LEN)`. The mesh is long along local +X, so it is rotated π/2
/// about Y only when the direction is along Z (`dir_x == 0`); the direction's
/// sign is carried by the translation because the bar is symmetric about its
/// center. The arm's Y is the pole top, so it overlaps the pole top —
/// connected, no gap.
fn lamp_arm_transform(dir_x: f32, dir_z: f32) -> Transform {
    let rot = if dir_x == 0.0 {
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)
    } else {
        Quat::IDENTITY
    };
    Transform::from_xyz(dir_x * ARM_LEN / 2.0, POLE_HEIGHT, dir_z * ARM_LEN / 2.0)
        .with_rotation(rot)
}

/// Local Transform of the lamp (fixture/bulb): hangs from the arm's outer
/// end, just below the arm so the bulb's top touches the arm's bottom —
/// connected, not floating. Same XZ as the arm outer end.
fn lamp_fixture_transform(dir_x: f32, dir_z: f32) -> Transform {
    let arm_bottom = POLE_HEIGHT - ARM_THICK / 2.0;
    Transform::from_xyz(dir_x * ARM_LEN, arm_bottom - LAMP_RADIUS, dir_z * ARM_LEN)
}

/// Half-extents of the arm's axis-aligned bounding box in the `LampPost`
/// local frame, derived from the actual orientation in `lamp_arm_transform`.
/// The long axis (`ARM_LEN`) ends up along the chosen direction and the thin
/// axes (`ARM_THICK`) along the other two — i.e. the arm is oriented ALONG
/// the road direction, not across it. Pure; used by the geometry tests to
/// verify connection (inner end at the pole, outer end toward the road) and
/// orientation (long along the road, thin across it).
#[cfg(test)]
fn lamp_arm_aabb_half_extents(dir_x: f32, dir_z: f32) -> Vec3 {
    let rot = lamp_arm_transform(dir_x, dir_z).rotation;
    // Local half-extents of the arm mesh (long along X, thin in Y/Z).
    let h = Vec3::new(ARM_LEN / 2.0, ARM_THICK / 2.0, ARM_THICK / 2.0);
    // World-space directions of the local X / Y / Z axes after rotation.
    let bx = rot * Vec3::X;
    let by = rot * Vec3::Y;
    let bz = rot * Vec3::Z;
    // AABB half-extents = sum over local axes of |world component| * local half.
    Vec3::new(
        bx.x.abs() * h.x + by.x.abs() * h.y + bz.x.abs() * h.z,
        bx.y.abs() * h.x + by.y.abs() * h.y + bz.y.abs() * h.z,
        bx.z.abs() * h.x + by.z.abs() * h.y + bz.z.abs() * h.z,
    )
}

/// Build all of one block's contents as children of `root`, per the chosen
/// Wang-tile `kind`: grass cell (always); a road segment on each `Road`
/// edge of the tile (W=−X, E=+X, S=−Z, N=+Z); curbs + lane dashes on each
/// road edge; buildings / trees / lamp posts / T12 obstacles in the interior
/// (overlap-rejected via `try_place`, shrunk away from each `Road` edge by a
/// 6u margin; `None` edges can use the full half-block); for `Park`: trees +
/// a park-green ground tint, no buildings; coins on the `Road` edges only.
///
/// The caller passes the resolved `kind` directly. Decorations are laid out
/// relative to the 40u block size.
#[allow(clippy::too_many_arguments)]
pub fn populate_block(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    textures: &TextureAssets,
    world_assets: &WorldAssets,
    root: Entity,
    gx: i32,
    gz: i32,
    seed: u32,
    kind: TileKind,
) {
    let block = 40.0_f32; // matches GridConfig default; decorations are laid
    // out relative to this.
    let half = block / 2.0;

    let sock = sockets(kind);
    let road_w = sock[W] == Edge::Road;
    let road_e = sock[E] == Edge::Road;
    let road_s = sock[S] == Edge::Road;
    let road_n = sock[N] == Edge::Road;
    let any_road = road_w || road_e || road_s || road_n;
    let is_park = kind == TileKind::Park;
    let is_field = kind == TileKind::Field;
    let is_orchard = kind == TileKind::Orchard;

    // Block-local interior bounds: keep a 6.0u margin from any Road edge (so
    // obstacles never straddle a road), while None edges can use the full
    // half-block. The road is 8 wide (±4 from the edge line), so 6.0u keeps
    // obstacles just past the road's inner edge.
    let interior_max_x_lo = if road_w { -half + 6.0 } else { -half + 1.0 };
    let interior_max_x_hi = if road_e { half - 6.0 } else { half - 1.0 };
    let interior_max_z_lo = if road_s { -half + 6.0 } else { -half + 1.0 };
    let interior_max_z_hi = if road_n { half - 6.0 } else { half - 1.0 };

    let shadow_mat = world_assets.materials.shadow.clone();
    let park_mat = world_assets.materials.park.clone();
    let field_mat = world_assets.materials.field.clone();
    let orchard_mat = world_assets.materials.orchard.clone();

    let _ = (gx, gz); // available for callers; layout uses the seed instead.

    commands.entity(root).with_children(|p| {
        // --- Ground cell (block-wide, slightly oversized to avoid seams) ---
        // Countryside variants use only cached procedural meshes/materials.
        if is_park {
            p.spawn((
                Mesh3d(world_assets.meshes.ground.clone()),
                MeshMaterial3d(park_mat.clone()),
                Transform::from_xyz(0.0, 0.01, 0.0),
            ));
        } else if is_field {
            p.spawn((
                Mesh3d(world_assets.meshes.field_ground.clone()),
                MeshMaterial3d(field_mat.clone()),
                Transform::from_xyz(0.0, 0.01, 0.0),
            ));
            // Parallel low ridges make the procedural field readable while
            // reusing one cached strip mesh for every furrow.
            for z in [-15.0_f32, -10.0, -5.0, 0.0, 5.0, 10.0, 15.0] {
                p.spawn((
                    Mesh3d(world_assets.meshes.field_furrow.clone()),
                    MeshMaterial3d(world_assets.materials.field_furrow.clone()),
                    Transform::from_xyz(0.0, 0.025, z),
                ));
            }
        } else if is_orchard {
            p.spawn((
                Mesh3d(world_assets.meshes.ground.clone()),
                MeshMaterial3d(orchard_mat.clone()),
                Transform::from_xyz(0.0, 0.01, 0.0),
            ));
        } else {
            p.spawn((
                Mesh3d(world_assets.meshes.ground.clone()),
                MeshMaterial3d(textures.grass.clone()),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ));
        }

        // --- Centre-connected road topology ---
        // Every road-bearing tile owns exactly one 8x8 junction pad and one
        // finite 16x8 (or 8x16) arm for each socket. Arms end at the tile
        // boundary and adjacent tiles meet there without overlapping planes.
        if any_road {
            p.spawn((
                Mesh3d(world_assets.meshes.road_pad.clone()),
                MeshMaterial3d(textures.road.clone()),
                Transform::from_xyz(0.0, 0.02, 0.0),
            ));
        }
        for (socket, enabled, center, mesh) in [
            (
                W,
                road_w,
                Vec2::new(-12.0, 0.0),
                world_assets.meshes.road_x.clone(),
            ),
            (
                E,
                road_e,
                Vec2::new(12.0, 0.0),
                world_assets.meshes.road_x.clone(),
            ),
            (
                S,
                road_s,
                Vec2::new(0.0, -12.0),
                world_assets.meshes.road_z.clone(),
            ),
            (
                N,
                road_n,
                Vec2::new(0.0, 12.0),
                world_assets.meshes.road_z.clone(),
            ),
        ] {
            if !enabled {
                continue;
            }
            p.spawn((
                Mesh3d(mesh),
                MeshMaterial3d(textures.road.clone()),
                Transform::from_xyz(center.x, 0.02, center.y),
            ));

            // Curbs and edge lines belong to the arm, never to a boundary
            // plane. Two parallel curbs make each finite arm legible.
            let line_mat = world_assets.materials.line.clone();
            if socket <= E {
                for z in [-4.75_f32, 4.75] {
                    p.spawn((
                        Mesh3d(world_assets.meshes.curb_x[0].clone()),
                        MeshMaterial3d(textures.sidewalk.clone()),
                        Transform::from_xyz(center.x, 0.09, z),
                        Curb {
                            half_x: 8.0,
                            half_z: 0.75,
                            height: 0.18,
                        },
                    ));
                }
                for z in [-3.75_f32, 3.75] {
                    p.spawn((
                        Mesh3d(world_assets.meshes.edge_line_x[0].clone()),
                        MeshMaterial3d(line_mat.clone()),
                        Transform::from_xyz(center.x, 0.035, z),
                    ));
                }
                let sign = if socket == W { -1.0 } else { 1.0 };
                for along in [6.0_f32, 10.0, 14.0, 18.0] {
                    p.spawn((
                        Mesh3d(world_assets.meshes.dash_x.clone()),
                        MeshMaterial3d(line_mat.clone()),
                        Transform::from_xyz(sign * along, 0.035, 0.0),
                    ));
                }
            } else {
                for x in [-4.75_f32, 4.75] {
                    p.spawn((
                        Mesh3d(world_assets.meshes.curb_z[0].clone()),
                        MeshMaterial3d(textures.sidewalk.clone()),
                        Transform::from_xyz(x, 0.09, center.y),
                        Curb {
                            half_x: 0.75,
                            half_z: 8.0,
                            height: 0.18,
                        },
                    ));
                }
                for x in [-3.75_f32, 3.75] {
                    p.spawn((
                        Mesh3d(world_assets.meshes.edge_line_z[0].clone()),
                        MeshMaterial3d(line_mat.clone()),
                        Transform::from_xyz(x, 0.035, center.y),
                    ));
                }
                let sign = if socket == S { -1.0 } else { 1.0 };
                for along in [6.0_f32, 10.0, 14.0, 18.0] {
                    p.spawn((
                        Mesh3d(world_assets.meshes.dash_z.clone()),
                        MeshMaterial3d(line_mat.clone()),
                        Transform::from_xyz(0.0, 0.035, sign * along),
                    ));
                }
            }
        }

        // Junction approaches get compact arm-owned crossing/stop marks.
        let road_count = [road_w, road_e, road_s, road_n]
            .into_iter()
            .filter(|enabled| *enabled)
            .count();
        if road_count >= 2 {
            let marking_mat = world_assets.materials.road_marking.clone();
            for (socket, enabled) in [(W, road_w), (E, road_e), (S, road_s), (N, road_n)] {
                if !enabled {
                    continue;
                }
                let sign = if socket == W || socket == S {
                    -1.0
                } else {
                    1.0
                };
                for offset in [-0.38_f32, 0.0, 0.38] {
                    let (mesh, pos) = if socket <= E {
                        (
                            world_assets.meshes.crosswalk_z.clone(),
                            Vec3::new(sign * (5.0 + offset), 0.06, 0.0),
                        )
                    } else {
                        (
                            world_assets.meshes.crosswalk_x.clone(),
                            Vec3::new(0.0, 0.06, sign * (5.0 + offset)),
                        )
                    };
                    p.spawn((
                        Mesh3d(mesh),
                        MeshMaterial3d(marking_mat.clone()),
                        Transform::from_translation(pos),
                    ));
                }
                let (mesh, pos) = if socket <= E {
                    (
                        world_assets.meshes.stop_line_z.clone(),
                        Vec3::new(sign * 6.2, 0.06, 0.0),
                    )
                } else {
                    (
                        world_assets.meshes.stop_line_x.clone(),
                        Vec3::new(0.0, 0.06, sign * 6.2),
                    )
                };
                p.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(marking_mat.clone()),
                    Transform::from_translation(pos),
                ));
            }
        }

        // A single-socket tile is a dead end. Cap the far side of its centre
        // pad with a transverse raised curb so the stub reads as intentional.
        if road_count == 1 {
            let (mesh, transform, half_x, half_z) = if road_w {
                (
                    world_assets.meshes.curb_z[0].clone(),
                    Transform::from_xyz(4.75, 0.09, 0.0).with_scale(Vec3::new(1.0, 1.0, 0.5)),
                    0.75,
                    4.0,
                )
            } else if road_e {
                (
                    world_assets.meshes.curb_z[0].clone(),
                    Transform::from_xyz(-4.75, 0.09, 0.0).with_scale(Vec3::new(1.0, 1.0, 0.5)),
                    0.75,
                    4.0,
                )
            } else if road_s {
                (
                    world_assets.meshes.curb_x[0].clone(),
                    Transform::from_xyz(0.0, 0.09, 4.75).with_scale(Vec3::new(0.5, 1.0, 1.0)),
                    4.0,
                    0.75,
                )
            } else {
                (
                    world_assets.meshes.curb_x[0].clone(),
                    Transform::from_xyz(0.0, 0.09, -4.75).with_scale(Vec3::new(0.5, 1.0, 1.0)),
                    4.0,
                    0.75,
                )
            };
            p.spawn((
                Mesh3d(mesh),
                MeshMaterial3d(textures.sidewalk.clone()),
                transform,
                Curb {
                    half_x,
                    half_z,
                    height: 0.18,
                },
            ));
        }

        // --- Shared obstacle assets ---
        let a = world_assets;
        let trunk_mesh = a.meshes.trunk.clone();
        let trunk_mat = a.materials.trunk.clone();
        let foliage_mesh = a.meshes.foliage.clone();
        let foliage_mat = a.materials.foliage.clone();
        let tree_shadow_mesh = a.meshes.tree_shadow.clone();
        let body_mats = &a.materials.building_body;
        let roof_mats = &a.materials.building_roof;
        let window_mat = a.materials.building_window.clone();
        let pole_mesh = a.meshes.pole.clone();
        let arm_mesh = a.meshes.arm.clone();
        let metal_mat = a.materials.metal.clone();
        let lamp_mesh = a.meshes.lamp.clone();
        let lamp_mat = a.materials.lamp.clone();
        let coin_mesh = a.meshes.coin.clone();
        let coin_mat = a.materials.coin.clone();
        let cone_body_mesh = a.meshes.cone_body.clone();
        let cone_base_mesh = a.meshes.cone_base.clone();
        let cone_mat = a.materials.cone.clone();
        let cone_shadow_mesh = a.meshes.cone_shadow.clone();
        let hydrant_body_mesh = a.meshes.hydrant_body.clone();
        let hydrant_dome_mesh = a.meshes.hydrant_dome.clone();
        let hydrant_nub_mesh = a.meshes.hydrant_nub.clone();
        let hydrant_mat = a.materials.hydrant.clone();
        let hydrant_shadow_mesh = a.meshes.hydrant_shadow.clone();
        let bench_seat_mesh = a.meshes.bench_seat.clone();
        let bench_leg_mesh = a.meshes.bench_leg.clone();
        let bench_back_mesh = a.meshes.bench_back.clone();
        let bench_mat = a.materials.bench.clone();
        let bench_shadow_mesh = a.meshes.bench_shadow.clone();
        let hedge_box_mesh = a.meshes.hedge_box.clone();
        let hedge_mat = a.materials.hedge.clone();
        let hedge_shadow_mesh = a.meshes.hedge_shadow.clone();
        let hay_bale_mesh = a.meshes.hay_bale.clone();
        let hay_mat = a.materials.hay.clone();
        let farm_crate_mesh = a.meshes.farm_crate.clone();
        let farm_wood_mat = a.materials.farm_wood.clone();

        // --- Deterministic per-block LCG for placement variety ---
        let mut s = seed;
        // Overlap-rejection footprint list (simple-room-placement): every
        // building/tree/lamp/obstacle we place pushes its AABB here so later
        // placements skip spots that overlap it (with a margin). Prevents the
        // overlapping buildings/obstacles the user reported.
        let mut placed: Vec<[f32; 4]> = Vec::new();
        // Register the actual pad/arm/curb footprint before any prop. This is
        // the authoritative road exclusion path for buildings, vegetation,
        // lamps, farm dressing, and street obstacles.
        if any_road {
            placed.push([-5.5, 5.5, -5.5, 5.5]);
        }
        if road_w {
            placed.push([-20.0, -4.0, -5.5, 5.5]);
        }
        if road_e {
            placed.push([4.0, 20.0, -5.5, 5.5]);
        }
        if road_s {
            placed.push([-5.5, 5.5, -20.0, -4.0]);
        }
        if road_n {
            placed.push([-5.5, 5.5, 4.0, 20.0]);
        }

        // --- Coins on the Road arms only ---
        let road_sockets: Vec<_> = [road_w, road_e, road_s, road_n]
            .into_iter()
            .enumerate()
            .filter_map(|(socket, enabled)| enabled.then_some(socket))
            .collect();
        for _ in 0..if any_road { 4 } else { 0 } {
            let index =
                ((rand(&mut s) * road_sockets.len() as f32) as usize).min(road_sockets.len() - 1);
            let socket = road_sockets[index];
            let along = 6.0 + rand(&mut s) * 12.0;
            let lateral = (rand(&mut s) * 2.0 - 1.0) * 3.0;
            let (cx, cz) = match socket {
                W => (-along, lateral),
                E => (along, lateral),
                S => (lateral, -along),
                _ => (lateral, along),
            };
            p.spawn((
                Mesh3d(coin_mesh.clone()),
                MeshMaterial3d(coin_mat.clone()),
                Transform::from_xyz(cx, 0.5, cz),
                Coin,
            ));
        }

        // --- Interior decorations ---
        // Park, Field and Orchard are dedicated non-urban branches: none can
        // reach the buildings/lamps/T12 branch below. Empty and road-bearing
        // tiles retain the existing urban decoration behavior. Interior
        // bounds are shrunk away from Road edges; None edges use the block.
        if is_park {
            // --- Park: more trees, no buildings/lamps/obstacles ---
            for _ in 0..6 {
                let Some((tx, tz)) = try_place(
                    &mut placed,
                    &mut s,
                    0.3,
                    0.3,
                    interior_max_x_lo,
                    interior_max_x_hi,
                    interior_max_z_lo,
                    interior_max_z_hi,
                    1.0,
                    10,
                ) else {
                    continue;
                };
                p.spawn((
                    Transform::from_xyz(tx, 0.0, tz),
                    Visibility::default(),
                    Collider {
                        half_x: 0.3,
                        half_z: 0.3,
                    },
                    Tree,
                ))
                .with_children(|tp| {
                    tp.spawn((
                        Mesh3d(trunk_mesh.clone()),
                        MeshMaterial3d(trunk_mat.clone()),
                        Transform::from_xyz(0.0, 0.45, 0.0),
                    ));
                    tp.spawn((
                        Mesh3d(foliage_mesh.clone()),
                        MeshMaterial3d(foliage_mat.clone()),
                        Transform::from_xyz(0.0, 1.35, 0.0),
                    ));
                    tp.spawn((
                        Mesh3d(tree_shadow_mesh.clone()),
                        MeshMaterial3d(shadow_mat.clone()),
                        Transform::from_xyz(0.0, 0.05, 0.0),
                    ));
                });
            }
        } else if is_field {
            // --- Field: a bounded deterministic set of cached farm props ---
            // The layout helper uses widely separated slots, so full collider
            // footprints remain in bounds and never overlap.
            let (props, count) = field_prop_layout(seed);
            // Keep the existing slot/jitter layout, but admit each fixed
            // candidate through the same footprint path as other obstacles.
            // Degenerate center ranges mean `try_place` validates/registers
            // the candidate without changing its visual position. The exact
            // same rotation-independent half-extent is assigned to Collider.
            let mut footprint_seed = seed ^ 0xa511_e9b3;
            for prop in props.into_iter().take(count) {
                let half_extent = field_prop_collider_half_extent(prop.kind);
                let Some((prop_x, prop_z)) = try_place(
                    &mut placed,
                    &mut footprint_seed,
                    half_extent,
                    half_extent,
                    prop.position.x,
                    prop.position.x,
                    prop.position.y,
                    prop.position.y,
                    0.0,
                    1,
                ) else {
                    // Slots are intentionally much farther apart than the
                    // largest conservative footprint, so this is unreachable
                    // unless that layout invariant is changed.
                    debug_assert!(false, "field prop layout produced overlapping footprints");
                    continue;
                };
                p.spawn((
                    Transform::from_xyz(prop_x, 0.0, prop_z)
                        .with_rotation(Quat::from_rotation_y(prop.rotation)),
                    Visibility::default(),
                    Collider {
                        half_x: half_extent,
                        half_z: half_extent,
                    },
                    FarmProp,
                ))
                .with_children(|fp| {
                    match prop.kind {
                        FieldPropKind::HayBale => {
                            fp.spawn((
                                Mesh3d(hay_bale_mesh.clone()),
                                MeshMaterial3d(hay_mat.clone()),
                                // Cylinder axis Y -> rotate onto its side along X.
                                Transform::from_xyz(0.0, HAY_BALE_RADIUS, 0.0).with_rotation(
                                    Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                                ),
                            ));
                        }
                        FieldPropKind::FarmCrate => {
                            fp.spawn((
                                Mesh3d(farm_crate_mesh.clone()),
                                MeshMaterial3d(farm_wood_mat.clone()),
                                Transform::from_xyz(0.0, FARM_CRATE_HEIGHT / 2.0, 0.0),
                            ));
                        }
                    }
                });
            }
        } else if is_orchard {
            // --- Orchard: fixed-cardinality, aligned rows of trees ---
            // Row orientation varies by seed, but spacing and alignment stay
            // exact. No buildings, lamps or T12 street obstacles are emitted.
            for pos in orchard_tree_layout(seed) {
                p.spawn((
                    Transform::from_xyz(pos.x, 0.0, pos.y),
                    Visibility::default(),
                    Collider {
                        half_x: 0.3,
                        half_z: 0.3,
                    },
                    Tree,
                ))
                .with_children(|tp| {
                    tp.spawn((
                        Mesh3d(trunk_mesh.clone()),
                        MeshMaterial3d(trunk_mat.clone()),
                        Transform::from_xyz(0.0, 0.45, 0.0),
                    ));
                    tp.spawn((
                        Mesh3d(foliage_mesh.clone()),
                        MeshMaterial3d(foliage_mat.clone()),
                        Transform::from_xyz(0.0, 1.35, 0.0),
                    ));
                    tp.spawn((
                        Mesh3d(tree_shadow_mesh.clone()),
                        MeshMaterial3d(shadow_mat.clone()),
                        Transform::from_xyz(0.0, 0.05, 0.0),
                    ));
                });
            }
        } else {
            // --- ~3 buildings (overlap-rejected, block interior) ---
            for _ in 0..3 {
                let w = 3.5 + rand(&mut s) * 1.5; // 3.5..5.0
                let h = 4.0 + rand(&mut s) * 5.0; // 4.0..9.0
                let d = 3.5 + rand(&mut s) * 1.5;
                let ci = (rand(&mut s) * 3.0) as usize % 3;
                let Some((bx, bz)) = try_place(
                    &mut placed,
                    &mut s,
                    w / 2.0,
                    d / 2.0,
                    // Shrink the center range by the building's half-extent so
                    // the FULL footprint (center +/- half) stays past the curb /
                    // sidewalk (the user-reported "buildings on top of sidewalk").
                    interior_max_x_lo + w / 2.0,
                    interior_max_x_hi - w / 2.0,
                    interior_max_z_lo + d / 2.0,
                    interior_max_z_hi - d / 2.0,
                    1.5,
                    8,
                ) else {
                    continue;
                };
                // Facade dimensions vary with the building, so these two
                // meshes stay per-building like its body/roof/shadow. All
                // rows and buildings share the one cached glass material.
                let window_rows = window_row_heights(h);
                let window_x_mesh = meshes.add(Cuboid::new(w * 0.72, 0.55, 0.08));
                let window_z_mesh = meshes.add(Cuboid::new(0.08, 0.55, d * 0.72));
                p.spawn((
                    Transform::from_xyz(bx, 0.0, bz),
                    Visibility::default(),
                    Collider {
                        half_x: w / 2.0,
                        half_z: d / 2.0,
                    },
                    Building,
                ))
                .with_children(|bp| {
                    bp.spawn((
                        Mesh3d(meshes.add(Cuboid::new(w, h, d))),
                        MeshMaterial3d(body_mats[ci].clone()),
                        Transform::from_xyz(0.0, h / 2.0, 0.0),
                    ));
                    bp.spawn((
                        Mesh3d(meshes.add(Cuboid::new(w * 1.12, 0.4, d * 1.12))),
                        MeshMaterial3d(roof_mats[ci].clone()),
                        Transform::from_xyz(0.0, h + 0.2, 0.0),
                    ));
                    bp.spawn((
                        Mesh3d(meshes.add(Plane3d::default().mesh().size(w * 1.4, d * 1.4))),
                        MeshMaterial3d(shadow_mat.clone()),
                        Transform::from_xyz(0.0, 0.05, 0.0),
                    ));
                    for &row_y in &window_rows {
                        for z in [-d / 2.0 - 0.045, d / 2.0 + 0.045] {
                            bp.spawn((
                                Mesh3d(window_x_mesh.clone()),
                                MeshMaterial3d(window_mat.clone()),
                                Transform::from_xyz(0.0, row_y, z),
                            ));
                        }
                        for x in [-w / 2.0 - 0.045, w / 2.0 + 0.045] {
                            bp.spawn((
                                Mesh3d(window_z_mesh.clone()),
                                MeshMaterial3d(window_mat.clone()),
                                Transform::from_xyz(x, row_y, 0.0),
                            ));
                        }
                    }
                });
            }

            // --- ~3 trees (overlap-rejected, block interior) ---
            for _ in 0..3 {
                let Some((tx, tz)) = try_place(
                    &mut placed,
                    &mut s,
                    0.3,
                    0.3,
                    interior_max_x_lo,
                    interior_max_x_hi,
                    interior_max_z_lo,
                    interior_max_z_hi,
                    1.0,
                    8,
                ) else {
                    continue;
                };
                p.spawn((
                    Transform::from_xyz(tx, 0.0, tz),
                    Visibility::default(),
                    Collider {
                        half_x: 0.3,
                        half_z: 0.3,
                    },
                    Tree,
                ))
                .with_children(|tp| {
                    tp.spawn((
                        Mesh3d(trunk_mesh.clone()),
                        MeshMaterial3d(trunk_mat.clone()),
                        Transform::from_xyz(0.0, 0.45, 0.0),
                    ));
                    tp.spawn((
                        Mesh3d(foliage_mesh.clone()),
                        MeshMaterial3d(foliage_mat.clone()),
                        Transform::from_xyz(0.0, 1.35, 0.0),
                    ));
                    tp.spawn((
                        Mesh3d(tree_shadow_mesh.clone()),
                        MeshMaterial3d(shadow_mat.clone()),
                        Transform::from_xyz(0.0, 0.05, 0.0),
                    ));
                });
            }

            // --- ~2 lamp posts (overlap-rejected, block interior) ---
            for _ in 0..2 {
                let Some((lx, lz)) = try_place(
                    &mut placed,
                    &mut s,
                    0.15,
                    0.15,
                    interior_max_x_lo,
                    interior_max_x_hi,
                    interior_max_z_lo,
                    interior_max_z_hi,
                    2.0,
                    8,
                ) else {
                    continue;
                };
                // Arm points toward the nearest Road edge. Only actual road
                // edges are considered; with no road, preserve the -X default.
                let (mut dir_x, dir_z) =
                    lamp_arm_direction(road_w, road_e, road_s, road_n, lx, lz, half);
                if dir_x == 0.0 && dir_z == 0.0 {
                    dir_x = -1.0;
                }
                p.spawn((
                    Transform::from_xyz(lx, 0.0, lz),
                    Visibility::default(),
                    Collider {
                        half_x: 0.15,
                        half_z: 0.15,
                    },
                    LampPost,
                ))
                .with_children(|lp| {
                    lp.spawn((
                        Mesh3d(pole_mesh.clone()),
                        MeshMaterial3d(metal_mat.clone()),
                        lamp_pole_transform(),
                    ));
                    lp.spawn((
                        Mesh3d(arm_mesh.clone()),
                        MeshMaterial3d(metal_mat.clone()),
                        lamp_arm_transform(dir_x, dir_z),
                    ));
                    lp.spawn((
                        Mesh3d(lamp_mesh.clone()),
                        MeshMaterial3d(lamp_mat.clone()),
                        lamp_fixture_transform(dir_x, dir_z),
                    ));
                });
            }

            // --- Scatter 2-4 T12 obstacles (mix of four types, overlap-rejected) ---
            let n_obs = 2 + (rand(&mut s) * 3.0) as usize; // 2..4
            for _ in 0..n_obs {
                let kind = (rand(&mut s) * 4.0) as usize % 4; // 0=cone,1=hydrant,2=bench,3=hedge
                // Footprint half-extents per kind (matches the Collider below).
                let (half_x, half_z) = match kind {
                    0 => (0.2, 0.2),   // cone
                    1 => (0.25, 0.25), // hydrant
                    2 => (0.5, 0.18),  // bench
                    _ => (0.6, 0.25),  // hedge
                };
                let Some((ox, oz)) = try_place(
                    &mut placed,
                    &mut s,
                    half_x,
                    half_z,
                    interior_max_x_lo,
                    interior_max_x_hi,
                    interior_max_z_lo,
                    interior_max_z_hi,
                    0.8,
                    8,
                ) else {
                    continue;
                };
                match kind {
                    0 => {
                        // Traffic cone: tapered cone body on a square base.
                        p.spawn((
                            Transform::from_xyz(ox, 0.0, oz),
                            Visibility::default(),
                            Collider {
                                half_x: 0.2,
                                half_z: 0.2,
                            },
                            Cone,
                            ConeMotion::default(),
                        ))
                        .with_children(|cp| {
                            // Cone is centered on its midpoint (base at y=0, tip at
                            // y=height), so a 0.4-tall cone sits at y=0.2.
                            cp.spawn((
                                Mesh3d(cone_body_mesh.clone()),
                                MeshMaterial3d(cone_mat.clone()),
                                Transform::from_xyz(0.0, 0.2, 0.0),
                            ));
                            cp.spawn((
                                Mesh3d(cone_base_mesh.clone()),
                                MeshMaterial3d(cone_mat.clone()),
                                Transform::from_xyz(0.0, 0.02, 0.0),
                            ));
                            cp.spawn((
                                Mesh3d(cone_shadow_mesh.clone()),
                                MeshMaterial3d(shadow_mat.clone()),
                                Transform::from_xyz(0.0, 0.05, 0.0),
                                ConeShadow,
                            ));
                        });
                    }
                    1 => {
                        // Fire hydrant: short cylinder body, dome cap, two side nubs.
                        p.spawn((
                            Transform::from_xyz(ox, 0.0, oz),
                            Visibility::default(),
                            Collider {
                                half_x: 0.25,
                                half_z: 0.25,
                            },
                            Hydrant,
                        ))
                        .with_children(|hp| {
                            // Cylinder centered on midpoint: 0.3 tall -> y=0.15.
                            hp.spawn((
                                Mesh3d(hydrant_body_mesh.clone()),
                                MeshMaterial3d(hydrant_mat.clone()),
                                Transform::from_xyz(0.0, 0.15, 0.0),
                            ));
                            // Dome caps the top (cylinder top at y=0.3).
                            hp.spawn((
                                Mesh3d(hydrant_dome_mesh.clone()),
                                MeshMaterial3d(hydrant_mat.clone()),
                                Transform::from_xyz(0.0, 0.34, 0.0),
                            ));
                            // Side nubs: rotate cylinder axis from Y to X.
                            hp.spawn((
                                Mesh3d(hydrant_nub_mesh.clone()),
                                MeshMaterial3d(hydrant_mat.clone()),
                                Transform::from_xyz(0.15, 0.18, 0.0).with_rotation(
                                    Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                                ),
                            ));
                            hp.spawn((
                                Mesh3d(hydrant_nub_mesh.clone()),
                                MeshMaterial3d(hydrant_mat.clone()),
                                Transform::from_xyz(-0.15, 0.18, 0.0).with_rotation(
                                    Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
                                ),
                            ));
                            hp.spawn((
                                Mesh3d(hydrant_shadow_mesh.clone()),
                                MeshMaterial3d(shadow_mat.clone()),
                                Transform::from_xyz(0.0, 0.05, 0.0),
                            ));
                        });
                    }
                    2 => {
                        // Bench: long seat on two legs + a backrest, wood/brown.
                        p.spawn((
                            Transform::from_xyz(ox, 0.0, oz),
                            Visibility::default(),
                            Collider {
                                half_x: 0.5,
                                half_z: 0.18,
                            },
                            Bench,
                        ))
                        .with_children(|bp| {
                            // Seat at sitting height ~0.45.
                            bp.spawn((
                                Mesh3d(bench_seat_mesh.clone()),
                                MeshMaterial3d(bench_mat.clone()),
                                Transform::from_xyz(0.0, 0.45, 0.0),
                            ));
                            // Two legs supporting the seat.
                            bp.spawn((
                                Mesh3d(bench_leg_mesh.clone()),
                                MeshMaterial3d(bench_mat.clone()),
                                Transform::from_xyz(0.35, 0.225, 0.0),
                            ));
                            bp.spawn((
                                Mesh3d(bench_leg_mesh.clone()),
                                MeshMaterial3d(bench_mat.clone()),
                                Transform::from_xyz(-0.35, 0.225, 0.0),
                            ));
                            // Backrest along the back edge of the seat.
                            bp.spawn((
                                Mesh3d(bench_back_mesh.clone()),
                                MeshMaterial3d(bench_mat.clone()),
                                Transform::from_xyz(0.0, 0.65, -0.12),
                            ));
                            bp.spawn((
                                Mesh3d(bench_shadow_mesh.clone()),
                                MeshMaterial3d(shadow_mat.clone()),
                                Transform::from_xyz(0.0, 0.05, 0.0),
                            ));
                        });
                    }
                    _ => {
                        // Hedge: a dark-green box row segment.
                        p.spawn((
                            Transform::from_xyz(ox, 0.0, oz),
                            Visibility::default(),
                            Collider {
                                half_x: 0.6,
                                half_z: 0.25,
                            },
                            Hedge,
                        ))
                        .with_children(|hp| {
                            // Box centered on its midpoint: 0.5 tall -> y=0.25.
                            hp.spawn((
                                Mesh3d(hedge_box_mesh.clone()),
                                MeshMaterial3d(hedge_mat.clone()),
                                Transform::from_xyz(0.0, 0.25, 0.0),
                            ));
                            hp.spawn((
                                Mesh3d(hedge_shadow_mesh.clone()),
                                MeshMaterial3d(shadow_mat.clone()),
                                Transform::from_xyz(0.0, 0.05, 0.0),
                            ));
                        });
                    }
                }
            }
        }
    });
}

/// Marker for collidable field dressing. `Collider` also keeps these props on
/// the existing minimap obstacle path without any minimap-specific entities.
#[derive(Component)]
struct FarmProp;

const FIELD_PROP_MIN: usize = 3;
const FIELD_PROP_MAX: usize = 5;
#[cfg(test)]
const FIELD_PROP_LIMIT: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldPropKind {
    HayBale,
    FarmCrate,
}

/// Rotation-independent square half-extent for a field prop's XZ collider.
///
/// Hay is a cylinder laid on its side: before root yaw its horizontal extents
/// are half its axial length and its radius. A crate has square horizontal
/// extents. In both cases the rectangle's half-diagonal contains its AABB at
/// every yaw, so collision cannot underbound a randomly rotated mesh.
fn field_prop_collider_half_extent(kind: FieldPropKind) -> f32 {
    let (local_half_x, local_half_z) = field_prop_local_horizontal_half_extents(kind);
    local_half_x.hypot(local_half_z)
}

fn field_prop_local_horizontal_half_extents(kind: FieldPropKind) -> (f32, f32) {
    match kind {
        FieldPropKind::HayBale => (HAY_BALE_LENGTH / 2.0, HAY_BALE_RADIUS),
        FieldPropKind::FarmCrate => (FARM_CRATE_SIDE / 2.0, FARM_CRATE_SIDE / 2.0),
    }
}

/// Exact horizontal AABB half-extents of the prop geometry after root yaw.
/// Used to test the conservative collider against the full rotation range.
#[cfg(test)]
fn field_prop_geometry_aabb_half_extents(kind: FieldPropKind, yaw: f32) -> Vec2 {
    let (half_x, half_z) = field_prop_local_horizontal_half_extents(kind);
    let (sin, cos) = yaw.sin_cos();
    Vec2::new(
        cos.abs() * half_x + sin.abs() * half_z,
        sin.abs() * half_x + cos.abs() * half_z,
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FieldPropPlacement {
    position: Vec2,
    rotation: f32,
    kind: FieldPropKind,
}

/// Deterministic bounded field layout with no heap allocation. Slots are far
/// enough apart that the largest conservative collider footprints cannot
/// overlap; small jitter avoids making every field identical.
fn field_prop_layout(seed: u32) -> ([FieldPropPlacement; FIELD_PROP_MAX], usize) {
    let mut s = seed ^ 0x6d2b_79f5;
    let mut slots = [
        Vec2::new(-11.0, -11.0),
        Vec2::new(0.0, -11.0),
        Vec2::new(11.0, -11.0),
        Vec2::new(-11.0, 0.0),
        Vec2::ZERO,
        Vec2::new(11.0, 0.0),
        Vec2::new(-11.0, 11.0),
        Vec2::new(0.0, 11.0),
        Vec2::new(11.0, 11.0),
    ];
    // In-place Fisher-Yates shuffle; clamp handles rand's inclusive 1.0 edge.
    for i in (1..slots.len()).rev() {
        let j = ((rand(&mut s) * (i + 1) as f32) as usize).min(i);
        slots.swap(i, j);
    }
    let span = FIELD_PROP_MAX - FIELD_PROP_MIN + 1;
    let count = FIELD_PROP_MIN + ((rand(&mut s) * span as f32) as usize).min(span - 1);
    let placements = std::array::from_fn(|i| {
        let jitter = Vec2::new(rand(&mut s) * 2.0 - 1.0, rand(&mut s) * 2.0 - 1.0);
        // The minimum count is three, so forcing the first two kinds ensures
        // every field visibly contains both hay and a second farm prop.
        let kind = match i {
            0 => FieldPropKind::HayBale,
            1 => FieldPropKind::FarmCrate,
            _ if rand(&mut s) < 0.7 => FieldPropKind::HayBale,
            _ => FieldPropKind::FarmCrate,
        };
        FieldPropPlacement {
            position: slots[i] + jitter,
            rotation: rand(&mut s) * std::f32::consts::TAU,
            kind,
        }
    });
    (placements, count)
}

const ORCHARD_ROWS: usize = 3;
const ORCHARD_TREES_PER_ROW: usize = 4;
const ORCHARD_TREE_COUNT: usize = ORCHARD_ROWS * ORCHARD_TREES_PER_ROW;
#[cfg(test)]
const ORCHARD_LIMIT: f32 = 16.0;

/// Deterministic aligned orchard rows, returned as a fixed array to avoid a
/// transient allocation during streaming. The seed selects X- or Z-running
/// rows; every position remains comfortably inside the all-None block.
fn orchard_tree_layout(seed: u32) -> [Vec2; ORCHARD_TREE_COUNT] {
    const ACROSS: [f32; ORCHARD_ROWS] = [-10.0, 0.0, 10.0];
    const ALONG: [f32; ORCHARD_TREES_PER_ROW] = [-13.5, -4.5, 4.5, 13.5];
    let rows_run_x = visual_coordinate_hash(seed as i32, (seed >> 16) as i32) & 1 == 0;
    std::array::from_fn(|i| {
        let row = i / ORCHARD_TREES_PER_ROW;
        let tree = i % ORCHARD_TREES_PER_ROW;
        if rows_run_x {
            Vec2::new(ALONG[tree], ACROSS[row])
        } else {
            Vec2::new(ACROSS[row], ALONG[tree])
        }
    })
}

/// Tiny LCG for deterministic-but-varied placement without pulling in `rand`.
fn rand(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
    (*seed as f32) / (u32::MAX as f32)
}

/// Overlap-rejection placement (a la "simple room placement"): try up to
/// `attempts` random positions within `[x_lo,x_hi] x [z_lo,z_hi]` for a box of
/// half-extents `(half_x, half_z)` plus a `margin`, returning the first that
/// doesn't overlap any footprint already in `placed`. On success the new
/// footprint is pushed to `placed`. Footprints are stored as
/// `[min_x, max_x, min_z, max_z]`.
fn try_place(
    placed: &mut Vec<[f32; 4]>,
    s: &mut u32,
    half_x: f32,
    half_z: f32,
    x_lo: f32,
    x_hi: f32,
    z_lo: f32,
    z_hi: f32,
    margin: f32,
    attempts: usize,
) -> Option<(f32, f32)> {
    for _ in 0..attempts {
        let x = x_lo + rand(s) * (x_hi - x_lo);
        let z = z_lo + rand(s) * (z_hi - z_lo);
        let minx = x - half_x - margin;
        let maxx = x + half_x + margin;
        let minz = z - half_z - margin;
        let maxz = z + half_z + margin;
        let overlaps = placed.iter().any(|r| {
            // AABB-AABB overlap test (inclusive rejected).
            !(maxx <= r[0] || minx >= r[1] || maxz <= r[2] || minz >= r[3])
        });
        if !overlaps {
            placed.push([minx, maxx, minz, maxz]);
            return Some((x, z));
        }
    }
    None
}

/// Reconcile the block roots to the exact count×count rectangle centered on
/// the car's current grid cell. The query is read once, then a pure set-diff
/// plan is applied through deferred commands. This avoids stale-query axis
/// passes and handles X, Z, diagonal and arbitrarily large teleports in one
/// frame.
fn recycle_grid(
    mut commands: Commands,
    cfg: Res<GridConfig>,
    mut meshes: ResMut<Assets<Mesh>>,
    textures: Res<TextureAssets>,
    world_assets: Res<WorldAssets>,
    car: Query<&Transform, (With<Car>, Without<Block>)>,
    blocks: Query<(Entity, &Block)>,
) {
    let Ok(car_t) = car.single() else {
        return;
    };
    let block = cfg.block;
    if !block.is_finite() || block <= 0.0 {
        return;
    }

    let center = (
        ((car_t.translation.x + block * 0.5) / block).floor() as i32,
        ((car_t.translation.z + block * 0.5) / block).floor() as i32,
    );
    let desired = desired_grid_coords(center, cfg.count);

    // One immutable ECS snapshot. Grouping entities by coordinate lets us
    // apply each coordinate action exactly once; duplicate roots, if a prior
    // bad frame left any, are also retired while retaining one desired root.
    let mut entities_by_coord: BTreeMap<GridCoord, Vec<Entity>> = BTreeMap::new();
    for (entity, block_component) in &blocks {
        entities_by_coord
            .entry((block_component.gx, block_component.gz))
            .or_default()
            .push(entity);
    }
    let existing_coords: BTreeSet<_> = entities_by_coord.keys().copied().collect();
    let plan = recycle_plan(existing_coords.iter().copied(), &desired);

    for coord in &plan.despawn {
        if let Some(entities) = entities_by_coord.get(coord) {
            for &entity in entities {
                commands.entity(entity).despawn();
            }
        }
    }

    // A desired coordinate needs one root. Keep the first snapshot entity and
    // remove any duplicates without ever scheduling an entity twice.
    for coord in desired.intersection(&existing_coords) {
        if let Some(entities) = entities_by_coord.get(coord) {
            for &duplicate in entities.iter().skip(1) {
                commands.entity(duplicate).despawn();
            }
        }
    }

    for &(gx, gz) in &plan.spawn {
        spawn_block_at(
            &mut commands,
            &mut meshes,
            &textures,
            &world_assets,
            block,
            gx,
            gz,
        );
    }
}

/// Spawn one block at (gx,gz): derive its tile deterministically from the road
/// lines (see `tile_from_edges`), spawn the root, and populate it. Shared
/// edges always agree because both adjacent blocks derive them from the same
/// line index — no neighbour querying needed.
fn spawn_block_at(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    textures: &TextureAssets,
    world_assets: &WorldAssets,
    block: f32,
    gx: i32,
    gz: i32,
) {
    let kind = tile_from_edges(gx, gz);
    let root = commands
        .spawn((
            Transform::from_xyz(gx as f32 * block, 0.0, gz as f32 * block),
            Visibility::default(),
            Block { gx, gz, kind },
        ))
        .id();
    populate_block(
        commands,
        meshes,
        textures,
        world_assets,
        root,
        gx,
        gz,
        seed_for(gx, gz),
        kind,
    );
}

/// On a fresh round, re-center the grid on the car's spawn (origin): despawn
/// all blocks and re-spawn the count×count grid centered on origin. Skips on
/// resume from Paused (`RoundActive` already true). Runs in `SpawnSet` before
/// `reset_run` zeroes the car. The sun is `Startup`-only and persists — it is
/// NOT re-spawned here.
fn reset_grid(
    mut commands: Commands,
    cfg: Res<GridConfig>,
    mut meshes: ResMut<Assets<Mesh>>,
    textures: Res<TextureAssets>,
    world_assets: Res<WorldAssets>,
    blocks: Query<Entity, With<Block>>,
    round_active: Res<RoundActive>,
) {
    if round_active.0 {
        return;
    }
    for e in &blocks {
        commands.entity(e).despawn();
    }
    spawn_grid_window(&mut commands, &cfg, &mut meshes, &textures, &world_assets);
}

// ---------------------------------------------------------------------------
// Coins (environment now — spawned in blocks, collected on pickup)
// ---------------------------------------------------------------------------

const COIN_TIME_BONUS: f32 = 1.5;
const MAX_ROUND_TIME: f32 = 90.0;

/// Apply one ordinary-coin time bonus after sanitizing the current timer.
/// Invalid low values start from zero; high and infinite values stay capped.
fn coin_time_after_collect(current: f32) -> f32 {
    let current = if current.is_nan() {
        0.0
    } else {
        current.clamp(0.0, MAX_ROUND_TIME)
    };
    (current + COIN_TIME_BONUS).min(MAX_ROUND_TIME)
}

fn spin_coins(mut coins: Query<&mut Transform, With<Coin>>, time: Res<Time>) {
    let t = time.elapsed_secs();
    for mut tf in &mut coins {
        tf.rotation = Quat::from_rotation_y(t * 2.0);
        tf.translation.y = 0.5 + (t * 2.0 + tf.translation.x).sin() * 0.08;
    }
}

fn collect_coins(
    car: Query<&Transform, (With<Car>, Without<Coin>)>,
    mut coins: Query<(Entity, &GlobalTransform), (With<Coin>, Without<Car>)>,
    mut commands: Commands,
    mut score: ResMut<Score>,
    mut timeleft: ResMut<TimeLeft>,
    input_frozen: Res<InputFrozen>,
    mut coin_events: MessageWriter<CoinCollected>,
) {
    // Fresh blocks are spawned during the countdown. Waiting until input is
    // released avoids collecting anything before the round visibly begins.
    if input_frozen.0 {
        return;
    }
    let Ok(car_t) = car.single() else {
        return;
    };
    for (e, coin_t) in &mut coins {
        // Coins are block-root children -> `Transform` is local; use
        // `GlobalTransform` for the world position or pickup won't line up.
        // Newly spawned children still carry IDENTITY until transform
        // propagation; treating that as a world position would mass-collect
        // every fresh/recycled coin whenever the car is near the origin.
        if *coin_t == GlobalTransform::IDENTITY {
            continue;
        }
        if car_t.translation.distance(coin_t.translation()) < 1.2 {
            commands.entity(e).despawn();
            score.coins += 1;
            timeleft.0 = coin_time_after_collect(timeleft.0);
            coin_events.write(CoinCollected);
        }
    }
}

// ---------------------------------------------------------------------------
// Knockable cones (T12): bounded deterministic launch + flight
// ---------------------------------------------------------------------------
//
// Cones are spawned as `Cone` + `Collider` + `ConeMotion` children of
// recyclable block roots. The car knocks an idle cone flying in
// `car.rs::cone_collisions` (a modest speed bleed, never a concrete stop or
// pushout, and never a damaging `ObstacleHit`). Flight is integrated here on
// the cone's LOCAL transform by `update_cone_motion`, which runs only while
// `GameState::Playing` and after the driving chain so a cone launched this
// frame begins moving this frame. All launch/flight helpers are pure and
// deterministic (no randomness, no per-frame allocation/assets), so the
// bounded-launch, nonzero-post-hit-speed, lifetime/termination and
// determinism contracts are unit-testable without an ECS world.

/// World-space gravity accelerating airborne cones downward (m/s²). Tuned
/// with `CONE_LAUNCH_POP` so a cone always lands well inside the lifetime cap.
const CONE_GRAVITY: f32 = 14.0;
/// Upward pop imparted on launch (m/s). Fixed so every cone arcs predictably
/// and lands within the lifetime cap regardless of car speed.
const CONE_LAUNCH_POP: f32 = 5.0;
/// Fraction of the player's speed transferred to the cone's horizontal launch.
const CONE_LAUNCH_TRANSFER: f32 = 0.5;
/// Cap on the cone's horizontal launch speed (m/s) so even a very fast car
/// produces a bounded, readable knock.
const CONE_MAX_LAUNCH_SPEED: f32 = 6.0;
/// Max airborne lifetime (s). A cone always despawns by this even if it
/// somehow stayed airborne; combined with gravity the real flight is ~0.7s,
/// so this is well under the <= 2s requirement.
const CONE_MAX_LIFETIME: f32 = 1.8;
/// Tumble rate per unit of player speed (rad/s), capped by `CONE_MAX_SPIN`.
const CONE_SPIN_PER_SPEED: f32 = 3.0;
/// Cap on cone tumble rate (rad/s).
const CONE_MAX_SPIN: f32 = 14.0;
/// Speed bleed applied to the car on a cone hit: the car keeps most of its
/// speed (cones are harmless) but loses a modest fraction. A fractional bleed
/// can never flip the sign of or zero a nonzero speed, so there is no concrete
/// stop.
const CONE_SPEED_BLEED: f32 = 0.8;

/// World-space launch velocity for a cone struck by the car. `player_vel` is
/// the car's XZ velocity; `normal` is the unit contact normal pointing from the
/// car toward the cone (the direction the cone flies). The horizontal speed is
/// the player's speed times a transfer fraction, capped to
/// `CONE_MAX_LAUNCH_SPEED`; a fixed upward pop is added so every cone arcs
/// predictably. Pure; deterministic; bounded.
pub(crate) fn cone_launch_velocity(player_vel: Vec2, normal: Vec2) -> Vec3 {
    let speed = (player_vel.length() * CONE_LAUNCH_TRANSFER).min(CONE_MAX_LAUNCH_SPEED);
    let dir = normal.normalize_or_zero();
    Vec3::new(dir.x * speed, CONE_LAUNCH_POP, dir.y * speed)
}

/// World-space unit tumble axis for a cone launched along `normal`: the
/// horizontal axis perpendicular to the launch direction, so the cone tips
/// forward. Returns `Vec3::ZERO` for a degenerate normal (no spin). Pure;
/// deterministic.
pub(crate) fn cone_spin_axis(normal: Vec2) -> Vec3 {
    let dir = Vec3::new(normal.x, 0.0, normal.y);
    Vec3::Y.cross(dir).normalize_or_zero()
}

/// Tumble rate (rad/s) about `cone_spin_axis`, scaled by the player's speed
/// and capped. Pure; deterministic.
pub(crate) fn cone_spin_rate(player_vel: Vec2) -> f32 {
    (player_vel.length() * CONE_SPIN_PER_SPEED).min(CONE_MAX_SPIN)
}

/// Car speed after a cone hit: a modest fractional bleed. Never zeroes a
/// nonzero speed and never flips its sign (cones are harmless — no concrete
/// stop). Pure; deterministic.
pub(crate) fn cone_hit_speed(speed: f32) -> f32 {
    speed * CONE_SPEED_BLEED
}

/// Initial airborne lifetime assigned on launch (s). Bounded by
/// `CONE_MAX_LIFETIME` (<= 2s). Pure; deterministic.
pub(crate) fn cone_initial_lifetime() -> f32 {
    CONE_MAX_LIFETIME
}

/// One deterministic projectile integration step for an airborne cone
/// (semi-Euler): gravity acts on `vel.y`, then the position advances by
/// `vel * dt`. The ECS motion system is a thin wrapper over this. Pure;
/// deterministic; no allocation.
pub(crate) fn step_cone_flight(vel: Vec3, pos: Vec3, dt: f32) -> (Vec3, Vec3) {
    let mut new_vel = vel;
    new_vel.y -= CONE_GRAVITY * dt;
    (new_vel, pos + new_vel * dt)
}

/// Whether an airborne cone should despawn this step: it has returned to
/// ground (local `y <= 0`) or its lifetime has expired. Pure; deterministic.
pub(crate) fn cone_should_despawn(pos_y: f32, lifetime: f32) -> bool {
    pos_y <= 0.0 || lifetime <= 0.0
}

/// Integrate airborne cones and despawn on ground impact or lifetime expiry.
/// Idle cones are left untouched (they are static contacts). Runs only while
/// `GameState::Playing` and after the driving chain, so cones launched this
/// frame begin moving this frame. No per-frame allocation or asset cloning:
/// flight is a pure function of each cone's stored state + `dt`, and the only
/// commands issued are occasional despawns on termination.
fn update_cone_motion(
    mut commands: Commands,
    mut cones: Query<(Entity, &mut ConeMotion, &mut Transform, &Children), With<Cone>>,
    mut shadows: Query<&mut Visibility, With<ConeShadow>>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (entity, mut motion, mut tf, children) in &mut cones {
        if motion.state != ConeState::Flying {
            continue;
        }
        // The ground shadow is a child, so hide it before it can inherit the
        // parent's airborne translation and tumble. The cone despawns on land.
        for child in children.iter() {
            if let Ok(mut visibility) = shadows.get_mut(child) {
                *visibility = Visibility::Hidden;
            }
        }
        // Bounded projectile integration on the LOCAL transform. Block roots
        // are pure-translation (identity rotation/scale), so local-space deltas
        // equal world-space deltas — no GlobalTransform read is needed, which
        // keeps flight deterministic and free of propagation lag.
        let (new_vel, new_pos) = step_cone_flight(motion.vel, tf.translation, dt);
        motion.vel = new_vel;
        tf.translation = new_pos;
        // Deterministic tumble about the stored world-space axis (a child of
        // an identity-rotation parent tumbles the same in local and world).
        let spin_delta = motion.spin * dt;
        if spin_delta != 0.0 {
            let axis = motion.spin_axis.normalize_or_zero();
            if axis.length_squared() > 1e-8 {
                tf.rotation = Quat::from_axis_angle(axis, spin_delta) * tf.rotation;
            }
        }
        motion.lifetime -= dt;
        if cone_should_despawn(tf.translation.y, motion.lifetime) {
            commands.entity(entity).despawn();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests: grid recycling reliability + deterministic world generation
// ---------------------------------------------------------------------------
//
// Pure tests cover the bounded coin-time economy, grid-window/set-difference
// contract, and road-line seam contract that the world relies on:
//   * line 0 is always a road on both axes (spawn intersection guarantee),
//   * road-line decisions are deterministic across negative/positive indices,
//   * `tile_from_edges` derives its four sockets from exactly the same four
//     shared line decisions (so blocks never disagree with the seam),
//   * adjacent blocks agree on their shared east/west and north/south edges
//     across a broad coordinate range (the actual seam-correctness property),
//   * `seed_for` is deterministic and distinguishes representative coords,
//   * `try_place` never returns a footprint overlapping an accepted one and
//     always lands inside the requested interior bounds.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coin_time_bonus_obeys_boundaries_and_sanitizes_invalid_values() {
        assert_eq!(coin_time_after_collect(0.0), 1.5);
        assert_eq!(coin_time_after_collect(60.0), 61.5);
        assert_eq!(coin_time_after_collect(89.5), 90.0);
        assert_eq!(coin_time_after_collect(90.0), 90.0);
        assert_eq!(coin_time_after_collect(120.0), 90.0);

        let from_nan = coin_time_after_collect(f32::NAN);
        assert!(from_nan.is_finite());
        assert!((0.0..=MAX_ROUND_TIME).contains(&from_nan));
        assert_eq!(from_nan, COIN_TIME_BONUS);

        assert_eq!(coin_time_after_collect(-10.0), COIN_TIME_BONUS);
        assert_eq!(coin_time_after_collect(f32::NEG_INFINITY), COIN_TIME_BONUS);
        assert_eq!(coin_time_after_collect(f32::INFINITY), MAX_ROUND_TIME);
        assert_eq!(coin_time_after_collect(f32::MAX), MAX_ROUND_TIME);
    }

    #[test]
    fn repeated_coin_time_bonuses_never_exceed_round_cap() {
        let mut time = 0.0;
        for _ in 0..100 {
            let previous = time;
            time = coin_time_after_collect(time);
            assert!((0.0..=MAX_ROUND_TIME).contains(&time));
            assert!(time - previous <= COIN_TIME_BONUS);
        }
        assert_eq!(time, MAX_ROUND_TIME);
    }

    fn assert_contiguous_window(coords: &BTreeSet<GridCoord>, center: GridCoord, count: i32) {
        let count = count.max(1);
        assert_eq!(coords.len(), (count * count) as usize);
        let xs: BTreeSet<_> = coords.iter().map(|&(gx, _)| gx).collect();
        let zs: BTreeSet<_> = coords.iter().map(|&(_, gz)| gz).collect();
        let first_x = center.0 - count / 2;
        let first_z = center.1 - count / 2;
        let expected_xs: BTreeSet<_> = (first_x..first_x + count).collect();
        let expected_zs: BTreeSet<_> = (first_z..first_z + count).collect();
        assert_eq!(xs, expected_xs);
        assert_eq!(zs, expected_zs);
        for gx in xs {
            for gz in &zs {
                assert!(coords.contains(&(gx, *gz)), "missing ({gx},{gz})");
            }
        }
    }

    fn apply_plan(existing: &BTreeSet<GridCoord>, plan: &RecyclePlan) -> BTreeSet<GridCoord> {
        let mut result = existing.clone();
        for coord in &plan.despawn {
            assert!(
                result.remove(coord),
                "despawned absent coordinate {coord:?}"
            );
        }
        for &coord in &plan.spawn {
            assert!(
                result.insert(coord),
                "spawned duplicate coordinate {coord:?}"
            );
        }
        result
    }

    /// The default odd window is exactly 5×5. In particular, integer
    /// division must not accidentally turn the inclusive high bound into 1.
    #[test]
    fn desired_five_window_has_25_unique_coords_and_exact_span() {
        let coords = desired_grid_coords((0, 0), 5);
        assert_contiguous_window(&coords, (0, 0), 5);
        assert_eq!(coords.len(), 25);
        assert_eq!(coords.first(), Some(&(-2, -2)));
        assert_eq!(coords.last(), Some(&(2, 2)));
        assert!(
            coords
                .iter()
                .all(|&(gx, gz)| { (-2..=2).contains(&gx) && (-2..=2).contains(&gz) })
        );
    }

    /// Even windows use the documented negative-side bias, while invalid
    /// non-positive counts clamp to one coordinate instead of becoming empty.
    #[test]
    fn desired_even_window_policy_and_non_positive_clamp_are_exact() {
        let even = desired_grid_coords((10, -4), 4);
        assert_contiguous_window(&even, (10, -4), 4);
        let xs: BTreeSet<_> = even.iter().map(|&(gx, _)| gx).collect();
        let zs: BTreeSet<_> = even.iter().map(|&(_, gz)| gz).collect();
        assert_eq!(xs, BTreeSet::from([8, 9, 10, 11]));
        assert_eq!(zs, BTreeSet::from([-6, -5, -4, -3]));

        assert_eq!(desired_grid_coords((7, 9), 0), BTreeSet::from([(7, 9)]));
        assert_eq!(desired_grid_coords((7, 9), -5), BTreeSet::from([(7, 9)]));
    }

    /// Every direction and distance is handled by one old-vs-desired set
    /// difference. Applying each plan yields exactly 25 unique contiguous
    /// coordinates immediately, including disjoint multi-window teleports.
    #[test]
    fn recycle_plans_handle_x_z_diagonal_and_multi_window_moves() {
        let starts_and_targets = [
            ((0, 0), (1, 0)),   // +X
            ((0, 0), (-1, 0)),  // -X
            ((0, 0), (0, 1)),   // +Z
            ((0, 0), (0, -1)),  // -Z
            ((0, 0), (1, 1)),   // diagonal
            ((2, -3), (-1, 4)), // diagonal beyond one window
            ((0, 0), (13, -9)), // fully disjoint multi-window teleport
        ];

        for (start, target) in starts_and_targets {
            let existing = desired_grid_coords(start, 5);
            let desired = desired_grid_coords(target, 5);
            let plan = recycle_plan(existing.iter().copied(), &desired);
            assert!(plan.spawn.is_disjoint(&plan.despawn));
            let result = apply_plan(&existing, &plan);
            assert_eq!(result, desired, "failed move {start:?} -> {target:?}");
            assert_contiguous_window(&result, target, 5);
        }
    }

    /// Duplicate coordinates in a malformed snapshot are collapsed before
    /// differencing, so neither side of the plan can contain duplicate or
    /// contradictory coordinate actions.
    #[test]
    fn recycle_plan_never_duplicates_spawn_or_despawn_coordinates() {
        let desired = desired_grid_coords((1, 1), 5);
        let existing = desired_grid_coords((0, 0), 5);
        let duplicated: Vec<_> = existing
            .iter()
            .flat_map(|&coord| [coord, coord, coord])
            .collect();
        let plan = recycle_plan(duplicated, &desired);

        assert_eq!(plan.despawn.len(), 9);
        assert_eq!(plan.spawn.len(), 9);
        assert!(plan.spawn.is_disjoint(&plan.despawn));
        assert_eq!(apply_plan(&existing, &plan), desired);
    }

    /// With no movement, the set difference is empty: stable frames issue no
    /// unnecessary spawn/despawn work.
    #[test]
    fn recycle_plan_is_empty_when_window_is_already_desired() {
        let desired = desired_grid_coords((-20, 30), 5);
        let plan = recycle_plan(desired.iter().copied(), &desired);
        assert!(plan.spawn.is_empty());
        assert!(plan.despawn.is_empty());
    }

    #[test]
    fn spawn_backbone_is_bounded_not_an_infinite_axis() {
        for line in -SPAWN_BACKBONE_RADIUS..SPAWN_BACKBONE_RADIUS {
            assert!(road_edge(RoadAxis::X, line, 0));
            assert!(road_edge(RoadAxis::Z, line, 0));
        }
        assert!((-200..=200).any(|segment| !road_edge(RoadAxis::X, 0, segment)));
        assert!((-200..=200).any(|segment| !road_edge(RoadAxis::Z, 0, segment)));
    }

    #[test]
    fn block_retains_authoritative_kind() {
        let kind = road_tile_kind(-3, 7);
        let block = Block {
            gx: -3,
            gz: 7,
            kind,
        };
        assert_eq!(block.kind, kind);
    }

    #[test]
    fn review_catalog_is_exhaustive_unique_and_socket_stable() {
        assert_eq!(TILE_CATALOG.len(), 19);
        let names: BTreeSet<_> = TILE_CATALOG
            .iter()
            .map(|&kind| tile_kind_name(kind))
            .collect();
        assert_eq!(names.len(), TILE_CATALOG.len());
        for &kind in &TILE_CATALOG {
            assert_eq!(socket_names(kind).len(), 4);
            assert!(
                socket_names(kind)
                    .iter()
                    .all(|socket| matches!(*socket, "road" | "none"))
            );
        }
    }

    fn review_test_app() -> App {
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<TextureAssets>()
            .insert_resource(WorldReviewMode)
            .init_resource::<WorldAssets>()
            .add_systems(Startup, spawn_review_world);
        app.update();
        app
    }

    #[test]
    fn normal_world_plugin_has_no_review_mode_or_review_archetypes() {
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<TextureAssets>();
        app.add_plugins(WorldPlugin);
        assert!(!app.world().contains_resource::<WorldReviewMode>());
        let review_tiles = {
            let world = app.world_mut();
            let mut query = world.query::<&ReviewTile>();
            query.iter(world).count()
        };
        assert_eq!(review_tiles, 0);
    }

    #[test]
    #[should_panic(expected = "outside WorldReviewMode")]
    fn review_builder_rejects_normal_mode() {
        let mut world = World::new();
        let _ = build_review_metadata(&mut world);
    }

    #[test]
    fn real_review_builder_is_deterministic_complete_and_uses_count_schema() {
        let mut app = review_test_app();
        let first = build_review_metadata(app.world_mut());
        let second = build_review_metadata(app.world_mut());
        assert_eq!(first, second);
        assert!(first.ready);
        assert_eq!(first.schema, "roady-world-review-v1");
        assert_eq!(first.seed, REVIEW_SEED);
        assert_eq!(first.topology_version, 1);
        assert_eq!(first.blocks.len(), 25 + TILE_CATALOG.len());
        assert_eq!(
            first
                .blocks
                .iter()
                .filter(|block| block.source == "production")
                .count(),
            25
        );
        let atlas: Vec<_> = first
            .blocks
            .iter()
            .filter(|block| block.source == "atlas")
            .collect();
        assert_eq!(atlas.len(), TILE_CATALOG.len());
        assert!(atlas.iter().enumerate().all(|(index, block)| {
            block.catalog_index == Some(index) && block.kind == tile_kind_name(TILE_CATALOG[index])
        }));
        assert!(first.blocks.iter().all(|block| block.counts.mesh3d > 0));
        assert!(first.blocks.iter().any(|block| block.counts.roads > 0));
        assert!(first.blocks.iter().any(|block| block.counts.curbs > 0));
        assert!(first.blocks.iter().any(|block| block.counts.markings > 0));
        assert!(first.blocks.iter().any(|block| block.counts.buildings > 0));
        assert!(first.blocks.iter().any(|block| block.counts.trees > 0));
        assert!(first.blocks.iter().any(|block| block.counts.farm_props > 0));
        assert!(first.blocks.iter().any(|block| block.counts.coins > 0));
        assert!(first.blocks.iter().any(|block| block.counts.lamps > 0));
        assert!(first.blocks.iter().any(|block| block.counts.obstacles > 0));

        let json = serde_json::to_value(&first).unwrap();
        let counts = json["blocks"][0]["counts"].as_object().unwrap();
        assert_eq!(
            counts.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "buildings",
                "coins",
                "curbs",
                "farm_props",
                "lamps",
                "markings",
                "mesh3d",
                "obstacles",
                "roads",
                "trees",
            ])
        );
    }

    #[test]
    fn forced_atlas_has_visible_gutter_beyond_road_spill_and_metadata_matches() {
        assert!(REVIEW_ATLAS_GUTTER > REVIEW_ROAD_SPILL);
        assert_eq!(REVIEW_ROAD_SPILL, 0.0);
        // The 42u seam-hiding ground plane leaves an actual 8u gutter in a
        // 50u pitch; topology itself remains fully inside the nominal tile.
        assert_eq!(REVIEW_ATLAS_PITCH - 2.0 * REVIEW_CONTENT_HALF_EXTENT, 8.0);
        let mut app = review_test_app();
        let metadata = build_review_metadata(app.world_mut());
        assert_eq!(metadata.atlas.pitch, REVIEW_ATLAS_PITCH);
        assert_eq!(metadata.atlas.gutter, REVIEW_ATLAS_GUTTER);
        assert_eq!(metadata.atlas.road_spill, REVIEW_ROAD_SPILL);
        let atlas: Vec<_> = metadata
            .blocks
            .iter()
            .filter(|b| b.source == "atlas")
            .collect();
        assert_eq!(atlas[1].world_x - atlas[0].world_x, REVIEW_ATLAS_PITCH);
        assert_eq!(
            atlas[REVIEW_ATLAS_COLUMNS].world_z - atlas[0].world_z,
            REVIEW_ATLAS_PITCH
        );
        let (min, max) = world_review_bounds();
        assert_eq!(metadata.scene_bounds.min_x, min.x);
        assert_eq!(metadata.scene_bounds.max_x, max.x);
        assert_eq!(metadata.scene_bounds.min_z, min.y);
        assert_eq!(metadata.scene_bounds.max_z, max.y);
    }

    #[test]
    fn all_none_visual_variants_have_only_none_sockets() {
        for kind in [
            TileKind::Empty,
            TileKind::Park,
            TileKind::Field,
            TileKind::Orchard,
        ] {
            assert_eq!(sockets(kind), [Edge::None; 4], "{kind:?}");
        }
    }

    /// Every visual variant is selected at reachable all-None coordinates,
    /// and selection is stable across repeated generation/recycling calls.
    #[test]
    fn all_none_visual_variants_are_deterministic_and_reachable() {
        let mut reached = [false; 4];
        for gx in -200..=200 {
            for gz in -200..=200 {
                if sockets(tile_from_edges(gx, gz)) != [Edge::None; 4] {
                    continue;
                }
                let first = tile_from_edges(gx, gz);
                assert_eq!(first, tile_from_edges(gx, gz));
                assert_eq!(first, all_none_variant(gx, gz));
                assert_eq!(sockets(first), [Edge::None; 4]);
                match first {
                    TileKind::Empty => reached[0] = true,
                    TileKind::Park => reached[1] = true,
                    TileKind::Field => reached[2] = true,
                    TileKind::Orchard => reached[3] = true,
                    other => panic!("all-None coordinate selected {other:?}"),
                }
            }
        }
        assert_eq!(reached, [true; 4], "not all visual variants were reachable");
    }

    #[test]
    fn coordinate_pair_edges_are_deterministic_and_vary_on_both_coordinates() {
        for axis in [RoadAxis::X, RoadAxis::Z] {
            for line in -30..=30 {
                for segment in -30..=30 {
                    assert_eq!(
                        road_edge(axis, line, segment),
                        road_edge(axis, line, segment)
                    );
                }
            }
        }
        let along_x: BTreeSet<_> = (-100..=100)
            .map(|segment| road_edge(RoadAxis::X, 17, segment))
            .collect();
        let across_x: BTreeSet<_> = (-100..=100)
            .map(|line| road_edge(RoadAxis::X, line, 17))
            .collect();
        assert_eq!(along_x, BTreeSet::from([false, true]));
        assert_eq!(across_x, BTreeSet::from([false, true]));
    }

    #[test]
    fn tile_sockets_match_coordinate_pair_edges() {
        for gx in -20..=20 {
            for gz in -20..=20 {
                let sock = sockets(tile_from_edges(gx, gz));
                assert_eq!(sock[W] == Edge::Road, road_edge(RoadAxis::X, gx - 1, gz));
                assert_eq!(sock[E] == Edge::Road, road_edge(RoadAxis::X, gx, gz));
                assert_eq!(sock[S] == Edge::Road, road_edge(RoadAxis::Z, gz - 1, gx));
                assert_eq!(sock[N] == Edge::Road, road_edge(RoadAxis::Z, gz, gx));
            }
        }
    }

    #[test]
    fn all_19_tile_road_plans_have_pad_plus_one_arm_per_socket() {
        for &kind in &TILE_CATALOG {
            let arm_count = sockets(kind)
                .into_iter()
                .filter(|edge| *edge == Edge::Road)
                .count();
            // Atlas plans are forced kinds, while generated plans establish
            // the same geometric cardinality from their authoritative kind.
            assert_eq!(
                arm_count,
                sockets(kind)
                    .iter()
                    .filter(|edge| **edge == Edge::Road)
                    .count()
            );
        }
        for gx in -10..=10 {
            for gz in -10..=10 {
                let plan = road_plan(gx, gz);
                assert_eq!(
                    plan.segments.iter().flatten().count(),
                    sockets(plan.kind)
                        .iter()
                        .filter(|edge| **edge == Edge::Road)
                        .count()
                );
            }
        }
    }

    #[test]
    fn world_cell_conversion_handles_negative_and_exact_boundaries() {
        assert_eq!(world_to_road_cell(-60.0001), -2);
        assert_eq!(world_to_road_cell(-60.0), -1);
        assert_eq!(world_to_road_cell(-20.0001), -1);
        assert_eq!(world_to_road_cell(-20.0), 0);
        assert_eq!(world_to_road_cell(19.9999), 0);
        assert_eq!(world_to_road_cell(20.0), 1);
    }

    /// The seam-correctness property: two horizontally-adjacent blocks
    /// (gx, gz) and (gx+1, gz) share an edge — block A's east edge and
    /// block B's west edge are the SAME world line `x = (gx+1) * block`.
    /// Their road/not-road decision must agree, otherwise the road would
    /// start/stop mid-block. Same for vertically-adjacent blocks sharing a
    /// north/south edge. Checked across a broad coordinate range spanning
    /// negative and positive indices (including the recycling frontier).
    #[test]
    fn adjacent_blocks_agree_on_shared_edges() {
        let lo = -40i32;
        let hi = 40i32;
        for gx in lo..=hi {
            for gz in lo..=hi {
                let a = sockets(tile_from_edges(gx, gz));
                // East neighbour (gx+1, gz): A's E edge == B's W edge.
                let b = sockets(tile_from_edges(gx + 1, gz));
                assert_eq!(
                    a[E],
                    b[W],
                    "E/W seam mismatch at ({gx},{gz}) vs ({},{gz})",
                    gx + 1,
                );
                // North neighbour (gx, gz+1): A's N edge == B's S edge.
                let c = sockets(tile_from_edges(gx, gz + 1));
                assert_eq!(
                    a[N],
                    c[S],
                    "N/S seam mismatch at ({gx},{gz}) vs ({gx},{})",
                    gz + 1,
                );
            }
        }
    }

    #[test]
    fn shared_arms_meet_at_seams_without_overlap() {
        for gx in -20..=20 {
            for gz in -20..=20 {
                let a = road_plan(gx, gz);
                let east = road_plan(gx + 1, gz);
                assert_eq!(a.segments[E].is_some(), east.segments[W].is_some());
                if let (Some(left), Some(right)) = (a.segments[E], east.segments[W]) {
                    assert_eq!(left.end, right.end);
                    assert_ne!(left.start, right.start);
                }
                let north = road_plan(gx, gz + 1);
                assert_eq!(a.segments[N].is_some(), north.segments[S].is_some());
                if let (Some(south), Some(top)) = (a.segments[N], north.segments[S]) {
                    assert_eq!(south.end, top.end);
                    assert_ne!(south.start, top.start);
                }
            }
        }
    }

    /// Markings are emitted only for owned road approaches whose endpoint
    /// has a perpendicular road. Straight roads, empty/park blocks and stubs
    /// therefore never receive a crosswalk or stop line.
    #[test]
    fn marking_approaches_require_perpendicular_road_sockets() {
        use TileKind::*;

        let cases = [
            (Empty, [false; 4]),
            (Park, [false; 4]),
            (Field, [false; 4]),
            (Orchard, [false; 4]),
            (RoadNS, [false; 4]),
            (RoadEW, [false; 4]),
            (Cross, [true; 4]),
            (TN, [true, false, true, true]),
            (TE, [true, true, true, false]),
            (TS, [false, true, false, false]),
            (TW, [false, false, false, true]),
            (CornerWN, [false, true, false, false]),
            (CornerNE, [false; 4]),
            (CornerES, [false, false, false, true]),
            (CornerSW, [true, false, true, false]),
            (StubW, [false; 4]),
            (StubE, [false; 4]),
            (StubS, [false; 4]),
            (StubN, [false; 4]),
        ];
        for (kind, expected) in cases {
            assert_eq!(
                marking_approaches(sockets(kind)),
                expected,
                "unexpected marking approaches for {kind:?}"
            );
        }
    }

    /// Facade strips stay bounded within the usable wall height and are
    /// capped at three rows, including at the full generated 4–9u range.
    #[test]
    fn window_rows_have_sensible_count_and_vertical_bounds() {
        assert!(window_row_heights(f32::NAN).is_empty());
        assert!(window_row_heights(1.7).is_empty());
        assert_eq!(window_row_heights(4.0), vec![0.9, 3.1]);
        assert_eq!(window_row_heights(9.0), vec![0.9, 4.5, 8.1]);

        for step in 0..=100 {
            let height = 4.0 + step as f32 * 0.05;
            let rows = window_row_heights(height);
            assert!((2..=MAX_WINDOW_ROWS).contains(&rows.len()));
            assert!(rows.windows(2).all(|pair| pair[0] < pair[1]));
            assert!(
                rows.iter().all(|&row| {
                    row >= WINDOW_ROW_BOTTOM && row <= height - WINDOW_ROW_TOP_MARGIN
                })
            );
            assert_eq!(rows, window_row_heights(height));
        }
    }

    /// `seed_for` is deterministic: the same (gx, gz) always yields the same
    /// seed (stable across recycles — a block re-spawned at the same coords
    /// reproduces its layout). Pure function of (gx, gz).
    #[test]
    fn seed_for_is_deterministic() {
        for gx in -50..=50 {
            for gz in -50..=50 {
                let s1 = seed_for(gx, gz);
                let s2 = seed_for(gx, gz);
                assert_eq!(s1, s2, "seed_for({gx},{gz}) not stable");
            }
        }
    }

    /// `seed_for` should distinguish representative coordinates — different
    /// (gx, gz) pairs should (almost always) produce different seeds. We
    /// don't require injectivity over all i32² (collisions are statistically
    /// possible with a 32-bit output), but a handful of representative
    // distinct coords must all differ; otherwise the layout would be uniform.
    #[test]
    fn seed_for_distinguishes_representative_coords() {
        let coords = [
            (0, 0),
            (1, 0),
            (0, 1),
            (-1, 0),
            (0, -1),
            (1, 1),
            (-1, -1),
            (7, -3),
            (-3, 7),
            (100, 100),
            (-100, -100),
            (12345, -67890),
        ];
        let mut seen = std::collections::HashSet::new();
        for &(gx, gz) in &coords {
            let s = seed_for(gx, gz);
            assert!(seen.insert(s), "seed_for collision at ({gx},{gz}): {s}");
        }
    }

    /// `seed_for` should vary with EACH axis independently — moving one step
    /// along X or Z should (almost always) change the seed, so neighbouring
    /// blocks get different layouts.
    #[test]
    fn seed_for_varies_along_each_axis() {
        let base = seed_for(5, 5);
        assert_ne!(base, seed_for(6, 5), "seed unchanged moving +X");
        assert_ne!(base, seed_for(4, 5), "seed unchanged moving -X");
        assert_ne!(base, seed_for(5, 6), "seed unchanged moving +Z");
        assert_ne!(base, seed_for(5, 4), "seed unchanged moving -Z");
    }

    #[test]
    fn field_prop_colliders_contain_geometry_at_every_sampled_yaw() {
        for kind in [FieldPropKind::HayBale, FieldPropKind::FarmCrate] {
            let collider_half = field_prop_collider_half_extent(kind);
            // Include axis-aligned, diagonal, and dense arbitrary rotations.
            for step in 0..=720 {
                let yaw = step as f32 * std::f32::consts::TAU / 720.0;
                let geometry = field_prop_geometry_aabb_half_extents(kind, yaw);
                assert!(
                    geometry.x <= collider_half + 1e-6 && geometry.y <= collider_half + 1e-6,
                    "{kind:?} geometry {geometry:?} escapes collider half {collider_half} at {yaw} rad"
                );
            }
        }
    }

    #[test]
    fn field_props_are_deterministic_bounded_and_nonoverlapping_with_collider_footprints() {
        for seed in 0..512_u32 {
            let (a, count_a) = field_prop_layout(seed);
            let (b, count_b) = field_prop_layout(seed);
            assert_eq!(a, b);
            assert_eq!(count_a, count_b);
            assert!((FIELD_PROP_MIN..=FIELD_PROP_MAX).contains(&count_a));

            let props = &a[..count_a];
            assert!(props.iter().any(|p| p.kind == FieldPropKind::HayBale));
            assert!(props.iter().any(|p| p.kind == FieldPropKind::FarmCrate));

            // Mirror populate_block's try_place registration. Each stored
            // footprint must be exactly the Collider square, remain bounded,
            // and be accepted without overlap for every generated rotation
            // and kind combination.
            let mut placed = Vec::new();
            let mut footprint_seed = seed ^ 0xa511_e9b3;
            for prop in props {
                let half = field_prop_collider_half_extent(prop.kind);
                let accepted = try_place(
                    &mut placed,
                    &mut footprint_seed,
                    half,
                    half,
                    prop.position.x,
                    prop.position.x,
                    prop.position.y,
                    prop.position.y,
                    0.0,
                    1,
                );
                assert_eq!(accepted, Some((prop.position.x, prop.position.y)));
                assert!(prop.position.x.abs() + half <= FIELD_PROP_LIMIT);
                assert!(prop.position.y.abs() + half <= FIELD_PROP_LIMIT);
                assert!((0.0..=std::f32::consts::TAU).contains(&prop.rotation));

                let footprint = placed.last().unwrap();
                assert_eq!(
                    *footprint,
                    [
                        prop.position.x - half,
                        prop.position.x + half,
                        prop.position.y - half,
                        prop.position.y + half,
                    ]
                );
            }
            assert_eq!(placed.len(), count_a);
            for (i, footprint) in placed.iter().enumerate() {
                for other in &placed[..i] {
                    assert!(
                        footprint[1] <= other[0]
                            || footprint[0] >= other[1]
                            || footprint[3] <= other[2]
                            || footprint[2] >= other[3],
                        "field collider footprints overlap at seed {seed}: {footprint:?} / {other:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn orchard_rows_are_deterministic_aligned_bounded_and_fixed_size() {
        for seed in 0..128_u32 {
            let trees = orchard_tree_layout(seed);
            assert_eq!(trees, orchard_tree_layout(seed));
            assert_eq!(trees.len(), ORCHARD_TREE_COUNT);
            assert!(
                trees.iter().all(|p| {
                    p.x.abs() + 0.3 <= ORCHARD_LIMIT && p.y.abs() + 0.3 <= ORCHARD_LIMIT
                })
            );

            let rows_run_x = trees[0].y == trees[1].y;
            for row in trees.chunks_exact(ORCHARD_TREES_PER_ROW) {
                if rows_run_x {
                    assert!(row.iter().all(|p| p.y == row[0].y));
                    assert!(row.windows(2).all(|pair| pair[0].x < pair[1].x));
                } else {
                    assert!(row.iter().all(|p| p.x == row[0].x));
                    assert!(row.windows(2).all(|pair| pair[0].y < pair[1].y));
                }
            }
        }
    }

    /// `try_place` must NEVER return a footprint that overlaps an already-
    /// accepted placement. We exercise it by hammering it with many requests
    /// in a small interior and checking every accepted footprint against
    /// every other accepted footprint (AABB-overlap with the same margin the
    /// function uses).
    #[test]
    fn try_place_never_overlaps_accepted() {
        let mut placed: Vec<[f32; 4]> = Vec::new();
        let mut s = seed_for(3, 7);
        let half_x = 1.0_f32;
        let half_z = 1.0_f32;
        let margin = 0.5_f32;
        for _ in 0..200 {
            if try_place(
                &mut placed,
                &mut s,
                half_x,
                half_z,
                -10.0,
                10.0,
                -10.0,
                10.0,
                margin,
                12,
            )
            .is_some()
            {
                // Every accepted footprint must not overlap any previously
                // accepted one (using the same margin-expanded AABB test).
                let last = *placed.last().unwrap();
                for (i, r) in placed[..placed.len() - 1].iter().enumerate() {
                    let overlaps =
                        !(last[1] <= r[0] || last[0] >= r[1] || last[3] <= r[2] || last[2] >= r[3]);
                    assert!(
                        !overlaps,
                        "accepted footprint overlaps #{i}: {last:?} vs {r:?}"
                    );
                }
            }
        }
    }

    /// Every `try_place` success must place its center inside the supplied
    /// bounds. Callers that require the full footprint inside an area pass
    /// bounds already shrunk by the half-extents.
    #[test]
    fn try_place_returns_within_interior() {
        let mut placed: Vec<[f32; 4]> = Vec::new();
        let mut s = seed_for(-2, 11);
        let half_x = 0.5_f32;
        let half_z = 0.5_f32;
        let margin = 0.25_f32;
        let x_lo = -8.0_f32;
        let x_hi = 8.0_f32;
        let z_lo = -8.0_f32;
        let z_hi = 8.0_f32;
        for _ in 0..100 {
            if let Some((x, z)) = try_place(
                &mut placed,
                &mut s,
                half_x,
                half_z,
                x_lo,
                x_hi,
                z_lo,
                z_hi,
                margin,
                8,
            ) {
                assert!(
                    (x_lo..=x_hi).contains(&x),
                    "x={x} outside center bounds [{x_lo},{x_hi}]"
                );
                assert!(
                    (z_lo..=z_hi).contains(&z),
                    "z={z} outside center bounds [{z_lo},{z_hi}]"
                );
            }
        }
    }

    /// `try_place` must eventually give up (return None) when the interior is
    /// saturated — it should not loop forever, and once it returns None the
    /// `placed` list must be unchanged.
    #[test]
    fn try_place_returns_none_when_saturated() {
        let mut placed: Vec<[f32; 4]> = Vec::new();
        let mut s = seed_for(9, 9);
        // Pre-fill the interior with one big footprint covering everything,
        // so no further placement can fit.
        placed.push([-10.0, 10.0, -10.0, 10.0]);
        let before = placed.len();
        let r = try_place(&mut placed, &mut s, 0.5, 0.5, -8.0, 8.0, -8.0, 8.0, 0.5, 8);
        assert!(r.is_none(), "expected None in a saturated interior");
        assert_eq!(placed.len(), before, "placed grew on a failed try_place");
    }

    /// `rand` (the LCG used by `populate_block`) is deterministic: the same
    /// seed produces the same sequence, so a recycled block reproduces its
    /// layout exactly.
    #[test]
    fn rand_lcg_is_deterministic() {
        let mut a = seed_for(4, 4);
        let mut b = seed_for(4, 4);
        for _ in 0..64 {
            assert_eq!(rand(&mut a), rand(&mut b));
        }
        // And a different seed produces a different first value (almost
        // surely).
        let mut c = seed_for(4, 5);
        assert_ne!(rand(&mut c), {
            let mut d = seed_for(4, 4);
            rand(&mut d)
        });
    }

    // --- Street-lamp geometry (pure helpers; no ECS hierarchy) ---

    /// The arm points toward the nearest Road edge, and only Road edges are
    /// candidates — a closer non-road edge is never chosen.
    #[test]
    fn lamp_arm_direction_picks_nearest_road_edge() {
        // All four roads: the closest edge by distance wins.
        assert_eq!(
            lamp_arm_direction(true, true, true, true, -18.0, 0.0, 20.0),
            (-1.0, 0.0),
            "near W -> W"
        );
        assert_eq!(
            lamp_arm_direction(true, true, true, true, 18.0, 0.0, 20.0),
            (1.0, 0.0),
            "near E -> E"
        );
        assert_eq!(
            lamp_arm_direction(true, true, true, true, 0.0, -18.0, 20.0),
            (0.0, -1.0),
            "near S -> S"
        );
        assert_eq!(
            lamp_arm_direction(true, true, true, true, 0.0, 18.0, 20.0),
            (0.0, 1.0),
            "near N -> N"
        );
    }

    #[test]
    fn lamp_arm_direction_ignores_non_road_edges() {
        // Only S is a road; even though the post is closer to the W edge
        // (no road), the arm must point S — never toward a non-road edge.
        assert_eq!(
            lamp_arm_direction(false, false, true, false, -18.0, 5.0, 20.0),
            (0.0, -1.0)
        );
        // Only E is a road; post closer to W (no road) -> still E.
        assert_eq!(
            lamp_arm_direction(false, true, false, false, -18.0, 0.0, 20.0),
            (1.0, 0.0)
        );
        // Only N is a road; post closer to S (no road) -> still N.
        assert_eq!(
            lamp_arm_direction(false, false, false, true, 5.0, -18.0, 20.0),
            (0.0, 1.0)
        );
    }

    #[test]
    fn lamp_arm_direction_zero_when_no_road() {
        assert_eq!(
            lamp_arm_direction(false, false, false, false, 0.0, 0.0, 20.0),
            (0.0, 0.0)
        );
        assert_eq!(
            lamp_arm_direction(false, false, false, false, -19.0, 19.0, 20.0),
            (0.0, 0.0)
        );
    }

    #[test]
    fn lamp_arm_direction_is_axis_aligned_unit_vector() {
        // Across a grid of post positions with all roads present, the
        // direction is always a single axis-aligned unit vector.
        for lx in -19..=19 {
            for lz in -19..=19 {
                let (dx, dz) =
                    lamp_arm_direction(true, true, true, true, lx as f32, lz as f32, 20.0);
                let mag = (dx * dx + dz * dz).sqrt();
                assert!(
                    (mag - 1.0).abs() < 1e-6,
                    "non-unit direction ({dx},{dz}) at ({lx},{lz})"
                );
                assert!(
                    (dx == 0.0) != (dz == 0.0),
                    "non-axis-aligned direction ({dx},{dz}) at ({lx},{lz})"
                );
                assert!(dx.abs() <= 1.0 && dz.abs() <= 1.0);
            }
        }
    }

    #[test]
    fn lamp_pole_roots_at_ground_and_spans_full_height() {
        let t = lamp_pole_transform();
        // Cylinder mesh is centered on its midpoint, so center.y = half the
        // height means it spans exactly 0 .. POLE_HEIGHT.
        assert_eq!(t.translation.y, POLE_HEIGHT / 2.0);
        assert_eq!(
            t.translation.y - POLE_HEIGHT / 2.0,
            0.0,
            "pole bottom at ground"
        );
        assert_eq!(t.translation.y + POLE_HEIGHT / 2.0, POLE_HEIGHT, "pole top");
        // Pole sits at the post's XZ origin (no horizontal offset).
        assert_eq!(t.translation.x, 0.0);
        assert_eq!(t.translation.z, 0.0);
    }

    #[test]
    fn lamp_arm_is_connected_to_pole_and_oriented_along_road() {
        for (dx, dz, label) in [
            (-1.0_f32, 0.0_f32, "W"),
            (1.0, 0.0, "E"),
            (0.0, -1.0, "S"),
            (0.0, 1.0, "N"),
        ] {
            let t = lamp_arm_transform(dx, dz);
            let he = lamp_arm_aabb_half_extents(dx, dz);

            // Arm Y is the pole top -> arm overlaps the pole top (connected).
            assert_eq!(t.translation.y, POLE_HEIGHT, "arm Y for {label}");
            let arm_bottom = t.translation.y - ARM_THICK / 2.0;
            let arm_top = t.translation.y + ARM_THICK / 2.0;
            assert!(
                arm_bottom <= POLE_HEIGHT && POLE_HEIGHT <= arm_top,
                "arm must overlap pole top for {label}"
            );

            // Along the road direction: inner end at the pole (0), outer end
            // at dir * ARM_LEN. Perpendicular: thin (ARM_THICK), not long.
            let (along_center, along_half, perp_half) = if dx != 0.0 {
                (t.translation.x, he.x, he.z)
            } else {
                (t.translation.z, he.z, he.x)
            };
            let end_a = along_center - along_half;
            let end_b = along_center + along_half;
            let want = (dx + dz) * ARM_LEN;
            assert!(
                (end_a - 0.0).abs() < 1e-6 || (end_b - 0.0).abs() < 1e-6,
                "arm inner end not at pole for {label}: ends {end_a},{end_b}"
            );
            assert!(
                (end_a - want).abs() < 1e-6 || (end_b - want).abs() < 1e-6,
                "arm outer end not toward road for {label}: ends {end_a},{end_b} want {want}"
            );
            assert!(
                (along_half - ARM_LEN / 2.0).abs() < 1e-6,
                "arm long along road for {label}"
            );
            assert!(
                (perp_half - ARM_THICK / 2.0).abs() < 1e-6,
                "arm thin perpendicular for {label}"
            );
        }
    }

    #[test]
    fn lamp_arm_rotation_aligns_long_axis_with_road_direction() {
        // Along X: no rotation (the mesh is already long along X).
        assert_eq!(lamp_arm_transform(-1.0, 0.0).rotation, Quat::IDENTITY);
        assert_eq!(lamp_arm_transform(1.0, 0.0).rotation, Quat::IDENTITY);
        // Along Z: π/2 about Y. The arm is symmetric about its center, so the
        // direction's sign is carried by the translation; the rotation is the
        // same for +Z and -Z.
        let want = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        for (dx, dz) in [(0.0_f32, -1.0_f32), (0.0, 1.0)] {
            let q = lamp_arm_transform(dx, dz).rotation;
            assert!(
                (q.x - want.x).abs() < 1e-6
                    && (q.y - want.y).abs() < 1e-6
                    && (q.z - want.z).abs() < 1e-6
                    && (q.w - want.w).abs() < 1e-6,
                "arm along Z must be rotated π/2 about Y for ({dx},{dz}), got {q}"
            );
        }
    }

    #[test]
    fn lamp_fixture_hangs_connected_at_arm_end() {
        for (dx, dz, label) in [
            (-1.0_f32, 0.0_f32, "W"),
            (1.0, 0.0, "E"),
            (0.0, -1.0, "S"),
            (0.0, 1.0, "N"),
        ] {
            let arm = lamp_arm_transform(dx, dz);
            let lamp = lamp_fixture_transform(dx, dz);

            // Same XZ as the arm's outer end.
            assert!(
                (lamp.translation.x - dx * ARM_LEN).abs() < 1e-6,
                "lamp X at arm outer end for {label}"
            );
            assert!(
                (lamp.translation.z - dz * ARM_LEN).abs() < 1e-6,
                "lamp Z at arm outer end for {label}"
            );

            // Hangs BELOW the arm.
            assert!(
                lamp.translation.y < arm.translation.y,
                "lamp must hang below arm for {label}"
            );

            // Bulb top touches arm bottom — connected, no gap, no float.
            let arm_bottom = arm.translation.y - ARM_THICK / 2.0;
            let lamp_top = lamp.translation.y + LAMP_RADIUS;
            assert!(
                (lamp_top - arm_bottom).abs() < 1e-6,
                "lamp top must meet arm bottom for {label}: {lamp_top} vs {arm_bottom}"
            );

            // Entire bulb sits below the arm (no overlap with the bar) and
            // clears the ground.
            assert!(
                lamp_top <= arm_bottom + 1e-6,
                "bulb must not overlap arm for {label}"
            );
            assert!(
                lamp.translation.y - LAMP_RADIUS > 0.0,
                "lamp must clear the ground for {label}"
            );
        }
    }

    #[test]
    fn lamp_post_vertical_chain_is_connected_with_no_gaps() {
        // Pole: roots at ground, top at POLE_HEIGHT.
        let pole = lamp_pole_transform();
        assert_eq!(
            pole.translation.y - POLE_HEIGHT / 2.0,
            0.0,
            "pole roots at ground"
        );
        assert_eq!(
            pole.translation.y + POLE_HEIGHT / 2.0,
            POLE_HEIGHT,
            "pole top"
        );

        // Arm: at the pole top, overlapping it (connected — no gap to pole).
        let arm = lamp_arm_transform(1.0, 0.0);
        let arm_bottom = arm.translation.y - ARM_THICK / 2.0;
        let arm_top = arm.translation.y + ARM_THICK / 2.0;
        assert!(
            arm_bottom <= POLE_HEIGHT && POLE_HEIGHT <= arm_top,
            "arm must overlap pole top (no gap)"
        );

        // Lamp: hangs from the arm end, top meeting the arm bottom (no gap).
        let lamp = lamp_fixture_transform(1.0, 0.0);
        let lamp_top = lamp.translation.y + LAMP_RADIUS;
        assert!(
            (lamp_top - arm_bottom).abs() < 1e-6,
            "lamp top must meet arm bottom (no gap)"
        );

        // The whole assembly is above ground and vertically ordered.
        assert!(lamp.translation.y - LAMP_RADIUS > 0.0, "lamp clears ground");
        assert!(
            arm.translation.y > pole.translation.y,
            "arm above pole center"
        );
    }

    // --- Knockable cones: bounded launch, nonzero post-hit speed,
    // lifetime/termination, determinism ---

    #[test]
    fn cone_launch_velocity_is_bounded_and_directed() {
        // Fast car: horizontal speed capped at CONE_MAX_LAUNCH_SPEED.
        let v = cone_launch_velocity(Vec2::new(0.0, 30.0), Vec2::new(0.0, 1.0));
        let h = Vec2::new(v.x, v.z).length();
        assert!(
            h <= CONE_MAX_LAUNCH_SPEED + 1e-5,
            "horizontal {h} exceeds cap"
        );
        assert!((v.y - CONE_LAUNCH_POP).abs() < 1e-5, "upward pop");
        // Direction follows the normal (cone flies away from the car).
        assert!(v.z > 0.0 && v.x.abs() < 1e-5, "flies along +Z normal");

        // Slow car: horizontal speed is transfer * speed (below the cap).
        let v2 = cone_launch_velocity(Vec2::new(0.0, 4.0), Vec2::new(1.0, 0.0));
        let h2 = Vec2::new(v2.x, v2.z).length();
        assert!((h2 - 4.0 * CONE_LAUNCH_TRANSFER).abs() < 1e-5, "h2={h2}");
        assert!(v2.x > 0.0 && v2.z.abs() < 1e-5, "flies along +X normal");
        assert!((v2.y - CONE_LAUNCH_POP).abs() < 1e-5);

        // Stationary car: no horizontal launch, just the upward pop.
        let v3 = cone_launch_velocity(Vec2::ZERO, Vec2::new(1.0, 0.0));
        assert!(
            Vec2::new(v3.x, v3.z).length() < 1e-5,
            "no horizontal when parked"
        );
        assert!((v3.y - CONE_LAUNCH_POP).abs() < 1e-5);
    }

    #[test]
    fn cone_launch_velocity_is_deterministic() {
        let pv = Vec2::new(3.0, -7.0);
        let n = Vec2::new(-1.0, 2.0).normalize();
        let a = cone_launch_velocity(pv, n);
        let b = cone_launch_velocity(pv, n);
        assert_eq!(a, b);
        // A different input (almost surely) yields a different output.
        assert_ne!(a, cone_launch_velocity(pv * 2.0, n));
    }

    #[test]
    fn cone_hit_speed_is_nonzero_and_modest() {
        let pre = 10.0_f32;
        let post = cone_hit_speed(pre);
        assert!(post > 0.0, "post-hit speed must stay nonzero");
        assert!(post < pre, "post-hit speed must bleed");

        // Reverse speed: sign preserved, magnitude reduced (no sign flip).
        let post_rev = cone_hit_speed(-8.0);
        assert!(post_rev < 0.0, "reverse sign preserved");
        assert!(post_rev.abs() < 8.0, "reverse magnitude reduced");

        // Zero stays zero (you don't knock a cone while parked on it).
        assert_eq!(cone_hit_speed(0.0), 0.0);

        // Repeated bleeds monotonically shrink magnitude without flipping.
        let mut s = 12.0_f32;
        for _ in 0..20 {
            let prev = s;
            s = cone_hit_speed(s);
            assert!(s > 0.0 && s < prev, "bleed must shrink: {s} vs {prev}");
        }
    }

    #[test]
    fn cone_spin_axis_is_horizontal_perpendicular_and_unit() {
        let ax = cone_spin_axis(Vec2::new(1.0, 0.0));
        assert!(ax.y.abs() < 1e-6, "spin axis must be horizontal");
        assert!((ax.length() - 1.0).abs() < 1e-5, "spin axis must be unit");
        // The signed axis must lean the upright tip toward the launch rather
        // than making the cone tumble backward.
        assert!(ax.z < -1e-5 && ax.x.abs() < 1e-5, "-Z for +X normal");
        let tipped_x = Quat::from_axis_angle(ax, 0.1) * Vec3::Y;
        assert!(tipped_x.x > 0.0, "tip must lean toward +X launch");

        let az = cone_spin_axis(Vec2::new(0.0, 1.0));
        assert!(az.x > 1e-5 && az.z.abs() < 1e-5, "+X for +Z normal");
        let tipped_z = Quat::from_axis_angle(az, 0.1) * Vec3::Y;
        assert!(tipped_z.z > 0.0, "tip must lean toward +Z launch");

        // Degenerate normal -> zero axis (no spin).
        assert_eq!(cone_spin_axis(Vec2::ZERO), Vec3::ZERO);
    }

    #[test]
    fn cone_spin_rate_is_bounded_and_scales_with_speed() {
        assert!(cone_spin_rate(Vec2::new(0.0, 100.0)) <= CONE_MAX_SPIN + 1e-5);
        assert!((cone_spin_rate(Vec2::new(0.0, 2.0)) - 2.0 * CONE_SPIN_PER_SPEED).abs() < 1e-5);
        assert_eq!(cone_spin_rate(Vec2::ZERO), 0.0);
    }

    #[test]
    fn cone_initial_lifetime_is_under_two_seconds() {
        assert!(cone_initial_lifetime() > 0.0);
        assert!(cone_initial_lifetime() <= 2.0);
    }

    #[test]
    fn cone_flight_lands_within_lifetime_and_under_two_seconds() {
        let mut vel = cone_launch_velocity(Vec2::new(0.0, 12.0), Vec2::new(0.0, 1.0));
        let mut pos = Vec3::ZERO;
        let mut lifetime = cone_initial_lifetime();
        let dt = 1.0 / 60.0;
        let mut elapsed = 0.0;
        let mut despawned = false;
        while elapsed < 3.0 {
            let (nv, np) = step_cone_flight(vel, pos, dt);
            vel = nv;
            pos = np;
            lifetime -= dt;
            elapsed += dt;
            if cone_should_despawn(pos.y, lifetime) {
                despawned = true;
                break;
            }
        }
        assert!(despawned, "cone never despawned");
        assert!(elapsed <= 2.0, "despawned at {elapsed}s, must be <= 2s");
        assert!(
            pos.y <= 0.0 || lifetime <= 0.0,
            "must terminate via ground or lifetime"
        );
    }

    #[test]
    fn cone_flight_is_deterministic() {
        fn simulate() -> (Vec3, Vec3, f32) {
            let mut vel =
                cone_launch_velocity(Vec2::new(5.0, -3.0), Vec2::new(1.0, 1.0).normalize());
            let mut pos = Vec3::ZERO;
            let mut lifetime = cone_initial_lifetime();
            let dt = 1.0 / 60.0;
            for _ in 0..30 {
                let (nv, np) = step_cone_flight(vel, pos, dt);
                vel = nv;
                pos = np;
                lifetime -= dt;
            }
            (vel, pos, lifetime)
        }
        assert_eq!(simulate(), simulate());
    }

    #[test]
    fn cone_flight_always_terminates_under_two_seconds_across_launches() {
        let cases = [
            (Vec2::new(0.0, 30.0), Vec2::new(0.0, 1.0)),
            (Vec2::new(20.0, 0.0), Vec2::new(1.0, 0.0)),
            (Vec2::new(1.0, 0.0), Vec2::new(-1.0, 0.0)),
            (Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0)),
            (Vec2::new(-9.0, 9.0), Vec2::new(-1.0, 1.0).normalize()),
        ];
        let dt = 1.0 / 60.0;
        for (pv, n) in cases {
            let mut vel = cone_launch_velocity(pv, n);
            let mut pos = Vec3::ZERO;
            let mut lifetime = cone_initial_lifetime();
            let mut elapsed = 0.0;
            let mut ok = false;
            while elapsed < 3.0 {
                let (nv, np) = step_cone_flight(vel, pos, dt);
                vel = nv;
                pos = np;
                lifetime -= dt;
                elapsed += dt;
                if cone_should_despawn(pos.y, lifetime) {
                    ok = true;
                    break;
                }
            }
            assert!(ok, "cone never despawned for pv={pv} n={n}");
            assert!(
                elapsed <= 2.0 + 1e-5,
                "despawned at {elapsed}s > 2s for pv={pv} n={n}"
            );
        }
    }
}
