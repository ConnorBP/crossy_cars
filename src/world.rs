use bevy::prelude::*;
use bevy::color::LinearRgba;

use crate::car::Car;
use crate::game::events::{ChickenHit, CoinCollected};
use crate::game::resources::{RoundActive, Score, TimeLeft};
use crate::game::state::GameState;
use crate::palette;
use crate::textures::TextureAssets;

/// Gate real-time shadows off on WebGL2 for performance.
const SHADOWS: bool = cfg!(not(target_arch = "wasm32"));

/// Tag for transient entities that should be despawned when leaving Playing.
#[derive(Component)]
pub struct Coin;

/// A wandering chicken. `dir` is the current horizontal heading;
/// `timer` counts down until the next random direction change.
#[derive(Component)]
pub struct Chicken {
    dir: Vec3,
    timer: f32,
}

/// A tiny feather puff spawned on a chicken hit; despawns after ~0.4s.
#[derive(Component)]
struct Feather {
    vel: Vec3,
    age: f32,
}

/// A raised curb the car can hop up onto (drives on top at `height`).
#[derive(Component)]
pub struct Curb {
    pub half_x: f32,
    pub half_z: f32,
    pub height: f32,
}

/// A solid obstacle (building) the car collides with and can't pass through.
#[derive(Component)]
pub struct Solid {
    pub half_x: f32,
    pub half_z: f32,
}

/// Arena clamp for wandering chickens (slightly inside the car arena).
const CHICKEN_ARENA: f32 = 48.0;

/// Cached mesh + material handles for chickens and feather poofs, created once
/// at startup and reused by `spawn_chickens`, `hit_chickens` (respawns + feathers).
#[derive(Resource)]
pub struct ChickenAssets {
    body_mesh: Handle<Mesh>,
    body_mat: Handle<StandardMaterial>,
    comb_mesh: Handle<Mesh>,
    comb_mat: Handle<StandardMaterial>,
    beak_mesh: Handle<Mesh>,
    beak_mat: Handle<StandardMaterial>,
    feather_mesh: Handle<Mesh>,
    feather_mat: Handle<StandardMaterial>,
}

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (spawn_environment, init_chicken_assets))
            .add_systems(
                OnEnter(GameState::Playing),
                (spawn_coins, spawn_chickens),
            )
            // Despawn the round's transient entities when the round truly ends
            // (GameOver / Menu) — NOT on Playing->Paused, so pause keeps them.
            .add_systems(
                OnEnter(GameState::GameOver),
                (cleanup_coins, cleanup_chickens, cleanup_feathers),
            )
            .add_systems(
                OnEnter(GameState::Menu),
                (cleanup_coins, cleanup_chickens, cleanup_feathers),
            )
            .add_systems(
                Update,
                (spin_coins, collect_coins)
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(
                Update,
                (wander_chickens, hit_chickens)
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(Update, feathers.run_if(in_state(GameState::Playing)));
    }
}

fn spawn_environment(
    mut commands: Commands,
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

    // Shared blob-shadow material (semi-transparent dark patch, reused by trees & buildings).
    let shadow_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.0, 0.0, 0.0, 0.35),
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    // --- Ground: single grass base ---
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(100.0, 100.0))),
        MeshMaterial3d(textures.grass.clone()),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));

    // --- Road: asphalt strip along Z ---
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, 100.0))),
        MeshMaterial3d(textures.road.clone()),
        Transform::from_xyz(0.0, 0.02, 0.0),
    ));

    // Sidewalks (raised concrete curbs flanking the road): textured + tagged
    // as `Curb` so the car can hop up onto them.
    let sidewalk_mesh = meshes.add(Cuboid::new(1.5, 0.18, 100.0));
    for x in [4.75_f32, -4.75_f32] {
        commands.spawn((
            Mesh3d(sidewalk_mesh.clone()),
            MeshMaterial3d(textures.sidewalk.clone()),
            Transform::from_xyz(x, 0.09, 0.0),
            Curb {
                half_x: 0.75,
                half_z: 50.0,
                height: 0.18,
            },
        ));
    }

    // Dashed center line (z = -48..48, step 4.0).
    let dash_mesh = meshes.add(Cuboid::new(0.18, 0.02, 2.0));
    let line_mat = materials.add(StandardMaterial {
        base_color: palette::LANE_WHITE,
        ..default()
    });
    for step in -12..=12 {
        let z = step as f32 * 4.0;
        commands.spawn((
            Mesh3d(dash_mesh.clone()),
            MeshMaterial3d(line_mat.clone()),
            Transform::from_xyz(0.0, 0.035, z),
        ));
    }

    // Solid edge lines.
    let edge_mesh = meshes.add(Cuboid::new(0.12, 0.02, 100.0));
    for x in [3.75_f32, -3.75_f32] {
        commands.spawn((
            Mesh3d(edge_mesh.clone()),
            MeshMaterial3d(line_mat.clone()),
            Transform::from_xyz(x, 0.035, 0.0),
        ));
    }

    // --- Trees (~16 on grass, flanking the road) ---
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

    let tree_positions: [(f32, f32); 16] = [
        (10.0, -48.0), (12.0, -36.0), (9.0, -24.0), (13.0, -12.0),
        (10.0, 12.0), (12.0, 24.0), (9.0, 36.0), (13.0, 48.0),
        (-10.0, -48.0), (-12.0, -36.0), (-9.0, -24.0), (-13.0, -12.0),
        (-10.0, 12.0), (-12.0, 24.0), (-9.0, 36.0), (-13.0, 48.0),
    ];
    for &(x, z) in tree_positions.iter() {
        commands
            .spawn((Transform::from_xyz(x, 0.0, z), Visibility::default()))
            .with_children(|p| {
                // Trunk
                p.spawn((
                    Mesh3d(trunk_mesh.clone()),
                    MeshMaterial3d(trunk_mat.clone()),
                    Transform::from_xyz(0.0, 0.45, 0.0),
                ));
                // Foliage
                p.spawn((
                    Mesh3d(foliage_mesh.clone()),
                    MeshMaterial3d(foliage_mat.clone()),
                    Transform::from_xyz(0.0, 1.35, 0.0),
                ));
                // Blob shadow (Plane3d is already horizontal: normal +Y)
                p.spawn((
                    Mesh3d(tree_shadow_mesh.clone()),
                    MeshMaterial3d(shadow_mat.clone()),
                    Transform::from_xyz(0.0, 0.012, 0.0),
                ));
            });
    }

    // --- Buildings (~10 cuboids outside the road) ---
    let building_colors = [
        Color::srgb(0.92, 0.88, 0.78), // cream
        Color::srgb(0.45, 0.55, 0.68), // steel-blue
        Color::srgb(0.65, 0.35, 0.28), // brick
    ];
    let roof_colors = [
        Color::srgb(0.64, 0.62, 0.55), // dark cream
        Color::srgb(0.32, 0.39, 0.48), // dark steel-blue
        Color::srgb(0.46, 0.25, 0.20), // dark brick
    ];
    let body_mats = [
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
    let roof_mats = [
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
    // (x, z, width, height, depth, color_index)
    let building_data: [(f32, f32, f32, f32, f32, usize); 10] = [
        (16.0, -35.0, 4.0, 6.0, 4.0, 0),
        (20.0, -18.0, 5.0, 8.0, 5.0, 1),
        (22.0, 2.0, 3.5, 5.0, 3.5, 2),
        (18.0, 18.0, 4.5, 7.0, 4.5, 0),
        (21.0, 36.0, 5.0, 9.0, 4.0, 1),
        (-16.0, -35.0, 4.0, 7.0, 4.0, 2),
        (-20.0, -18.0, 5.0, 4.0, 5.0, 0),
        (-22.0, 2.0, 3.5, 8.0, 3.5, 1),
        (-18.0, 18.0, 4.5, 6.0, 4.5, 2),
        (-21.0, 36.0, 5.0, 9.0, 4.0, 0),
    ];
    for &(x, z, w, h, d, ci) in building_data.iter() {
        commands
            .spawn((
                Transform::from_xyz(x, 0.0, z),
                Visibility::default(),
                Solid {
                    half_x: w / 2.0,
                    half_z: d / 2.0,
                },
            ))
            .with_children(|p| {
                // Body
                p.spawn((
                    Mesh3d(meshes.add(Cuboid::new(w, h, d))),
                    MeshMaterial3d(body_mats[ci].clone()),
                    Transform::from_xyz(0.0, h / 2.0, 0.0),
                ));
                // Roof (slightly larger and darker)
                p.spawn((
                    Mesh3d(meshes.add(Cuboid::new(w * 1.12, 0.4, d * 1.12))),
                    MeshMaterial3d(roof_mats[ci].clone()),
                    Transform::from_xyz(0.0, h + 0.2, 0.0),
                ));
                // Blob shadow (Plane3d is already horizontal: normal +Y)
                p.spawn((
                    Mesh3d(meshes.add(Plane3d::default().mesh().size(w * 1.4, d * 1.4))),
                    MeshMaterial3d(shadow_mat.clone()),
                    Transform::from_xyz(0.0, 0.012, 0.0),
                ));
            });
    }

    // --- Streetlights (~8 along sidewalks) ---
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
    // (x, z, arm_dir) — arm extends toward the road from each sidewalk.
    let streetlight_data: [(f32, f32, f32); 8] = [
        (4.75, -40.0, -1.0),
        (4.75, -20.0, -1.0),
        (4.75, 20.0, -1.0),
        (4.75, 40.0, -1.0),
        (-4.75, -40.0, 1.0),
        (-4.75, -20.0, 1.0),
        (-4.75, 20.0, 1.0),
        (-4.75, 40.0, 1.0),
    ];
    for &(x, z, dir) in streetlight_data.iter() {
        commands
            .spawn((Transform::from_xyz(x, 0.0, z), Visibility::default()))
            .with_children(|p| {
                // Pole
                p.spawn((
                    Mesh3d(pole_mesh.clone()),
                    MeshMaterial3d(metal_mat.clone()),
                    Transform::from_xyz(0.0, 1.6, 0.0),
                ));
                // Arm (extends toward road)
                p.spawn((
                    Mesh3d(arm_mesh.clone()),
                    MeshMaterial3d(metal_mat.clone()),
                    Transform::from_xyz(dir * 0.4, 3.1, 0.0),
                ));
                // Lamp head (emissive only — no PointLight, web-safe)
                p.spawn((
                    Mesh3d(lamp_mesh.clone()),
                    MeshMaterial3d(lamp_mat.clone()),
                    Transform::from_xyz(dir * 0.8, 3.1, 0.0),
                ));
            });
    }
}

pub fn spawn_coins(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    round_active: Res<RoundActive>,
) {
    // Resume from Paused keeps the existing coins — only spawn on a fresh round.
    if round_active.0 {
        return;
    }
    // ~22 coins: some on the road, some on the grass. Avoids the car start at origin.
    let positions: [(f32, f32); 22] = [
        // On the road
        (0.0, -8.0), (0.0, -18.0), (0.0, -28.0), (0.0, -38.0),
        (0.0, 8.0), (0.0, 18.0), (0.0, 28.0), (0.0, 38.0),
        (2.5, -23.0), (-2.5, 23.0), (1.5, -44.0), (-1.5, 44.0),
        // On the grass
        (6.0, -12.0), (-6.0, 12.0), (7.0, -32.0), (-7.0, 32.0),
        (5.5, 0.0), (-5.5, 0.0), (6.0, 42.0), (-6.0, -42.0),
        (7.5, 22.0), (-7.5, -22.0),
    ];

    let mesh = meshes.add(Cylinder::new(0.3, 0.08));
    let mat = materials.add(StandardMaterial {
        base_color: palette::COIN,
        metallic: 0.8,
        perceptual_roughness: 0.25,
        ..default()
    });

    for &(x, z) in positions.iter() {
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mat.clone()),
            Transform::from_xyz(x, 0.5, z),
            Coin,
        ));
    }
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

fn cleanup_coins(mut commands: Commands, coins: Query<Entity, With<Coin>>) {
    for e in &coins {
        commands.entity(e).despawn();
    }
}

fn init_chicken_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(ChickenAssets {
        body_mesh: meshes.add(Sphere::new(0.22).mesh().uv(10, 8)),
        body_mat: materials.add(StandardMaterial {
            base_color: Color::srgb(0.95, 0.95, 0.95),
            perceptual_roughness: 0.7,
            ..default()
        }),
        comb_mesh: meshes.add(Cuboid::new(0.12, 0.06, 0.12)),
        comb_mat: materials.add(StandardMaterial {
            base_color: Color::srgb(0.9, 0.1, 0.1),
            ..default()
        }),
        beak_mesh: meshes.add(Cuboid::new(0.06, 0.04, 0.08)),
        beak_mat: materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.6, 0.1),
            ..default()
        }),
        feather_mesh: meshes.add(Sphere::new(0.08).mesh().uv(6, 4)),
        feather_mat: materials.add(StandardMaterial {
            base_color: Color::srgb(0.95, 0.95, 0.95),
            ..default()
        }),
    });
}

pub fn spawn_chickens(
    mut commands: Commands,
    assets: Res<ChickenAssets>,
    round_active: Res<RoundActive>,
) {
    // Resume from Paused keeps the existing chickens — only spawn on a fresh round.
    if round_active.0 {
        return;
    }
    // ~14 chickens at scattered positions, avoiding origin / car start.
    let positions: [(f32, f32); 14] = [
        (8.0, -10.0), (-8.0, 10.0), (15.0, -25.0), (-15.0, 25.0),
        (20.0, 5.0), (-20.0, -5.0), (30.0, -40.0), (-30.0, 40.0),
        (40.0, 20.0), (-40.0, -20.0), (10.0, 35.0), (-10.0, -35.0),
        (25.0, 45.0), (-25.0, -45.0),
    ];

    let mut seed = 0x4d595df4u32;
    for &(x, z) in positions.iter() {
        let dir = rand_unit_xz(&mut seed);
        spawn_one_chicken(
            &mut commands,
            &assets,
            Vec3::new(x, 0.0, z),
            dir,
            1.0,
        );
    }
}

/// Tiny LCG for deterministic-but-varied randomness without pulling in `rand`.
fn rand(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
    (*seed as f32) / (u32::MAX as f32)
}

/// A random horizontal unit vector from the LCG.
fn rand_unit_xz(seed: &mut u32) -> Vec3 {
    let a = rand(seed) * std::f32::consts::TAU;
    Vec3::new(a.cos(), 0.0, a.sin())
}

fn wander_chickens(
    mut chickens: Query<(&mut Chicken, &mut Transform)>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    // Per-frame seed mixed from elapsed time so every run differs.
    let seed = (time.elapsed_secs() as u32).wrapping_add(0x9e3779b9);
    let mut i = 0u32;
    for (mut chicken, mut tf) in &mut chickens {
        chicken.timer -= dt;
        if chicken.timer <= 0.0 {
            // Fresh seed per chicken: mix frame seed with a counter.
            let mut local = seed.wrapping_add(i.wrapping_mul(0x85ebca6b));
            chicken.dir = rand_unit_xz(&mut local);
            // Reset timer to ~1.5..3.0s.
            chicken.timer = 1.5 + rand(&mut local) * 1.5;
        }
        tf.translation += chicken.dir * 2.5 * dt;
        tf.translation.x = tf.translation.x.clamp(-CHICKEN_ARENA, CHICKEN_ARENA);
        tf.translation.z = tf.translation.z.clamp(-CHICKEN_ARENA, CHICKEN_ARENA);
        // Face direction (match car forward = (-sin, -cos)).
        tf.rotation = Quat::from_rotation_y((-chicken.dir.x).atan2(-chicken.dir.z));
        i = i.wrapping_add(1);
    }
}

fn hit_chickens(
    car: Query<&Transform, (With<Car>, Without<Chicken>)>,
    mut chickens: Query<(Entity, &Transform), (With<Chicken>, Without<Car>)>,
    assets: Res<ChickenAssets>,
    mut commands: Commands,
    mut score: ResMut<Score>,
    mut hit_events: MessageWriter<ChickenHit>,
    time: Res<Time>,
) {
    let Ok(car_t) = car.single() else {
        return;
    };
    let mut seed = (time.elapsed_secs() as u32).wrapping_add(0x6a09e667);
    for (e, chicken_t) in &mut chickens {
        if car_t.translation.distance(chicken_t.translation) < 1.0 {
            // Feather poof at the chicken's position.
            for _ in 0..4 {
                let mut local = seed;
                let vx = (rand(&mut local) - 0.5) * 2.5;
                let vy = rand(&mut local) * 2.0 + 0.5;
                let vz = (rand(&mut local) - 0.5) * 2.5;
                seed = local;
                commands.spawn((
                    Mesh3d(assets.feather_mesh.clone()),
                    MeshMaterial3d(assets.feather_mat.clone()),
                    Transform::from_xyz(
                        chicken_t.translation.x,
                        0.3,
                        chicken_t.translation.z,
                    ),
                    Feather {
                        vel: Vec3::new(vx, vy, vz),
                        age: 0.0,
                    },
                ));
            }
            commands.entity(e).despawn();
            score.chickens += 1;
            hit_events.write(ChickenHit);
            // Respawn a chicken at a random position away from the car.
            let rx = (rand(&mut seed) * 2.0 - 1.0) * CHICKEN_ARENA;
            let rz = (rand(&mut seed) * 2.0 - 1.0) * CHICKEN_ARENA;
            let mut pos = Vec3::new(rx, 0.0, rz);
            // If the respawn landed too close to the car, nudge it away.
            if pos.distance(car_t.translation) < 6.0 {
                let away = (pos - car_t.translation).normalize_or_zero();
                pos = car_t.translation + away * 10.0;
            }
            pos.x = pos.x.clamp(-CHICKEN_ARENA, CHICKEN_ARENA);
            pos.z = pos.z.clamp(-CHICKEN_ARENA, CHICKEN_ARENA);
            spawn_one_chicken(&mut commands, &assets, pos, rand_unit_xz(&mut seed), 1.0);
        }
    }
}

/// Spawn a single chicken (parent + body/comb/beak children) at `pos`.
fn spawn_one_chicken(
    commands: &mut Commands,
    assets: &ChickenAssets,
    pos: Vec3,
    dir: Vec3,
    timer: f32,
) {
    commands
        .spawn((
            Transform::from_translation(pos),
            Visibility::default(),
            Chicken { dir, timer },
        ))
        .with_children(|p| {
            // White body
            p.spawn((
                Mesh3d(assets.body_mesh.clone()),
                MeshMaterial3d(assets.body_mat.clone()),
                Transform::from_xyz(0.0, 0.3, 0.0),
            ));
            // Red comb on top
            p.spawn((
                Mesh3d(assets.comb_mesh.clone()),
                MeshMaterial3d(assets.comb_mat.clone()),
                Transform::from_xyz(0.0, 0.5, 0.0),
            ));
            // Orange beak at front (-Z is forward)
            p.spawn((
                Mesh3d(assets.beak_mesh.clone()),
                MeshMaterial3d(assets.beak_mat.clone()),
                Transform::from_xyz(0.0, 0.32, -0.22),
            ));
        });
}

fn feathers(time: Res<Time>, mut commands: Commands, mut q: Query<(Entity, &mut Feather, &mut Transform)>) {
    let dt = time.delta_secs();
    for (e, mut feather, mut tf) in &mut q {
        feather.age += dt;
        if feather.age >= 0.4 {
            commands.entity(e).despawn();
            continue;
        }
        feather.vel.y -= 4.0 * dt; // gravity
        tf.translation += feather.vel * dt;
    }
}

fn cleanup_chickens(mut commands: Commands, chickens: Query<Entity, With<Chicken>>) {
    for e in &chickens {
        commands.entity(e).despawn();
    }
}

fn cleanup_feathers(mut commands: Commands, feathers: Query<Entity, With<Feather>>) {
    for e in &feathers {
        commands.entity(e).despawn();
    }
}
