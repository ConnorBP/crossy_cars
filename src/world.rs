use bevy::prelude::*;
use bevy::color::LinearRgba;

use crate::car::Car;
use crate::game::resources::Score;
use crate::game::state::GameState;
use crate::palette;

/// Gate real-time shadows off on WebGL2 for performance.
const SHADOWS: bool = cfg!(not(target_arch = "wasm32"));

/// Tag for transient entities that should be despawned when leaving Playing.
#[derive(Component)]
pub struct Coin;

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_environment)
            .add_systems(OnEnter(GameState::Playing), spawn_coins)
            .add_systems(OnExit(GameState::Playing), cleanup_coins)
            .add_systems(
                Update,
                (spin_coins, collect_coins)
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

fn spawn_environment(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
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
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: palette::GRASS_LIGHT,
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));

    // --- Road: asphalt strip along Z ---
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, 100.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: palette::ASPHALT,
            perceptual_roughness: 0.95,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.02, 0.0),
    ));

    // Sidewalks (raised concrete curbs flanking the road).
    let sidewalk_mesh = meshes.add(Cuboid::new(1.5, 0.18, 100.0));
    let sidewalk_mat = materials.add(StandardMaterial {
        base_color: palette::CONCRETE,
        perceptual_roughness: 0.9,
        ..default()
    });
    for x in [4.75_f32, -4.75_f32] {
        commands.spawn((
            Mesh3d(sidewalk_mesh.clone()),
            MeshMaterial3d(sidewalk_mat.clone()),
            Transform::from_xyz(x, 0.09, 0.0),
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
                // Blob shadow (flat plane facing up)
                p.spawn((
                    Mesh3d(tree_shadow_mesh.clone()),
                    MeshMaterial3d(shadow_mat.clone()),
                    Transform::from_xyz(0.0, 0.012, 0.0)
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
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
            .spawn((Transform::from_xyz(x, 0.0, z), Visibility::default()))
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
                // Blob shadow
                p.spawn((
                    Mesh3d(meshes.add(Plane3d::default().mesh().size(w * 1.4, d * 1.4))),
                    MeshMaterial3d(shadow_mat.clone()),
                    Transform::from_xyz(0.0, 0.012, 0.0)
                        .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
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

fn spawn_coins(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut score: ResMut<Score>,
) {
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

    score.total = positions.len() as u32;
    score.collected = 0;

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
    mut next: ResMut<NextState<GameState>>,
) {
    let Ok(car_t) = car.single() else {
        return;
    };
    for (e, coin_t) in &mut coins {
        if car_t.translation.distance(coin_t.translation) < 1.2 {
            commands.entity(e).despawn();
            score.collected += 1;
        }
    }
    if score.total > 0 && score.collected >= score.total {
        next.set(GameState::GameOver);
    }
}

fn cleanup_coins(mut commands: Commands, coins: Query<Entity, With<Coin>>) {
    for e in &coins {
        commands.entity(e).despawn();
    }
}
