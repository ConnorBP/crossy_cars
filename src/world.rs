use bevy::prelude::*;

use crate::car::Car;
use crate::game::resources::Score;
use crate::game::state::GameState;
use crate::palette;

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
    // Sun (default illuminance = daylight; shadows off for web perf).
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_xyz(10.0, 20.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Checkerboard ground.
    for i in 0..10 {
        for j in 0..10 {
            let shade = if (i + j) % 2 == 0 { 0.30 } else { 0.42 };
            commands.spawn((
                Mesh3d(meshes.add(Plane3d::default().mesh().size(10.0, 10.0))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(shade, 0.6, shade),
                    ..default()
                })),
                Transform::from_xyz(i as f32 * 10.0 - 45.0, 0.0, j as f32 * 10.0 - 45.0),
            ));
        }
    }
}

fn spawn_coins(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut score: ResMut<Score>,
) {
    let positions: [(f32, f32); 16] = [
        (8.0, 8.0), (16.0, 12.0), (24.0, 8.0), (32.0, 16.0),
        (40.0, 24.0), (32.0, 32.0), (24.0, 40.0), (16.0, 32.0),
        (8.0, 24.0), (-8.0, -8.0), (-16.0, -12.0), (-24.0, -8.0),
        (-32.0, -16.0), (-40.0, -24.0), (-24.0, 24.0), (8.0, -24.0),
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
