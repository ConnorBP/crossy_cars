//! Infinite 2D city: a recycling pool of city-block grid cells plus the
//! per-block environment (grass, road segments along the -X and -Z edges,
//! curbs, lane dashes, buildings, trees, lamp posts, T12 obstacles, coins).
//! The car can drive side-to-side (X) AND forward/back (Z) endlessly in ALL
//! directions via 2D (4-directional) recycling: as the car crosses a grid
//! edge, the far column/row is recycled to the opposite side with a fresh
//! deterministic seed, giving a seamless endless feel at constant entity
//! count (web-friendly).
//!
//! Grid alignment: block (gx,gz) root sits at world `((gx+0.5)*block, 0,
//! (gz+0.5)*block)`. Roads run along the world lines `x = n*block` and
//! `z = n*block` (multiples of 40, INCLUDING x=0 and z=0). Each block draws
//! ONLY its -X and -Z edge roads, so adjacent blocks tile the full road grid
//! with no overlap; intersections emerge at the corners (world
//! (gx*block, gz*block)). The car spawn (0,0,0) sits at the intersection of
//! road x=0 and road z=0.
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
/// with a fresh (gx,gz)-derived seed.
#[derive(Component)]
pub struct Block {
    pub gx: i32,
    pub gz: i32,
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
/// (gz+0.5)*block)` with `Block { gx, gz }`, then `populate_block`. Used by
/// both `spawn_initial_grid` (Startup) and `reset_grid` (round start).
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
    for gx in lo..=hi {
        for gz in lo..=hi {
            let root = commands
                .spawn((
                    Transform::from_xyz((gx as f32 + 0.5) * block, 0.0, (gz as f32 + 0.5) * block),
                    Visibility::default(),
                    Block { gx, gz },
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
            );
        }
    }
}

/// Deterministic per-block seed (varies with (gx,gz) so each block differs,
/// but the same (gx,gz) always yields the same layout — stable across
/// recycles).
fn seed_for(gx: i32, gz: i32) -> u32 {
    (gx as u32)
        .wrapping_mul(1664525)
        ^ (gz as u32)
            .wrapping_mul(22695477)
            .wrapping_add(0x9e3779b9)
}

/// Build all of one block's contents as children of `root`: grass cell, road
/// segments on the -X and -Z edges (so adjacent blocks tile the full road
/// grid with no overlap), curbs along the roads, lane dashes, buildings /
/// trees / lamp posts / T12 obstacles in the block interior (overlap-
/// rejected), and a few coins on the roads. Decorations are kept in the
/// block interior (`|x| < block/2 - 6` AND `|z| < block/2 - 6`) so they never
/// straddle a road or block boundary.
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
) {
    let block = 40.0_f32; // matches GridConfig default; decorations are laid
                          // out relative to this.
    let half = block / 2.0;

    // Block-local interior bounds: keep a 6.0u margin from the edges so
    // obstacles never straddle a road (which runs along the -X and -Z edges)
    // or a block boundary. The road is 8 wide (±4 from the edge line), so
    // 6.0u keeps obstacles just past the road's inner edge.
    let interior_max = half - 6.0;

    // Shared blob-shadow material (semi-transparent dark patch, reused by
    // trees, buildings & lamp posts).
    let shadow_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.0, 0.0, 0.0, 0.35),
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    let _ = (gx, gz); // available for callers; layout uses the seed instead.

    commands.entity(root).with_children(|p| {
        // --- Grass cell (block-wide, slightly oversized to avoid seams) ---
        p.spawn((
            Mesh3d(meshes.add(Plane3d::default().mesh().size(block + 2.0, block + 2.0))),
            MeshMaterial3d(textures.grass.clone()),
            Transform::from_xyz(0.0, 0.0, 0.0),
        ));

        // --- Road on the -X edge (runs along Z, world road line x = gx*block) ---
        p.spawn((
            Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, block))),
            MeshMaterial3d(textures.road.clone()),
            Transform::from_xyz(-half, 0.02, 0.0),
        ));

        // --- Road on the -Z edge (runs along X, world road line z = gz*block) ---
        p.spawn((
            Mesh3d(meshes.add(Plane3d::default().mesh().size(block, 8.0))),
            MeshMaterial3d(textures.road.clone()),
            Transform::from_xyz(0.0, 0.02, -half),
        ));

        // --- Curbs along the inner edges of each road (collidable, hop-up) ---
        // -X road curb: along Z at local x = -half + 4.75 (inner edge of the
        //   8-wide road, which spans x in [-half-4, -half+4]).
        let curb_x_mesh = meshes.add(Cuboid::new(1.5, 0.18, block));
        p.spawn((
            Mesh3d(curb_x_mesh.clone()),
            MeshMaterial3d(textures.sidewalk.clone()),
            Transform::from_xyz(-half + 4.75, 0.09, 0.0),
            Curb {
                half_x: 0.75,
                half_z: half,
                height: 0.18,
            },
        ));
        // -Z road curb: along X at local z = -half + 4.75.
        let curb_z_mesh = meshes.add(Cuboid::new(block, 0.18, 1.5));
        p.spawn((
            Mesh3d(curb_z_mesh.clone()),
            MeshMaterial3d(textures.sidewalk.clone()),
            Transform::from_xyz(0.0, 0.09, -half + 4.75),
            Curb {
                half_x: half,
                half_z: 0.75,
                height: 0.18,
            },
        ));

        // --- Lane dashes on each road (oriented along the road's direction) ---
        let dash_mesh_z = meshes.add(Cuboid::new(0.18, 0.02, 2.0)); // along Z
        let dash_mesh_x = meshes.add(Cuboid::new(2.0, 0.02, 0.18)); // along X
        let line_mat = materials.add(StandardMaterial {
            base_color: palette::LANE_WHITE,
            ..default()
        });
        // Dashes along the -X road (centered on x = -half, running in Z).
        let mut z = -half + 2.0;
        while z <= half - 2.0 {
            p.spawn((
                Mesh3d(dash_mesh_z.clone()),
                MeshMaterial3d(line_mat.clone()),
                Transform::from_xyz(-half, 0.035, z),
            ));
            z += 4.0;
        }
        // Dashes along the -Z road (centered on z = -half, running in X).
        let mut x = -half + 2.0;
        while x <= half - 2.0 {
            p.spawn((
                Mesh3d(dash_mesh_x.clone()),
                MeshMaterial3d(line_mat.clone()),
                Transform::from_xyz(x, 0.035, -half),
            ));
            x += 4.0;
        }
        // Solid edge lines on the -X road.
        let edge_x_mesh = meshes.add(Cuboid::new(0.12, 0.02, block));
        for &xo in &[3.75_f32, -3.75] {
            p.spawn((
                Mesh3d(edge_x_mesh.clone()),
                MeshMaterial3d(line_mat.clone()),
                Transform::from_xyz(-half + xo, 0.035, 0.0),
            ));
        }
        // Solid edge lines on the -Z road.
        let edge_z_mesh = meshes.add(Cuboid::new(block, 0.02, 0.12));
        for &zo in &[3.75_f32, -3.75] {
            p.spawn((
                Mesh3d(edge_z_mesh.clone()),
                MeshMaterial3d(line_mat.clone()),
                Transform::from_xyz(0.0, 0.035, -half + zo),
            ));
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

        // --- ~4 coins on the roads (local x near -half OR z near -half) ---
        for _ in 0..4 {
            // Pick one of the two roads (the -X edge road or the -Z edge road).
            if rand(&mut s) < 0.5 {
                // -X road: x near -half (within ±3 of the edge line), z across
                // the block.
                let cx = -half + (rand(&mut s) * 2.0 - 1.0) * 3.0;
                let cz = -half + 2.0 + rand(&mut s) * (block - 4.0);
                p.spawn((
                    Mesh3d(coin_mesh.clone()),
                    MeshMaterial3d(coin_mat.clone()),
                    Transform::from_xyz(cx, 0.5, cz),
                    Coin,
                ));
            } else {
                // -Z road: z near -half (within ±3 of the edge line), x across
                // the block.
                let cx = -half + 2.0 + rand(&mut s) * (block - 4.0);
                let cz = -half + (rand(&mut s) * 2.0 - 1.0) * 3.0;
                p.spawn((
                    Mesh3d(coin_mesh.clone()),
                    MeshMaterial3d(coin_mat.clone()),
                    Transform::from_xyz(cx, 0.5, cz),
                    Coin,
                ));
            }
        }

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
                -interior_max,
                interior_max,
                -interior_max,
                interior_max,
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
                -interior_max,
                interior_max,
                -interior_max,
                interior_max,
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
                -interior_max,
                interior_max,
                -interior_max,
                interior_max,
                2.0,
                8,
            ) else {
                continue;
            };
            // Arm extends toward the nearest road edge (the -X or -Z road,
            // whichever is closer). Pick the closer of -half-x vs -half-z.
            let dist_x = (-half - lx).abs();
            let dist_z = (-half - lz).abs();
            let (dir_x, dir_z) = if dist_x < dist_z {
                (-1.0_f32, 0.0_f32) // arm toward -X road
            } else {
                (0.0_f32, -1.0_f32) // arm toward -Z road
            };
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
                -interior_max,
                interior_max,
                -interior_max,
                interior_max,
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
    // BEFORE despawning (don't mutate the query mid-iteration).
    let block_list: Vec<(Entity, i32, i32, f32, f32)> = blocks
        .iter()
        .map(|(e, b, tf)| (e, b.gx, b.gz, tf.translation.x, tf.translation.z))
        .collect();

    // --- X axis: recycle a column if the car is past the +X or -X grid edge ---
    let (min_gx, max_gx) = block_list
        .iter()
        .map(|(_, gx, _, _, _)| *gx)
        .fold((i32::MAX, i32::MIN), |(mn, mx), gx| (mn.min(gx), mx.max(gx)));
    let x_edge_hi = (max_gx as f32 + 0.5) * block + VIEW_MARGIN; // past +X edge
    let x_edge_lo = (min_gx as f32 + 0.5) * block - VIEW_MARGIN; // past -X edge
    if car_x > x_edge_hi {
        // Recycle the min_gx column to gx = max_gx + 1.
        let to_despawn: Vec<Entity> = block_list
            .iter()
            .filter(|(_, gx, _, _, _)| *gx == min_gx)
            .map(|(e, _, _, _, _)| *e)
            .collect();
        for e in to_despawn {
            commands.entity(e).despawn();
        }
        let new_gx = max_gx + 1;
        for (_, _, gz, _, _) in block_list
            .iter()
            .filter(|(_, gx, _, _, _)| *gx == min_gx)
        {
            let new_gz = *gz;
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
            );
        }
    } else if car_x < x_edge_lo {
        // Recycle the max_gx column to gx = min_gx - 1.
        let to_despawn: Vec<Entity> = block_list
            .iter()
            .filter(|(_, gx, _, _, _)| *gx == max_gx)
            .map(|(e, _, _, _, _)| *e)
            .collect();
        for e in to_despawn {
            commands.entity(e).despawn();
        }
        let new_gx = min_gx - 1;
        for (_, _, gz, _, _) in block_list
            .iter()
            .filter(|(_, gx, _, _, _)| *gx == max_gx)
        {
            let new_gz = *gz;
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
            );
        }
    }

    // --- Z axis: recycle a row if the car is past the +Z or -Z grid edge ---
    // Re-snapshot after the X recycle (new blocks were spawned with new gz
    // values identical to the despawned ones, so min/max gz are unchanged —
    // but the entity set changed). We re-query to be safe.
    let block_list_z: Vec<(Entity, i32, i32)> = blocks
        .iter()
        .map(|(e, b, _)| (e, b.gx, b.gz))
        .collect();
    let (min_gz, max_gz) = block_list_z
        .iter()
        .map(|(_, _, gz)| *gz)
        .fold((i32::MAX, i32::MIN), |(mn, mx), gz| (mn.min(gz), mx.max(gz)));
    let z_edge_hi = (max_gz as f32 + 0.5) * block + VIEW_MARGIN; // past +Z edge
    let z_edge_lo = (min_gz as f32 + 0.5) * block - VIEW_MARGIN; // past -Z edge
    if car_z > z_edge_hi {
        // Recycle the min_gz row to gz = max_gz + 1.
        let to_despawn: Vec<Entity> = block_list_z
            .iter()
            .filter(|(_, _, gz)| *gz == min_gz)
            .map(|(e, _, _)| *e)
            .collect();
        for e in to_despawn {
            commands.entity(e).despawn();
        }
        let new_gz = max_gz + 1;
        for (_, gx, _) in block_list_z.iter().filter(|(_, _, gz)| *gz == min_gz) {
            let new_gx = *gx;
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
            );
        }
    } else if car_z < z_edge_lo {
        // Recycle the max_gz row to gz = min_gz - 1.
        let to_despawn: Vec<Entity> = block_list_z
            .iter()
            .filter(|(_, _, gz)| *gz == max_gz)
            .map(|(e, _, _)| *e)
            .collect();
        for e in to_despawn {
            commands.entity(e).despawn();
        }
        let new_gz = min_gz - 1;
        for (_, gx, _) in block_list_z.iter().filter(|(_, _, gz)| *gz == max_gz) {
            let new_gx = *gx;
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
