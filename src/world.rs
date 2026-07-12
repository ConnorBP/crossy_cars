//! Infinite-road world: a recycling pool of Z-axis chunks plus the per-chunk
//! environment (grass, road, sidewalks, lane dashes, buildings, trees, lamp
//! posts, coins). The car drives toward -Z forever; as it advances, the
//! trailing chunk is recycled to the front and re-populated with a fresh
//! deterministic seed, giving a seamless endless feel at constant entity count
//! (web-friendly).
//!
//! Solid obstacles (buildings / trees / lamp posts) carry a generic `Collider`
//! (axis-aligned box, half-extents) so `car.rs::physics_collisions` can push
//! the car out of any of them with one circle-vs-AABB loop. Curbs keep their
//! own `Curb` component for the hop-up behaviour.

use bevy::prelude::*;
use bevy::color::LinearRgba;

use crate::car::Car;
use crate::game::events::CoinCollected;
use crate::game::resources::{Score, TimeLeft};
use crate::game::state::GameState;
use crate::palette;
use crate::textures::TextureAssets;

/// Gate real-time shadows off on WebGL2 for performance.
const SHADOWS: bool = cfg!(not(target_arch = "wasm32"));

/// Tag for coin entities (environment now — spawned inside chunks, recycled
/// with them, collected on pickup and respawned when the chunk re-populates).
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
/// Tagged onto buildings, trees and lamp posts; `car.rs::physics_collisions`
/// iterates `&Collider` generically.
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

// ---------------------------------------------------------------------------
// Chunk system
// ---------------------------------------------------------------------------

/// Tunable chunk layout. `length` is the Z size of one chunk; `count` is the
/// pool size (kept alive and recycled). With the defaults (40 × 5) the world
/// covers 200u of Z at any time.
#[derive(Resource)]
pub struct ChunkConfig {
    pub length: f32,
    pub count: i32,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            length: 40.0,
            count: 5,
        }
    }
}

/// Identifies a chunk-root entity and its logical index. Root transform sits
/// at `z = -index * CHUNK_LENGTH` (car drives toward -Z). When recycled, the
/// root is moved forward by `count * length` and re-populated with a fresh
/// index-derived seed.
#[derive(Component)]
pub struct Chunk {
    pub index: i32,
}

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChunkConfig>()
            .add_systems(Startup, spawn_initial_chunks)
            // Coin spin + pickup still live here (coins are environment now).
            .add_systems(
                Update,
                (spin_coins, collect_coins)
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            )
            // Recycle trailing chunks to the front as the car advances.
            .add_systems(
                Update,
                recycle_chunks.run_if(in_state(GameState::Playing)),
            );
    }
}

/// Spawn the directional sun + the initial pool of `count` chunks covering
/// `z ∈ [0, -count*length)` (the car starts at the origin and drives toward
/// -Z). Run once at Startup.
fn spawn_initial_chunks(
    mut commands: Commands,
    cfg: Res<ChunkConfig>,
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

    let length = cfg.length;
    let count = cfg.count;
    for i in 0..count {
        let chunk_root = commands
            .spawn((
                Transform::from_xyz(0.0, 0.0, -i as f32 * length),
                Visibility::default(),
                Chunk { index: i },
            ))
            .id();
        populate_chunk(
            &mut commands,
            &mut meshes,
            &mut materials,
            &textures,
            chunk_root,
            i,
            seed_for(i),
        );
    }
}

/// Deterministic per-chunk seed (varies with index so each chunk differs, but
/// the same index always yields the same layout — stable across recycles).
fn seed_for(index: i32) -> u32 {
    (index as u32)
        .wrapping_mul(1664525)
        .wrapping_add(0x9e3779b9)
}

/// Build all of one chunk's contents as children of `chunk_root`: grass strip,
/// road segment (8 × length), two sidewalk `Curb`s, lane dashes, ~3 buildings
/// per side, ~3 trees per side, ~2 lamp posts per side, ~4 coins. Decorations
/// are kept at least a 3.0u margin inside the chunk's Z range so recycling
/// never pops a half-obstacle into the road (risk E12).
#[allow(clippy::too_many_arguments)]
pub fn populate_chunk(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    textures: &TextureAssets,
    chunk_root: Entity,
    index: i32,
    seed: u32,
) {
    let length = 40.0_f32; // matches ChunkConfig default; decorations are laid
                           // out relative to this.

    // Chunk-local Z spans [-length/2, +length/2] around the root (root sits at
    // the chunk center). Keep a margin so obstacles never straddle a boundary.
    let z_min = -length / 2.0 + 3.0;
    let z_max = length / 2.0 - 3.0;

    // Shared blob-shadow material (semi-transparent dark patch, reused by
    // trees, buildings & lamp posts).
    let shadow_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.0, 0.0, 0.0, 0.35),
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    let _ = index; // available for callers; layout uses the seed instead.

    commands.entity(chunk_root).with_children(|p| {
        // --- Grass strip (chunk-wide) ---
        p.spawn((
            Mesh3d(meshes.add(Plane3d::default().mesh().size(100.0, length))),
            MeshMaterial3d(textures.grass.clone()),
            Transform::from_xyz(0.0, 0.0, 0.0),
        ));

        // --- Road segment (8 × length) ---
        p.spawn((
            Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, length))),
            MeshMaterial3d(textures.road.clone()),
            Transform::from_xyz(0.0, 0.02, 0.0),
        ));

        // --- Sidewalk curbs (collidable as Curb for hop-up) ---
        let sidewalk_mesh = meshes.add(Cuboid::new(1.5, 0.18, length));
        for x in [4.75_f32, -4.75_f32] {
            p.spawn((
                Mesh3d(sidewalk_mesh.clone()),
                MeshMaterial3d(textures.sidewalk.clone()),
                Transform::from_xyz(x, 0.09, 0.0),
                Curb {
                    half_x: 0.75,
                    half_z: length / 2.0,
                    height: 0.18,
                },
            ));
        }

        // --- Lane dashes (step 4.0 across the chunk) ---
        let dash_mesh = meshes.add(Cuboid::new(0.18, 0.02, 2.0));
        let line_mat = materials.add(StandardMaterial {
            base_color: palette::LANE_WHITE,
            ..default()
        });
        let mut z = -length / 2.0 + 2.0;
        while z <= length / 2.0 - 2.0 {
            p.spawn((
                Mesh3d(dash_mesh.clone()),
                MeshMaterial3d(line_mat.clone()),
                Transform::from_xyz(0.0, 0.035, z),
            ));
            z += 4.0;
        }
        // Solid edge lines.
        let edge_mesh = meshes.add(Cuboid::new(0.12, 0.02, length));
        for x in [3.75_f32, -3.75_f32] {
            p.spawn((
                Mesh3d(edge_mesh.clone()),
                MeshMaterial3d(line_mat.clone()),
                Transform::from_xyz(x, 0.035, 0.0),
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
        let tree_shadow_mesh = meshes.add(Plane3d::default().mesh().size(1.8, 1.8));

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

        // --- Coins (~4 per chunk, on/near the road) ---
        let coin_mesh = meshes.add(Cylinder::new(0.3, 0.08));
        let coin_mat = materials.add(StandardMaterial {
            base_color: palette::COIN,
            metallic: 0.8,
            perceptual_roughness: 0.25,
            ..default()
        });

        // --- Deterministic per-chunk LCG for placement variety ---
        let mut s = seed;
        // ~4 coins spread along the chunk, mostly on the road.
        for _ in 0..4 {
            let cx = (rand(&mut s) * 2.0 - 1.0) * 3.0; // within road ±3
            let cz = z_min + rand(&mut s) * (z_max - z_min);
            p.spawn((
                Mesh3d(coin_mesh.clone()),
                MeshMaterial3d(coin_mat.clone()),
                Transform::from_xyz(cx, 0.5, cz),
                Coin,
            ));
        }

        // --- ~3 buildings per side ---
        for side in [-1.0_f32, 1.0] {
            for _ in 0..3 {
                let bx = side * (16.0 + rand(&mut s) * 6.0); // 16..22 from center
                let bz = z_min + rand(&mut s) * (z_max - z_min);
                let w = 3.5 + rand(&mut s) * 1.5; // 3.5..5.0
                let h = 4.0 + rand(&mut s) * 5.0; // 4.0..9.0
                let d = 3.5 + rand(&mut s) * 1.5;
                let ci = (rand(&mut s) * 3.0) as usize % 3;
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
                        Mesh3d(meshes.add(Plane3d::default().mesh().size(
                            w * 1.4,
                            d * 1.4,
                        ))),
                        MeshMaterial3d(shadow_mat.clone()),
                        Transform::from_xyz(0.0, 0.012, 0.0),
                    ));
                });
            }
        }

        // --- ~3 trees per side ---
        for side in [-1.0_f32, 1.0] {
            for _ in 0..3 {
                let tx = side * (8.0 + rand(&mut s) * 5.0); // 8..13 from center
                let tz = z_min + rand(&mut s) * (z_max - z_min);
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
                        Transform::from_xyz(0.0, 0.012, 0.0),
                    ));
                });
            }
        }

        // --- ~2 lamp posts per side ---
        for side in [-1.0_f32, 1.0] {
            for _ in 0..2 {
                let lx = side * 4.75;
                let lz = z_min + rand(&mut s) * (z_max - z_min);
                let dir = -side; // arm extends toward the road
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
                        Transform::from_xyz(dir * 0.4, 3.1, 0.0),
                    ));
                    lp.spawn((
                        Mesh3d(lamp_mesh.clone()),
                        MeshMaterial3d(lamp_mat.clone()),
                        Transform::from_xyz(dir * 0.8, 3.1, 0.0),
                    ));
                });
            }
        }
    });
}

/// Tiny LCG for deterministic-but-varied placement without pulling in `rand`.
fn rand(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
    (*seed as f32) / (u32::MAX as f32)
}

/// Recycle trailing chunks to the front as the car advances. When the car is
/// more than `CHUNK_LENGTH` ahead of the trailing chunk's leading edge,
/// despawn that chunk root (recursive — nukes all its children, safe in 0.19,
/// risk E2) and spawn a brand-new chunk root at the front (`z -= span`) with
/// a fresh index/seed. This keeps a constant pool of `count` chunks alive.
fn recycle_chunks(
    mut commands: Commands,
    cfg: Res<ChunkConfig>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    textures: Res<TextureAssets>,
    car: Query<&Transform, With<Car>>,
    chunks: Query<(Entity, &Chunk, &Transform)>,
) {
    let Ok(car_t) = car.single() else {
        return;
    };
    let length = cfg.length;
    let count = cfg.count;
    let span = count as f32 * length;

    // Find the trailing chunk: the one whose root-z is largest (closest to +Z,
    // furthest behind a car driving toward -Z).
    let mut trailing: Option<(Entity, i32, f32)> = None;
    for (e, chunk, tf) in &chunks {
        let root_z = tf.translation.z;
        match trailing {
            None => trailing = Some((e, chunk.index, root_z)),
            Some((_, _, best_z)) if root_z > best_z => {
                trailing = Some((e, chunk.index, root_z));
            }
            _ => {}
        }
    }
    let Some((chunk_e, old_index, root_z)) = trailing else {
        return;
    };

    // Leading edge of the trailing chunk = root_z + length/2 (root sits at
    // the chunk center). Recycle when the car is more than `length` past it.
    let leading_edge = root_z + length / 2.0;
    if car_t.translation.z > leading_edge - length {
        return;
    }

    // Despawn the old chunk root (recursively nukes its children, safe in 0.19)
    // and spawn a fresh root at the front with a progressed index + seed.
    commands.entity(chunk_e).despawn();
    let new_index = old_index + count;
    let new_z = root_z - span;
    let new_root = commands
        .spawn((
            Transform::from_xyz(0.0, 0.0, new_z),
            Visibility::default(),
            Chunk { index: new_index },
        ))
        .id();
    populate_chunk(
        &mut commands,
        &mut meshes,
        &mut materials,
        &textures,
        new_root,
        new_index,
        seed_for(new_index),
    );
}

// ---------------------------------------------------------------------------
// Coins (environment now — spawned in chunks, collected on pickup)
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
    mut coins: Query<(Entity, &Transform), (With<Coin>, Without<Car>)>,
    mut commands: Commands,
    mut score: ResMut<Score>,
    mut timeleft: ResMut<TimeLeft>,
    mut coin_events: MessageWriter<CoinCollected>,
) {
    let Ok(car_t) = car.single() else {
        return;
    };
    for (e, coin_t) in &mut coins {
        if car_t.translation.distance(coin_t.translation) < 1.2 {
            commands.entity(e).despawn();
            score.coins += 1;
            timeleft.0 += 3.0; // time bonus!
            coin_events.write(CoinCollected);
        }
    }
}
