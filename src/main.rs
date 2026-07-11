use bevy::{camera::ScalingMode, prelude::*, text::FontSize};

#[derive(Component)]
struct Car {
    speed: f32,
    heading: f32,
}

#[derive(Component)]
struct SpeedText;

const ACCEL: f32 = 8.0;
const MAX_SPEED: f32 = 12.0;
const TURN_RATE: f32 = 2.5;
const DRAG: f32 = 0.8;
const CAM_OFFSET: Vec3 = Vec3::new(12.0, 12.0, 12.0);

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .insert_resource(ClearColor(Color::srgb(0.53, 0.81, 0.92)))
        .insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 150.0,
            ..default()
        })
        .add_systems(Startup, setup)
        .add_systems(Update, (move_car, follow_camera, update_speed_text).chain())
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Isometric-style orthographic camera.
    commands.spawn((
        Camera3d::default(),
        Projection::from(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: 10.0,
            },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(12.0, 12.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Sun (default illuminance = daylight; shadows off for web perf).
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_xyz(10.0, 20.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Checkerboard ground so motion is visible in the iso view.
    for i in 0..10 {
        for j in 0..10 {
            let shade = if (i + j) % 2 == 0 { 0.30 } else { 0.42 };
            commands.spawn((
                Mesh3d(meshes.add(Plane3d::default().mesh().size(10.0, 10.0))),
                MeshMaterial3d(materials.add(Color::srgb(shade, 0.6, shade))),
                Transform::from_xyz(i as f32 * 10.0 - 45.0, 0.0, j as f32 * 10.0 - 45.0),
            ));
        }
    }

    // Car: parent holds position + heading; children are body, cabin, wheels.
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
            p.spawn((
                Mesh3d(meshes.add(Cuboid::new(1.0, 0.5, 2.0))),
                MeshMaterial3d(materials.add(Color::srgb(0.9, 0.1, 0.1))),
                Transform::from_xyz(0.0, 0.35, 0.0),
            ));
            p.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.8, 0.4, 1.0))),
                MeshMaterial3d(materials.add(Color::srgb(0.1, 0.1, 0.2))),
                Transform::from_xyz(0.0, 0.7, 0.2),
            ));
            let wheel_mesh = meshes.add(Cuboid::new(0.2, 0.2, 0.3));
            let wheel_mat = materials.add(Color::srgb(0.05, 0.05, 0.05));
            for &(x, z) in &[(0.6, 0.7), (-0.6, 0.7), (0.6, -0.7), (-0.6, -0.7)] {
                p.spawn((
                    Mesh3d(wheel_mesh.clone()),
                    MeshMaterial3d(wheel_mat.clone()),
                    Transform::from_xyz(x, 0.1, z),
                ));
            }
        });

    // Speed HUD (top-left).
    commands
        .spawn((
            Text::new("Speed: "),
            TextFont {
                font_size: FontSize::Px(28.0),
                ..default()
            },
            Node {
                position_type: PositionType::Absolute,
                top: px(10.0),
                left: px(10.0),
                ..default()
            },
        ))
        .with_child((
            TextSpan::default(),
            TextFont {
                font_size: FontSize::Px(28.0),
                ..default()
            },
            TextColor(Color::WHITE.into()),
            SpeedText,
        ));
}

fn move_car(
    mut car: Query<(&mut Car, &mut Transform)>,
    keys: Res<ButtonInput<KeyCode>>,
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

    car.speed += accel * ACCEL * dt;
    car.speed -= car.speed * DRAG * dt;
    car.speed = car.speed.clamp(-MAX_SPEED, MAX_SPEED);
    if car.speed.abs() < 0.01 && accel == 0.0 {
        car.speed = 0.0;
    }

    // Turn rate scales with speed (no spinning in place); reverses correctly.
    car.heading += steer * TURN_RATE * dt * (car.speed / MAX_SPEED);

    let forward = Vec3::new(-car.heading.sin(), 0.0, -car.heading.cos());
    tf.translation += forward * car.speed * dt;
    tf.translation.x = tf.translation.x.clamp(-49.0, 49.0);
    tf.translation.z = tf.translation.z.clamp(-49.0, 49.0);
    tf.rotation = Quat::from_rotation_y(car.heading);
}

fn follow_camera(
    car: Query<&Transform, (With<Car>, Without<Camera3d>)>,
    mut camera: Query<&mut Transform, (With<Camera3d>, Without<Car>)>,
) {
    let Ok(car_t) = car.single() else {
        return;
    };
    let Ok(mut cam_t) = camera.single_mut() else {
        return;
    };
    let pos = car_t.translation;
    *cam_t = Transform::from_translation(pos + CAM_OFFSET).looking_at(pos, Vec3::Y);
}

fn update_speed_text(
    car: Query<&Car>,
    mut query: Query<&mut TextSpan, With<SpeedText>>,
) {
    let Ok(car) = car.single() else {
        return;
    };
    for mut span in &mut query {
        **span = format!("{:.1} u/s", car.speed.abs());
    }
}
