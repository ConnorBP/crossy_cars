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
//! bigger blocks, T-intersections, corners and missing roads for variety.
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

use bevy::math::primitives::Circle;
use bevy::prelude::*;
use bevy::color::LinearRgba;

use crate::car::Car;
use crate::game::SpawnSet;
use crate::game::events::CoinCollected;
use crate::game::resources::{RoundActive, Score, TimeLeft};
use crate::game::state::GameState;
use crate::palette;
use crate::textures::TextureAssets;

/// Gate real-time shadows off on WebGL2 for performance.
const SHADOWS: bool = cfg!(not(target_arch = "wasm32"));

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
/// least one `TileKind` whose sockets match (see `all_tiles` / `pick_tile`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TileKind {
    /// All edges None — a full block of buildings.
    Empty,
    /// All edges None — a park (grass + trees, no buildings). Visual variant
    /// of `Empty` chosen for variety when no roads touch the block.
    Park,
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

/// All tiles in the set (used by `pick_tile` to find matches). Includes the
/// four single-edge `Stub*` tiles so the set is COMPLETE: every fixed-edge
/// combination (including a single fixed-Road edge with the rest free) has at
/// least one matching tile.
const ALL_TILES: [TileKind; 17] = [
    TileKind::Empty,
    TileKind::Park,
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

/// Weighting for the weighted-random tile choice. Through-roads and
/// intersections get high weights so the road network stays connected and
/// drivable; corners (dead-end-ish turns) get medium weight; Empty/Park get
/// weight 1 and are only realistically chosen when all four edges are free
/// or fixed to None (so a road never gets silently dropped where a neighbour
/// expected one).
fn tile_weight(kind: TileKind) -> u32 {
    match kind {
        TileKind::Cross => 10,
        TileKind::RoadNS | TileKind::RoadEW => 8,
        TileKind::TN | TileKind::TE | TileKind::TS | TileKind::TW => 5,
        TileKind::CornerWN | TileKind::CornerNE | TileKind::CornerES | TileKind::CornerSW => 3,
        // Stubs are dead-end spurs — low weight so they only appear when a
        // neighbour forces a single Road edge and the free edges roll None.
        TileKind::StubW | TileKind::StubE | TileKind::StubS | TileKind::StubN => 2,
        TileKind::Empty | TileKind::Park => 1,
    }
}

/// Pick a `TileKind` whose sockets match every `Some` entry in `fixed`
/// (`fixed = [W, E, S, N]`; `None` = free edge, `Some(e)` = fixed by an
/// already-placed neighbour's matching edge). Among matching tiles, choose
/// one by weighted random (bias toward through-roads/intersections) using
/// the provided LCG seed so the choice is deterministic per `(gx, gz, seed)`.
///
/// The tile set is COMPLETE: for any fixed-edge combination there is always
/// at least one matching tile (the `Cross` tile matches any all-`Road`
/// constraints; `Empty`/`Park` match any all-`None` constraints; and every
/// partial constraint is satisfied by at least one of the
/// Road/Corner/T/Cross tiles). So this never returns `None`.
fn pick_tile(fixed: [Option<Edge>; 4], seed: &mut u32) -> TileKind {
    // Collect all tiles whose sockets match every fixed entry.
    let candidates: Vec<TileKind> = ALL_TILES
        .iter()
        .copied()
        .filter(|&k| {
            let s = sockets(k);
            (fixed[W].is_none() || fixed[W] == Some(s[W]))
                && (fixed[E].is_none() || fixed[E] == Some(s[E]))
                && (fixed[S].is_none() || fixed[S] == Some(s[S]))
                && (fixed[N].is_none() || fixed[N] == Some(s[N]))
        })
        .collect();
    // Completeness guard: the tile set is complete (including the four
    // single-edge Stub tiles) so this is never empty. If a bug ever makes
    // it empty, fall back to Cross (all-Road) which matches any all-Road
    // constraint and otherwise at least keeps roads connected.
    if candidates.is_empty() {
        return TileKind::Cross;
    }

    let n_free = fixed.iter().filter(|e| e.is_none()).count();
    let n_fixed_road = fixed.iter().filter(|e| matches!(e, Some(Edge::Road))).count();
    let n_fixed_none = fixed.iter().filter(|e| matches!(e, Some(Edge::None))).count();
    debug_assert_eq!(n_free + n_fixed_road + n_fixed_none, 4);

    // Case 1: ALL four edges free (no neighbour constraints — e.g. a fresh
    // interior block on the very first spawn, or a recycled block with no
    // inward neighbours yet). Bias strongly toward roads so the network
    // stays connected; a small chance of Park for visual variety. Empty is
    // excluded (an all-None block would be a dead, roadless patch).
    if n_free == 4 {
        let r = rand(seed);
        if r < 0.10 {
            return TileKind::Park;
        }
        let road_candidates: Vec<TileKind> = candidates
            .iter()
            .copied()
            .filter(|&k| k != TileKind::Empty && k != TileKind::Park)
            .collect();
        return weighted_pick(&road_candidates, seed);
    }

    // Case 2: ALL four edges fixed to None (every neighbour says "no road on
    // our shared edge" and there are no free edges to extend a road into).
    // The block is enclosed by non-road neighbours -> Park vs Empty (both
    // all-None match). Bias toward Park for visual variety.
    if n_fixed_none == 4 {
        let r = rand(seed);
        if r < 0.6 {
            return TileKind::Park;
        }
        return TileKind::Empty;
    }

    // Case 3: at least one fixed-Road edge, or a mix of fixed-None and free
    // edges. If a road enters this block (a fixed-Road edge), REQUIRE the tile
    // to have >=2 Road edges so the road always continues or turns (never
    // dead-ends into an open field) — this excludes Stub tiles (single Road
    // edge = spur) when a road is present. Empty/Park are already ruled out by
    // a fixed-Road edge. If the >=2 filter empties (only when all non-fixed
    // edges are fixed-None, i.e. a road genuinely can't continue), fall back
    // to all matching candidates.
    let _ = (n_fixed_road, n_fixed_none);
    let pool: Vec<TileKind> = if n_fixed_road >= 1 {
        let continuing: Vec<TileKind> = candidates
            .iter()
            .copied()
            .filter(|&k| road_count(k) >= 2)
            .collect();
        if continuing.is_empty() {
            candidates
        } else {
            continuing
        }
    } else {
        candidates
    };
    weighted_pick(&pool, seed)
}

/// Number of `Road` edges on a tile.
fn road_count(kind: TileKind) -> usize {
    sockets(kind).iter().filter(|e| matches!(e, Edge::Road)).count()
}

/// Weighted random pick from `candidates` using `tile_weight` and the LCG
/// `seed`. Returns the first candidate if the total weight is somehow zero.
fn weighted_pick(candidates: &[TileKind], seed: &mut u32) -> TileKind {
    let total: u32 = candidates.iter().map(|&k| tile_weight(k)).sum();
    if total == 0 || candidates.is_empty() {
        return candidates[0];
    }
    let mut r = rand(seed) * (total as f32);
    for &k in candidates {
        let w = tile_weight(k) as f32;
        if r < w {
            return k;
        }
        r -= w;
    }
    candidates[candidates.len() - 1]
}

// ---------------------------------------------------------------------------
// 2D city-block grid system
// ---------------------------------------------------------------------------

/// Tunable grid layout. `block` is the size of one city block (and the
/// spacing of road grid lines); `count` is the grid window size (kept alive
/// and recycled in BOTH X and Z). With the defaults (40 × 5) the world
/// covers a 200u × 200u window around the car at any time.
#[derive(Resource)]
pub struct GridConfig {
    pub block: f32,
    pub count: i32,
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
/// sits at world `((gx+0.5)*block, 0, (gz+0.5)*block)`. When recycled along an
/// axis, the root is moved to the opposite side of the grid and re-populated
/// with a fresh (gx,gz)-derived seed. The `kind` is the Wang-tile kind
/// chosen for this block; neighbours read it to compute their fixed-edge
/// constraints (T19).
#[derive(Component)]
pub struct Block {
    pub gx: i32,
    pub gz: i32,
    pub kind: TileKind,
}

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GridConfig>()
            .add_systems(Startup, spawn_initial_grid)
            // Coin spin + pickup still live here (coins are environment now).
            .add_systems(
                Update,
                (spin_coins, collect_coins)
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            )
            // Re-center the grid on the car's spawn at the start of each
            // fresh round (skips on resume from Paused via RoundActive). Runs
            // in SpawnSet so it's before reset_run, which zeroes the car to
            // origin.
            .add_systems(
                OnEnter(GameState::Playing),
                reset_grid.in_set(SpawnSet),
            )
            // Recycle blocks that fall off any edge of the grid to the
            // opposite side, keeping a continuous count×count window around
            // the car in BOTH X and Z.
            .add_systems(
                Update,
                recycle_grid.run_if(in_state(GameState::Playing)),
            );
    }
}

/// Spawn the directional sun + the initial count×count grid of blocks
/// centered on the origin: gx,gz in `-count/2 .. count/2 - 1` (for count=5:
/// -2..=2). Run once at Startup. The sun is Startup-only and persists — it
/// is NOT re-spawned by `reset_grid`.
fn spawn_initial_grid(
    mut commands: Commands,
    cfg: Res<GridConfig>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    textures: Res<TextureAssets>,
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

    spawn_grid_window(&mut commands, &cfg, &mut meshes, &mut materials, &textures);
}

/// Spawn the count×count grid of blocks centered on the origin: gx,gz in
/// `-count/2 .. count/2 - 1`. Each block root at `((gx+0.5)*block, 0,
/// (gz+0.5)*block)` with `Block { gx, gz, kind }`, then `populate_block`.
/// Used by both `spawn_initial_grid` (Startup) and `reset_grid` (round
/// start).
///
/// Blocks are spawned in ascending (gx, gz) order so that the W (gx-1, gz)
/// and S (gx, gz-1) neighbours are already placed when a block is spawned;
/// their E / N sockets fix this block's W / S edges (E and N stay free).
/// This is the Wang-tile edge-continuity guarantee for the initial grid
/// (T19).
fn spawn_grid_window(
    commands: &mut Commands,
    cfg: &GridConfig,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    textures: &TextureAssets,
) {
    let block = cfg.block;
    let count = cfg.count;
    let lo = -count / 2;
    let hi = count / 2 - 1; // inclusive
    // Track the chosen kind of each placed block so later blocks can read
    // their W/S neighbours' E/N sockets. Keyed by (gx, gz).
    let mut placed: std::collections::HashMap<(i32, i32), TileKind> =
        std::collections::HashMap::new();
    for gx in lo..=hi {
        for gz in lo..=hi {
            // W neighbour (gx-1, gz): its E socket fixes our W edge.
            // S neighbour (gx, gz-1): its N socket fixes our S edge.
            let fixed_w = placed
                .get(&(gx - 1, gz))
                .map(|k| sockets(*k)[E]);
            let fixed_s = placed
                .get(&(gx, gz - 1))
                .map(|k| sockets(*k)[N]);
            let fixed = [fixed_w, None, fixed_s, None];
            // Deterministic per-block seed; the tile choice is folded into
            // the same seed so a given (gx,gz,seed) always picks the same
            // tile when free edges are randomized.
            let mut s = seed_for(gx, gz);
            let kind = pick_tile(fixed, &mut s);
            let root = commands
                .spawn((
                    Transform::from_xyz((gx as f32 + 0.5) * block, 0.0, (gz as f32 + 0.5) * block),
                    Visibility::default(),
                    Block { gx, gz, kind },
                ))
                .id();
            populate_block(
                commands,
                meshes,
                materials,
                textures,
                root,
                gx,
                gz,
                seed_for(gx, gz),
                kind,
            );
            placed.insert((gx, gz), kind);
        }
    }
}

/// Deterministic per-block seed (varies with (gx,gz) so each block differs,
/// but the same (gx,gz) always yields the same layout — stable across
/// recycles). The tile choice in `pick_tile` consumes a few LCG steps from
/// this same seed, so the whole block layout (tile + decorations) is a pure
/// function of (gx, gz).
fn seed_for(gx: i32, gz: i32) -> u32 {
    (gx as u32)
        .wrapping_mul(1664525)
        ^ (gz as u32)
            .wrapping_mul(22695477)
            .wrapping_add(0x9e3779b9)
}

/// Build all of one block's contents as children of `root`, per the chosen
/// Wang-tile `kind`: grass cell (always); a road segment on each `Road`
/// edge of the tile (W=−X, E=+X, S=−Z, N=+Z); curbs + lane dashes on each
/// road edge; buildings / trees / lamp posts / T12 obstacles in the interior
/// (overlap-rejected via `try_place`, shrunk away from each `Road` edge by a
/// 6u margin; `None` edges can use the full half-block); for `Park`: trees +
/// a park-green ground tint, no buildings; coins on the `Road` edges only.
///
/// `fixed` is NOT needed here (the `kind` is already chosen by the caller
/// via `pick_tile`); the caller passes the resolved `kind` directly. The
/// decorations are laid out relative to the 40u block size.
#[allow(clippy::too_many_arguments)]
pub fn populate_block(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    textures: &TextureAssets,
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

    // Block-local interior bounds: keep a 6.0u margin from any Road edge (so
    // obstacles never straddle a road), while None edges can use the full
    // half-block. The road is 8 wide (±4 from the edge line), so 6.0u keeps
    // obstacles just past the road's inner edge.
    let interior_max_x_lo = if road_w { -half + 6.0 } else { -half + 1.0 };
    let interior_max_x_hi = if road_e { half - 6.0 } else { half - 1.0 };
    let interior_max_z_lo = if road_s { -half + 6.0 } else { -half + 1.0 };
    let interior_max_z_hi = if road_n { half - 6.0 } else { half - 1.0 };

    // Shared blob-shadow material (semi-transparent dark patch, reused by
    // trees, buildings & lamp posts).
    let shadow_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.0, 0.0, 0.0, 0.35),
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    // Park-green ground tint (replaces the grass texture for Park tiles so
    // parks read as a distinct green space).
    let park_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.24, 0.52, 0.20),
        perceptual_roughness: 1.0,
        ..default()
    });

    let _ = (gx, gz); // available for callers; layout uses the seed instead.

    commands.entity(root).with_children(|p| {
        // --- Grass cell (block-wide, slightly oversized to avoid seams) ---
        // For Park tiles, use a flat park-green tint over the grass for a
        // distinct look; for non-park tiles, use the textured grass.
        if is_park {
            p.spawn((
                Mesh3d(meshes.add(Plane3d::default().mesh().size(block + 2.0, block + 2.0))),
                MeshMaterial3d(park_mat.clone()),
                Transform::from_xyz(0.0, 0.01, 0.0),
            ));
        } else {
            p.spawn((
                Mesh3d(meshes.add(Plane3d::default().mesh().size(block + 2.0, block + 2.0))),
                MeshMaterial3d(textures.grass.clone()),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ));
        }

        // --- Road segments on each Road edge ---
        // W (−X) edge road: runs along Z at local x = −half.
        if road_w {
            p.spawn((
                Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, block))),
                MeshMaterial3d(textures.road.clone()),
                Transform::from_xyz(-half, 0.02, 0.0),
            ));
        }
        // E (+X) edge road: runs along Z at local x = +half.
        if road_e {
            p.spawn((
                Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, block))),
                MeshMaterial3d(textures.road.clone()),
                Transform::from_xyz(half, 0.02, 0.0),
            ));
        }
        // S (−Z) edge road: runs along X at local z = −half.
        if road_s {
            p.spawn((
                Mesh3d(meshes.add(Plane3d::default().mesh().size(block, 8.0))),
                MeshMaterial3d(textures.road.clone()),
                Transform::from_xyz(0.0, 0.02, -half),
            ));
        }
        // N (+Z) edge road: runs along X at local z = +half.
        if road_n {
            p.spawn((
                Mesh3d(meshes.add(Plane3d::default().mesh().size(block, 8.0))),
                MeshMaterial3d(textures.road.clone()),
                Transform::from_xyz(0.0, 0.02, half),
            ));
        }

        // --- Curbs along the inner edges of each road (collidable, hop-up) ---
        // A road on edge E_dir spans the 8u around the edge line; its inner
        // curb sits 4.75u in from the edge line, on the block-interior side.
        // IMPORTANT: a curb does NOT span the full block — it stops at the
        // perpendicular road's inner edge (~4.75u in) so the sidewalk doesn't
        // cross the intersection, while still meeting the perpendicular curb at
        // the corner (no gap). 4.75 = the curb offset; 6.0 left a visible gap.
        const CURB_GAP: f32 = 4.75;
        if road_w {
            // W curb runs along Z at x = -half + 4.75; trim its z-ends where
            // the S/N roads cross.
            let z_lo = -half + if road_s { CURB_GAP } else { 0.0 };
            let z_hi = half - if road_n { CURB_GAP } else { 0.0 };
            if z_hi > z_lo {
                let len = z_hi - z_lo;
                let cz = (z_lo + z_hi) * 0.5;
                let curb_mesh = meshes.add(Cuboid::new(1.5, 0.18, len));
                p.spawn((
                    Mesh3d(curb_mesh.clone()),
                    MeshMaterial3d(textures.sidewalk.clone()),
                    Transform::from_xyz(-half + 4.75, 0.09, cz),
                    Curb {
                        half_x: 0.75,
                        half_z: len / 2.0,
                        height: 0.18,
                    },
                ));
            }
        }
        if road_e {
            let z_lo = -half + if road_s { CURB_GAP } else { 0.0 };
            let z_hi = half - if road_n { CURB_GAP } else { 0.0 };
            if z_hi > z_lo {
                let len = z_hi - z_lo;
                let cz = (z_lo + z_hi) * 0.5;
                let curb_mesh = meshes.add(Cuboid::new(1.5, 0.18, len));
                p.spawn((
                    Mesh3d(curb_mesh.clone()),
                    MeshMaterial3d(textures.sidewalk.clone()),
                    Transform::from_xyz(half - 4.75, 0.09, cz),
                    Curb {
                        half_x: 0.75,
                        half_z: len / 2.0,
                        height: 0.18,
                    },
                ));
            }
        }
        if road_s {
            // S curb runs along X at z = -half + 4.75; trim its x-ends where
            // the W/E roads cross.
            let x_lo = -half + if road_w { CURB_GAP } else { 0.0 };
            let x_hi = half - if road_e { CURB_GAP } else { 0.0 };
            if x_hi > x_lo {
                let len = x_hi - x_lo;
                let cx = (x_lo + x_hi) * 0.5;
                let curb_mesh = meshes.add(Cuboid::new(len, 0.18, 1.5));
                p.spawn((
                    Mesh3d(curb_mesh.clone()),
                    MeshMaterial3d(textures.sidewalk.clone()),
                    Transform::from_xyz(cx, 0.09, -half + 4.75),
                    Curb {
                        half_x: len / 2.0,
                        half_z: 0.75,
                        height: 0.18,
                    },
                ));
            }
        }
        if road_n {
            let x_lo = -half + if road_w { CURB_GAP } else { 0.0 };
            let x_hi = half - if road_e { CURB_GAP } else { 0.0 };
            if x_hi > x_lo {
                let len = x_hi - x_lo;
                let cx = (x_lo + x_hi) * 0.5;
                let curb_mesh = meshes.add(Cuboid::new(len, 0.18, 1.5));
                p.spawn((
                    Mesh3d(curb_mesh.clone()),
                    MeshMaterial3d(textures.sidewalk.clone()),
                    Transform::from_xyz(cx, 0.09, half - 4.75),
                    Curb {
                        half_x: len / 2.0,
                        half_z: 0.75,
                        height: 0.18,
                    },
                ));
            }
        }

        // --- Lane dashes + solid edge lines on each road edge ---
        let dash_mesh_z = meshes.add(Cuboid::new(0.18, 0.02, 2.0)); // along Z
        let dash_mesh_x = meshes.add(Cuboid::new(2.0, 0.02, 0.18)); // along X
        let line_mat = materials.add(StandardMaterial {
            base_color: palette::LANE_WHITE,
            ..default()
        });
        // Dashes + edge lines on the W road (centered on x = −half, running Z).
        if road_w {
            let z_lo = -half + if road_s { CURB_GAP } else { 0.0 };
            let z_hi = half - if road_n { CURB_GAP } else { 0.0 };
            let mut z = z_lo + 2.0;
            while z <= z_hi - 2.0 {
                p.spawn((
                    Mesh3d(dash_mesh_z.clone()),
                    MeshMaterial3d(line_mat.clone()),
                    Transform::from_xyz(-half, 0.035, z),
                ));
                z += 4.0;
            }
            // Edge lines trimmed to the same span as the curbs so they don't
            // overlap into the intersection.
            if z_hi > z_lo {
                let len = z_hi - z_lo;
                let cz = (z_lo + z_hi) * 0.5;
                let edge_mesh = meshes.add(Cuboid::new(0.12, 0.02, len));
                for &xo in &[3.75_f32, -3.75] {
                    p.spawn((
                        Mesh3d(edge_mesh.clone()),
                        MeshMaterial3d(line_mat.clone()),
                        Transform::from_xyz(-half + xo, 0.035, cz),
                    ));
                }
            }
        }
        // Dashes + edge lines on the E road (centered on x = +half, running Z).
        if road_e {
            let z_lo = -half + if road_s { CURB_GAP } else { 0.0 };
            let z_hi = half - if road_n { CURB_GAP } else { 0.0 };
            let mut z = z_lo + 2.0;
            while z <= z_hi - 2.0 {
                p.spawn((
                    Mesh3d(dash_mesh_z.clone()),
                    MeshMaterial3d(line_mat.clone()),
                    Transform::from_xyz(half, 0.035, z),
                ));
                z += 4.0;
            }
            if z_hi > z_lo {
                let len = z_hi - z_lo;
                let cz = (z_lo + z_hi) * 0.5;
                let edge_mesh = meshes.add(Cuboid::new(0.12, 0.02, len));
                for &xo in &[3.75_f32, -3.75] {
                    p.spawn((
                        Mesh3d(edge_mesh.clone()),
                        MeshMaterial3d(line_mat.clone()),
                        Transform::from_xyz(half + xo, 0.035, cz),
                    ));
                }
            }
        }
        // Dashes + edge lines on the S road (centered on z = −half, running X).
        if road_s {
            let x_lo = -half + if road_w { CURB_GAP } else { 0.0 };
            let x_hi = half - if road_e { CURB_GAP } else { 0.0 };
            let mut x = x_lo + 2.0;
            while x <= x_hi - 2.0 {
                p.spawn((
                    Mesh3d(dash_mesh_x.clone()),
                    MeshMaterial3d(line_mat.clone()),
                    Transform::from_xyz(x, 0.035, -half),
                ));
                x += 4.0;
            }
            if x_hi > x_lo {
                let len = x_hi - x_lo;
                let cx = (x_lo + x_hi) * 0.5;
                let edge_mesh = meshes.add(Cuboid::new(len, 0.02, 0.12));
                for &zo in &[3.75_f32, -3.75] {
                    p.spawn((
                        Mesh3d(edge_mesh.clone()),
                        MeshMaterial3d(line_mat.clone()),
                        Transform::from_xyz(cx, 0.035, -half + zo),
                    ));
                }
            }
        }
        // Dashes + edge lines on the N road (centered on z = +half, running X).
        if road_n {
            let x_lo = -half + if road_w { CURB_GAP } else { 0.0 };
            let x_hi = half - if road_e { CURB_GAP } else { 0.0 };
            let mut x = x_lo + 2.0;
            while x <= x_hi - 2.0 {
                p.spawn((
                    Mesh3d(dash_mesh_x.clone()),
                    MeshMaterial3d(line_mat.clone()),
                    Transform::from_xyz(x, 0.035, half),
                ));
                x += 4.0;
            }
            if x_hi > x_lo {
                let len = x_hi - x_lo;
                let cx = (x_lo + x_hi) * 0.5;
                let edge_mesh = meshes.add(Cuboid::new(len, 0.02, 0.12));
                for &zo in &[3.75_f32, -3.75] {
                    p.spawn((
                        Mesh3d(edge_mesh.clone()),
                        MeshMaterial3d(line_mat.clone()),
                        Transform::from_xyz(cx, 0.035, half + zo),
                    ));
                }
            }
        }

        // --- Shared obstacle assets ---
        let trunk_mesh = meshes.add(Cylinder::new(0.18, 0.9));
        let trunk_mat = materials.add(StandardMaterial {
            base_color: Color::srgb(0.34, 0.21, 0.11),
            perceptual_roughness: 0.9,
            ..default()
        });
        let foliage_mesh = meshes.add(Sphere::new(0.75).mesh().uv(12, 8));
        let foliage_mat = materials.add(StandardMaterial {
            base_color: Color::srgb(0.18, 0.42, 0.16),
            perceptual_roughness: 0.85,
            ..default()
        });
        let tree_shadow_mesh = meshes.add(Circle::new(0.9));

        let building_colors = [
            Color::srgb(0.92, 0.88, 0.78), // cream
            Color::srgb(0.45, 0.55, 0.68), // steel-blue
            Color::srgb(0.65, 0.35, 0.28), // brick
        ];
        let roof_colors = [
            Color::srgb(0.64, 0.62, 0.55),
            Color::srgb(0.32, 0.39, 0.48),
            Color::srgb(0.46, 0.25, 0.20),
        ];
        let body_mats: [Handle<StandardMaterial>; 3] = [
            materials.add(StandardMaterial {
                base_color: building_colors[0],
                perceptual_roughness: 0.8,
                ..default()
            }),
            materials.add(StandardMaterial {
                base_color: building_colors[1],
                perceptual_roughness: 0.8,
                ..default()
            }),
            materials.add(StandardMaterial {
                base_color: building_colors[2],
                perceptual_roughness: 0.8,
                ..default()
            }),
        ];
        let roof_mats: [Handle<StandardMaterial>; 3] = [
            materials.add(StandardMaterial {
                base_color: roof_colors[0],
                perceptual_roughness: 0.85,
                ..default()
            }),
            materials.add(StandardMaterial {
                base_color: roof_colors[1],
                perceptual_roughness: 0.85,
                ..default()
            }),
            materials.add(StandardMaterial {
                base_color: roof_colors[2],
                perceptual_roughness: 0.85,
                ..default()
            }),
        ];

        let pole_mesh = meshes.add(Cylinder::new(0.07, 3.2));
        let arm_mesh = meshes.add(Cuboid::new(0.8, 0.06, 0.06));
        let metal_mat = materials.add(StandardMaterial {
            base_color: Color::srgb(0.15, 0.15, 0.16),
            metallic: 0.8,
            perceptual_roughness: 0.4,
            ..default()
        });
        let lamp_mesh = meshes.add(Sphere::new(0.14).mesh().uv(8, 6));
        let lamp_mat = materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.85, 0.4),
            emissive: LinearRgba::new(1.5, 1.2, 0.5, 1.0),
            ..default()
        });

        // --- Coins (mesh + mat) ---
        let coin_mesh = meshes.add(Cylinder::new(0.3, 0.08));
        let coin_mat = materials.add(StandardMaterial {
            base_color: palette::COIN,
            metallic: 0.8,
            perceptual_roughness: 0.25,
            // Emissive gold glow so coins pop with bloom (T9 rendering beef-up).
            emissive: LinearRgba::rgb(0.9, 0.55, 0.05),
            ..default()
        });

        // --- T12 obstacle variety: cones, hydrants, benches, hedges ---
        // Shared assets for the four obstacle types (built from primitives,
        // each carries a generic `Collider` so `physics_collisions` handles them
        // automatically). NB: the Bevy `Cone` primitive is fully-qualified
        // here because this module also declares a `Cone` tag component (T12)
        // of the same name.
        let cone_body_mesh = meshes.add(bevy::math::primitives::Cone::new(0.18, 0.4));
        let cone_base_mesh = meshes.add(Cuboid::new(0.4, 0.04, 0.4));
        let cone_mat = materials.add(StandardMaterial {
            base_color: Color::srgb(0.95, 0.45, 0.05),
            perceptual_roughness: 0.7,
            // Slight emissive so cones pop under bloom (T9).
            emissive: LinearRgba::rgb(0.25, 0.08, 0.0),
            ..default()
        });
        let cone_shadow_mesh = meshes.add(Circle::new(0.3));

        let hydrant_body_mesh = meshes.add(Cylinder::new(0.12, 0.3));
        let hydrant_dome_mesh = meshes.add(Sphere::new(0.1).mesh().uv(10, 6));
        let hydrant_nub_mesh = meshes.add(Cylinder::new(0.05, 0.12));
        let hydrant_mat = materials.add(StandardMaterial {
            base_color: Color::srgb(0.85, 0.12, 0.1),
            perceptual_roughness: 0.6,
            emissive: LinearRgba::rgb(0.18, 0.02, 0.0),
            ..default()
        });
        let hydrant_shadow_mesh = meshes.add(Circle::new(0.35));

        let bench_seat_mesh = meshes.add(Cuboid::new(0.9, 0.1, 0.3));
        let bench_leg_mesh = meshes.add(Cuboid::new(0.08, 0.45, 0.28));
        let bench_back_mesh = meshes.add(Cuboid::new(0.9, 0.3, 0.06));
        let bench_mat = materials.add(StandardMaterial {
            base_color: Color::srgb(0.45, 0.28, 0.14),
            perceptual_roughness: 0.9,
            ..default()
        });
        let bench_shadow_mesh = meshes.add(Plane3d::default().mesh().size(1.1, 0.45));

        let hedge_box_mesh = meshes.add(Cuboid::new(1.2, 0.5, 0.4));
        let hedge_mat = materials.add(StandardMaterial {
            base_color: Color::srgb(0.16, 0.34, 0.14),
            perceptual_roughness: 0.9,
            ..default()
        });
        let hedge_shadow_mesh = meshes.add(Plane3d::default().mesh().size(1.4, 0.55));

        // --- Deterministic per-block LCG for placement variety ---
        let mut s = seed;
        // Overlap-rejection footprint list (simple-room-placement): every
        // building/tree/lamp/obstacle we place pushes its AABB here so later
        // placements skip spots that overlap it (with a margin). Prevents the
        // overlapping buildings/obstacles the user reported.
        let mut placed: Vec<[f32; 4]> = Vec::new();

        // --- Coins on the Road edges only ---
        // Collect the road edges so we can pick one at random per coin. Each
        // road edge gives a strip the coin sits on (within ±3 of the edge
        // line, spanning the block along the road's direction).
        let road_edges: [bool; 4] = [road_w, road_e, road_s, road_n];
        let n_coins = if any_road { 4 } else { 0 };
        for _ in 0..n_coins {
            // Pick a road edge. (rand < 0.5 picks a Z-running edge if any,
            // else an X-running edge; fallback to whichever exists.)
            let pick_z = rand(&mut s) < 0.5; // W or E (road runs along Z)
            let pick_x = !pick_z; // S or N (road runs along X)
            if pick_z && (road_w || road_e) {
                // Z-running road: x near the edge line, z across the block.
                let edge_x = if road_w && road_e {
                    if rand(&mut s) < 0.5 { -half } else { half }
                } else if road_w {
                    -half
                } else {
                    half
                };
                let cx = edge_x + (rand(&mut s) * 2.0 - 1.0) * 3.0;
                let cz = -half + 2.0 + rand(&mut s) * (block - 4.0);
                p.spawn((
                    Mesh3d(coin_mesh.clone()),
                    MeshMaterial3d(coin_mat.clone()),
                    Transform::from_xyz(cx, 0.5, cz),
                    Coin,
                ));
            } else if pick_x && (road_s || road_n) {
                // X-running road: z near the edge line, x across the block.
                let edge_z = if road_s && road_n {
                    if rand(&mut s) < 0.5 { -half } else { half }
                } else if road_s {
                    -half
                } else {
                    half
                };
                let cx = -half + 2.0 + rand(&mut s) * (block - 4.0);
                let cz = edge_z + (rand(&mut s) * 2.0 - 1.0) * 3.0;
                p.spawn((
                    Mesh3d(coin_mesh.clone()),
                    MeshMaterial3d(coin_mat.clone()),
                    Transform::from_xyz(cx, 0.5, cz),
                    Coin,
                ));
            } else {
                // Fallback: whichever road edge exists (handles odd combos
                // like a single Corner edge being the only one available on
                // the picked axis).
                if road_w {
                    let cx = -half + (rand(&mut s) * 2.0 - 1.0) * 3.0;
                    let cz = -half + 2.0 + rand(&mut s) * (block - 4.0);
                    p.spawn((
                        Mesh3d(coin_mesh.clone()),
                        MeshMaterial3d(coin_mat.clone()),
                        Transform::from_xyz(cx, 0.5, cz),
                        Coin,
                    ));
                } else if road_e {
                    let cx = half + (rand(&mut s) * 2.0 - 1.0) * 3.0;
                    let cz = -half + 2.0 + rand(&mut s) * (block - 4.0);
                    p.spawn((
                        Mesh3d(coin_mesh.clone()),
                        MeshMaterial3d(coin_mat.clone()),
                        Transform::from_xyz(cx, 0.5, cz),
                        Coin,
                    ));
                } else if road_s {
                    let cx = -half + 2.0 + rand(&mut s) * (block - 4.0);
                    let cz = -half + (rand(&mut s) * 2.0 - 1.0) * 3.0;
                    p.spawn((
                        Mesh3d(coin_mesh.clone()),
                        MeshMaterial3d(coin_mat.clone()),
                        Transform::from_xyz(cx, 0.5, cz),
                        Coin,
                    ));
                } else if road_n {
                    let cx = -half + 2.0 + rand(&mut s) * (block - 4.0);
                    let cz = half + (rand(&mut s) * 2.0 - 1.0) * 3.0;
                    p.spawn((
                        Mesh3d(coin_mesh.clone()),
                        MeshMaterial3d(coin_mat.clone()),
                        Transform::from_xyz(cx, 0.5, cz),
                        Coin,
                    ));
                }
            }
        }
        let _ = road_edges;

        // --- Interior decorations ---
        // For Park tiles: trees + park-green tint (already applied above), no
        // buildings. For Empty/non-park tiles: buildings + trees + lamps +
        // T12 obstacles. The interior bounds are shrunk away from each Road
        // edge (6u margin); None edges use the full half-block.
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
                // Arm extends toward the nearest Road edge (so the light
                // hangs over the road). If no road, default to -X.
                let mut best_dir = (-1.0_f32, 0.0_f32);
                let mut best_dist = (-half - lx).abs(); // distance to W road
                let d_e = (half - lx).abs();
                if road_e && d_e < best_dist {
                    best_dist = d_e;
                    best_dir = (1.0, 0.0);
                }
                let d_s = (-half - lz).abs();
                if road_s && d_s < best_dist {
                    best_dist = d_s;
                    best_dir = (0.0, -1.0);
                }
                let d_n = (half - lz).abs();
                if road_n && d_n < best_dist {
                    best_dist = d_n;
                    best_dir = (0.0, 1.0);
                }
                if !any_road {
                    // No road at all -> arm toward -X (default).
                    best_dir = (-1.0, 0.0);
                }
                let (dir_x, dir_z) = best_dir;
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
                        Transform::from_xyz(0.0, 1.6, 0.0),
                    ));
                    lp.spawn((
                        Mesh3d(arm_mesh.clone()),
                        MeshMaterial3d(metal_mat.clone()),
                        // Orient the arm along the chosen axis.
                        Transform::from_xyz(dir_x * 0.4, 3.1, dir_z * 0.4)
                            .with_rotation(Quat::from_rotation_y(if dir_x != 0.0 {
                                std::f32::consts::FRAC_PI_2
                            } else {
                                0.0
                            })),
                    ));
                    lp.spawn((
                        Mesh3d(lamp_mesh.clone()),
                        MeshMaterial3d(lamp_mat.clone()),
                        Transform::from_xyz(dir_x * 0.8, 3.1, dir_z * 0.8),
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
                                Transform::from_xyz(0.15, 0.18, 0.0)
                                    .with_rotation(Quat::from_rotation_z(
                                        std::f32::consts::FRAC_PI_2,
                                    )),
                            ));
                            hp.spawn((
                                Mesh3d(hydrant_nub_mesh.clone()),
                                MeshMaterial3d(hydrant_mat.clone()),
                                Transform::from_xyz(-0.15, 0.18, 0.0)
                                    .with_rotation(Quat::from_rotation_z(
                                        std::f32::consts::FRAC_PI_2,
                                    )),
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

/// 4-directional recycling: for EACH axis (X and Z) independently, find the
/// min/max block coordinate; when the car is past the grid edge by
/// `VIEW_MARGIN`, recycle the far column/row — despawn those block roots
/// (recursive — nukes all their children, safe in 0.19, risk E2) and spawn
/// fresh ones on the opposite side with progressed (gx,gz) + fresh seed.
/// Keeps a continuous count×count window around the car in BOTH X and Z ->
/// no gaps, car can drive endlessly in any direction. At most one
/// column/row recycles per axis per frame.
///
/// T19: when spawning a recycled block, its INWARD neighbours (the existing
/// grid on the side it's joining) fix the shared edges; outward edges are
/// free. The existing neighbours' `Block.kind` is read to get the matching
/// socket. This preserves Wang-tile edge-continuity across recycles.
fn recycle_grid(
    mut commands: Commands,
    cfg: Res<GridConfig>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    textures: Res<TextureAssets>,
    car: Query<&Transform, (With<Car>, Without<Block>)>,
    blocks: Query<(Entity, &Block, &Transform)>,
) {
    let Ok(car_t) = car.single() else {
        return;
    };
    let block = cfg.block;
    // Don't recycle a block until it's fully off-screen beyond the grid edge.
    // The ortho viewport is ~12u; add look-ahead + padding so blocks never
    // vanish while still visible.
    const VIEW_MARGIN: f32 = 16.0;
    let car_x = car_t.translation.x;
    let car_z = car_t.translation.z;

    // Snapshot the blocks so we can compute the despawn set + new (gx,gz)
    // BEFORE despawning (don't mutate the query mid-iteration). Includes the
    // tile kind so recycled blocks can read inward neighbours' sockets.
    let block_list: Vec<(Entity, i32, i32, TileKind, f32, f32)> = blocks
        .iter()
        .map(|(e, b, tf)| (e, b.gx, b.gz, b.kind, tf.translation.x, tf.translation.z))
        .collect();

    // Helper: look up a neighbour's tile kind by (gx, gz) in the snapshot.
    let kind_at = |gx: i32, gz: i32| -> Option<TileKind> {
        block_list
            .iter()
            .find(|(_, bgx, bgz, _, _, _)| *bgx == gx && *bgz == gz)
            .map(|(_, _, _, k, _, _)| *k)
    };

    // --- X axis: recycle a column if the car is past the +X or -X grid edge ---
    let (min_gx, max_gx) = block_list
        .iter()
        .map(|(_, gx, _, _, _, _)| *gx)
        .fold((i32::MAX, i32::MIN), |(mn, mx), gx| (mn.min(gx), mx.max(gx)));
    let x_edge_hi = (max_gx as f32 + 0.5) * block + VIEW_MARGIN; // past +X edge
    let x_edge_lo = (min_gx as f32 + 0.5) * block - VIEW_MARGIN; // past -X edge
    if car_x > x_edge_hi {
        // Recycle the min_gx column to gx = max_gx + 1.
        let to_despawn: Vec<Entity> = block_list
            .iter()
            .filter(|(_, gx, _, _, _, _)| *gx == min_gx)
            .map(|(e, _, _, _, _, _)| *e)
            .collect();
        for e in to_despawn {
            commands.entity(e).despawn();
        }
        let new_gx = max_gx + 1;
        // The new column joins the +X side of the grid: its W neighbour
        // (new_gx - 1 = max_gx) exists and fixes its W edge; E/N/S free (S
        // may exist if the row above/below was recycled this frame — but we
        // only recycle one column, so S is not freshly spawned; check the
        // snapshot for an existing S neighbour too, for safety).
        for (_, _, gz, _, _, _) in block_list
            .iter()
            .filter(|(_, gx, _, _, _, _)| *gx == min_gx)
        {
            let new_gz = *gz;
            let fixed_w = kind_at(new_gx - 1, new_gz).map(|k| sockets(k)[E]);
            let fixed_s = kind_at(new_gx, new_gz - 1).map(|k| sockets(k)[N]);
            let fixed = [fixed_w, None, fixed_s, None];
            let mut s = seed_for(new_gx, new_gz);
            let kind = pick_tile(fixed, &mut s);
            let root = commands
                .spawn((
                    Transform::from_xyz(
                        (new_gx as f32 + 0.5) * block,
                        0.0,
                        (new_gz as f32 + 0.5) * block,
                    ),
                    Visibility::default(),
                    Block {
                        gx: new_gx,
                        gz: new_gz,
                        kind,
                    },
                ))
                .id();
            populate_block(
                &mut commands,
                &mut meshes,
                &mut materials,
                &textures,
                root,
                new_gx,
                new_gz,
                seed_for(new_gx, new_gz),
                kind,
            );
        }
    } else if car_x < x_edge_lo {
        // Recycle the max_gx column to gx = min_gx - 1.
        let to_despawn: Vec<Entity> = block_list
            .iter()
            .filter(|(_, gx, _, _, _, _)| *gx == max_gx)
            .map(|(e, _, _, _, _, _)| *e)
            .collect();
        for e in to_despawn {
            commands.entity(e).despawn();
        }
        let new_gx = min_gx - 1;
        // The new column joins the -X side: its E neighbour (new_gx + 1 =
        // min_gx) exists and fixes its E edge; W/S free (S may exist — check
        // the snapshot).
        for (_, _, gz, _, _, _) in block_list
            .iter()
            .filter(|(_, gx, _, _, _, _)| *gx == max_gx)
        {
            let new_gz = *gz;
            let fixed_e = kind_at(new_gx + 1, new_gz).map(|k| sockets(k)[W]);
            let fixed_s = kind_at(new_gx, new_gz - 1).map(|k| sockets(k)[N]);
            let fixed = [None, fixed_e, fixed_s, None];
            let mut s = seed_for(new_gx, new_gz);
            let kind = pick_tile(fixed, &mut s);
            let root = commands
                .spawn((
                    Transform::from_xyz(
                        (new_gx as f32 + 0.5) * block,
                        0.0,
                        (new_gz as f32 + 0.5) * block,
                    ),
                    Visibility::default(),
                    Block {
                        gx: new_gx,
                        gz: new_gz,
                        kind,
                    },
                ))
                .id();
            populate_block(
                &mut commands,
                &mut meshes,
                &mut materials,
                &textures,
                root,
                new_gx,
                new_gz,
                seed_for(new_gx, new_gz),
                kind,
            );
        }
    }

    // --- Z axis: recycle a row if the car is past the +Z or -Z grid edge ---
    // Re-snapshot after the X recycle (new blocks were spawned with new gz
    // values identical to the despawned ones, so min/max gz are unchanged —
    // but the entity set changed). We re-query to be safe.
    let block_list_z: Vec<(Entity, i32, i32, TileKind)> = blocks
        .iter()
        .map(|(e, b, _)| (e, b.gx, b.gz, b.kind))
        .collect();
    let kind_at_z = |gx: i32, gz: i32| -> Option<TileKind> {
        block_list_z
            .iter()
            .find(|(_, bgx, bgz, _)| *bgx == gx && *bgz == gz)
            .map(|(_, _, _, k)| *k)
    };
    let (min_gz, max_gz) = block_list_z
        .iter()
        .map(|(_, _, gz, _)| *gz)
        .fold((i32::MAX, i32::MIN), |(mn, mx), gz| (mn.min(gz), mx.max(gz)));
    let z_edge_hi = (max_gz as f32 + 0.5) * block + VIEW_MARGIN; // past +Z edge
    let z_edge_lo = (min_gz as f32 + 0.5) * block - VIEW_MARGIN; // past -Z edge
    if car_z > z_edge_hi {
        // Recycle the min_gz row to gz = max_gz + 1.
        let to_despawn: Vec<Entity> = block_list_z
            .iter()
            .filter(|(_, _, gz, _)| *gz == min_gz)
            .map(|(e, _, _, _)| *e)
            .collect();
        for e in to_despawn {
            commands.entity(e).despawn();
        }
        let new_gz = max_gz + 1;
        // The new row joins the +Z side: its S neighbour (new_gz - 1 =
        // max_gz) exists and fixes its S edge; W/N free (W may exist — check
        // the snapshot).
        for (_, gx, _, _) in block_list_z.iter().filter(|(_, _, gz, _)| *gz == min_gz) {
            let new_gx = *gx;
            let fixed_s = kind_at_z(new_gx, new_gz - 1).map(|k| sockets(k)[N]);
            let fixed_w = kind_at_z(new_gx - 1, new_gz).map(|k| sockets(k)[E]);
            let fixed = [fixed_w, None, fixed_s, None];
            let mut s = seed_for(new_gx, new_gz);
            let kind = pick_tile(fixed, &mut s);
            let root = commands
                .spawn((
                    Transform::from_xyz(
                        (new_gx as f32 + 0.5) * block,
                        0.0,
                        (new_gz as f32 + 0.5) * block,
                    ),
                    Visibility::default(),
                    Block {
                        gx: new_gx,
                        gz: new_gz,
                        kind,
                    },
                ))
                .id();
            // The original entity was already despawned in the loop above;
            // spawn a fresh root and populate it.
            populate_block(
                &mut commands,
                &mut meshes,
                &mut materials,
                &textures,
                root,
                new_gx,
                new_gz,
                seed_for(new_gx, new_gz),
                kind,
            );
        }
    } else if car_z < z_edge_lo {
        // Recycle the max_gz row to gz = min_gz - 1.
        let to_despawn: Vec<Entity> = block_list_z
            .iter()
            .filter(|(_, _, gz, _)| *gz == max_gz)
            .map(|(e, _, _, _)| *e)
            .collect();
        for e in to_despawn {
            commands.entity(e).despawn();
        }
        let new_gz = min_gz - 1;
        // The new row joins the -Z side: its N neighbour (new_gz + 1 =
        // min_gz) exists and fixes its N edge; W/S free (W may exist — check
        // the snapshot).
        for (_, gx, _, _) in block_list_z.iter().filter(|(_, _, gz, _)| *gz == max_gz) {
            let new_gx = *gx;
            let fixed_n = kind_at_z(new_gx, new_gz + 1).map(|k| sockets(k)[S]);
            let fixed_w = kind_at_z(new_gx - 1, new_gz).map(|k| sockets(k)[E]);
            let fixed = [fixed_w, None, None, fixed_n];
            let mut s = seed_for(new_gx, new_gz);
            let kind = pick_tile(fixed, &mut s);
            let root = commands
                .spawn((
                    Transform::from_xyz(
                        (new_gx as f32 + 0.5) * block,
                        0.0,
                        (new_gz as f32 + 0.5) * block,
                    ),
                    Visibility::default(),
                    Block {
                        gx: new_gx,
                        gz: new_gz,
                        kind,
                    },
                ))
                .id();
            // The original entity was already despawned in the loop above;
            // spawn a fresh root and populate it.
            populate_block(
                &mut commands,
                &mut meshes,
                &mut materials,
                &textures,
                root,
                new_gx,
                new_gz,
                seed_for(new_gx, new_gz),
                kind,
            );
        }
    }
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
    mut materials: ResMut<Assets<StandardMaterial>>,
    textures: Res<TextureAssets>,
    blocks: Query<Entity, With<Block>>,
    round_active: Res<RoundActive>,
) {
    if round_active.0 {
        return;
    }
    for e in &blocks {
        commands.entity(e).despawn();
    }
    spawn_grid_window(&mut commands, &cfg, &mut meshes, &mut materials, &textures);
}

// ---------------------------------------------------------------------------
// Coins (environment now — spawned in blocks, collected on pickup)
// ---------------------------------------------------------------------------

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
    mut coin_events: MessageWriter<CoinCollected>,
) {
    let Ok(car_t) = car.single() else {
        return;
    };
    for (e, coin_t) in &mut coins {
        // Coins are block-root children -> `Transform` is local; use
        // `GlobalTransform` for the world position or pickup won't line up.
        if car_t.translation.distance(coin_t.translation()) < 1.2 {
            commands.entity(e).despawn();
            score.coins += 1;
            timeleft.0 += 3.0; // time bonus!
            coin_events.write(CoinCollected);
        }
    }
}
