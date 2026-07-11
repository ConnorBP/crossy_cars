use bevy::prelude::*;

use crate::game::resources::GameConfig;
use crate::game::state::GameState;
use crate::palette;

#[derive(Component)]
pub struct Car {
    pub speed: f32,
    pub heading: f32,
}

pub struct CarPlugin;

impl Plugin for CarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_car)
            .add_systems(Update, move_car.run_if(in_state(GameState::Playing)));
    }
}

fn spawn_car(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands
        .spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            Visibility::default(),
            Car {
                speed: 0.0,
                heading: 0.0,
            },
        ))
        .with_children(|p| {
            // Body
            p.spawn((
                Mesh3d(meshes.add(Cuboid::new(1.0, 0.5, 2.0))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: palette::CAR_BODY,
                    ..default()
                })),
                Transform::from_xyz(0.0, 0.35, 0.0),
            ));
            // Cabin
            p.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.8, 0.4, 1.0))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: palette::CAR_CABIN,
                    ..default()
                })),
                Transform::from_xyz(0.0, 0.7, 0.2),
            ));
            // Wheels
            let wheel_mesh = meshes.add(Cuboid::new(0.2, 0.2, 0.3));
            let wheel_mat = materials.add(StandardMaterial {
                base_color: palette::CAR_WHEEL,
                ..default()
            });
            for &(x, z) in &[(0.6, 0.7), (-0.6, 0.7), (0.6, -0.7), (-0.6, -0.7)] {
                p.spawn((
                    Mesh3d(wheel_mesh.clone()),
                    MeshMaterial3d(wheel_mat.clone()),
                    Transform::from_xyz(x, 0.1, z),
                ));
            }
        });
}

fn move_car(
    mut car: Query<(&mut Car, &mut Transform)>,
    keys: Res<ButtonInput<KeyCode>>,
    cfg: Res<GameConfig>,
    time: Res<Time>,
) {
    let Ok((mut car, mut tf)) = car.single_mut() else {
        return;
    };
    let dt = time.delta_secs();

    let accel = if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        1.0
    } else if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        -1.0
    } else {
        0.0
    };
    let steer = if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        1.0
    } else if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        -1.0
    } else {
        0.0
    };

    car.speed += accel * cfg.accel * dt;
    car.speed -= car.speed * cfg.drag * dt;
    car.speed = car.speed.clamp(-cfg.max_speed, cfg.max_speed);
    if car.speed.abs() < 0.01 && accel == 0.0 {
        car.speed = 0.0;
    }

    car.heading += steer * cfg.turn_rate * dt * (car.speed / cfg.max_speed);

    let forward = Vec3::new(-car.heading.sin(), 0.0, -car.heading.cos());
    tf.translation += forward * car.speed * dt;
    tf.translation.x = tf.translation.x.clamp(cfg.arena_min, cfg.arena_max);
    tf.translation.z = tf.translation.z.clamp(cfg.arena_min, cfg.arena_max);
    tf.rotation = Quat::from_rotation_y(car.heading);
}
