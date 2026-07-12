use bevy::prelude::*;
use bevy::color::LinearRgba;
use std::f32::consts::FRAC_PI_2;

use crate::game::events::ObstacleHit;
use crate::game::resources::GameConfig;
use crate::game::state::GameState;
use crate::palette;
use crate::textures::TextureAssets;
use crate::world::{Collider, Curb};

#[derive(Component)]
pub struct Car {
    pub speed: f32,
    pub heading: f32,
}

/// Freeze car input (and round-timer burn) while a countdown is active. Set
/// by T6's countdown plugin; `move_car` early-returns while this is true.
#[derive(Resource, Default)]
pub struct InputFrozen(pub bool);

/// Tag for the car's painted body shell. Tilted by `roll_body` for a subtle
/// weight-shift when cornering; the cabin, glass and lights are nested under
/// it so they lean together.
#[derive(Component)]
struct CarBody;

/// A single wheel. `spin` accumulates rolling rotation (radians) driven by
/// `spin_wheels` from the car's speed.
#[derive(Component)]
struct Wheel {
    spin: f32,
}

/// Tag for brake-light children so `brake_lights` can find their shared
/// material and brighten it while braking.
#[derive(Component)]
struct BrakeLight;

pub struct CarPlugin;

impl Plugin for CarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InputFrozen>()
            .add_systems(Startup, spawn_car)
            .add_systems(
                Update,
                // move_car first, then resolve curb hops + obstacle collisions,
                // then the juice systems read the fresh speed.
                (move_car, physics_collisions, spin_wheels, roll_body, brake_lights)
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

fn spawn_car(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    textures: Res<TextureAssets>,
) {
    // --- Shared meshes/materials for the body's nested children ---
    let cabin_mesh = meshes.add(Cuboid::new(0.8, 0.4, 1.0));
    let cabin_mat = materials.add(StandardMaterial {
        base_color: palette::CAR_CABIN,
        perceptual_roughness: 0.4,
        metallic: 0.1,
        ..default()
    });

    // Windshield: thin dark-glass slab on the front of the cabin.
    let windshield_mesh = meshes.add(Cuboid::new(0.7, 0.2, 0.03));
    let windshield_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.05, 0.08, 0.12),
        perceptual_roughness: 0.08,
        metallic: 0.6,
        ..default()
    });

    // Headlights: warm emissive cubes at the front bumper.
    let headlight_mesh = meshes.add(Cuboid::new(0.18, 0.12, 0.04));
    let headlight_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.9, 0.6),
        emissive: LinearRgba::new(1.0, 0.9, 0.6, 1.0),
        ..default()
    });

    // Brake lights: red emissive cubes at the rear. Both children share one
    // material handle so `brake_lights` can dim/brighten them in one place.
    let brake_mesh = meshes.add(Cuboid::new(0.18, 0.12, 0.04));
    let brake_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.3, 0.02, 0.02),
        emissive: LinearRgba::new(0.8, 0.05, 0.05, 1.0),
        ..default()
    });

    // Wheels: cylinders with the axle along X, tire-black. Width 0.18 (not 0.3) so
    // they read as tires, not fat blocks.
    let wheel_mesh = meshes.add(Cylinder::new(0.15, 0.18));
    let wheel_mat = materials.add(StandardMaterial {
        base_color: palette::CAR_WHEEL,
        perceptual_roughness: 0.9,
        ..default()
    });

    // Fake blob shadow: dark alpha-blended patch under the car.
    let shadow_mesh = meshes.add(Plane3d::default().mesh().size(1.6, 2.4));
    let shadow_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.0, 0.0, 0.0, 0.35),
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    commands
        .spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            Visibility::default(),
            Car {
                speed: 0.0,
                heading: 0.0,
            },
        ))
        .with_children(|car| {
            // Painted body shell (car paint). Cabin + glass + lights nest
            // under it so the whole upper structure rolls together.
            car.spawn((
                Mesh3d(meshes.add(Cuboid::new(1.0, 0.5, 2.0))),
                MeshMaterial3d(textures.car_paint.clone()),
                Transform::from_xyz(0.0, 0.35, 0.0),
                CarBody,
            ))
            .with_children(|body| {
                // Cabin (sits on top of the body).
                body.spawn((
                    Mesh3d(cabin_mesh.clone()),
                    MeshMaterial3d(cabin_mat.clone()),
                    Transform::from_xyz(0.0, 0.35, 0.2),
                ));
                // Windshield on the front of the cabin (front is -Z).
                body.spawn((
                    Mesh3d(windshield_mesh.clone()),
                    MeshMaterial3d(windshield_mat.clone()),
                    Transform::from_xyz(0.0, 0.45, -0.3),
                ));
                // Headlights at the front bumper (-Z).
                for &(x, z) in &[(0.3, -1.0), (-0.3, -1.0)] {
                    body.spawn((
                        Mesh3d(headlight_mesh.clone()),
                        MeshMaterial3d(headlight_mat.clone()),
                        Transform::from_xyz(x, -0.1, z),
                    ));
                }
                // Brake lights at the rear (+Z).
                for &(x, z) in &[(0.3, 1.0), (-0.3, 1.0)] {
                    body.spawn((
                        Mesh3d(brake_mesh.clone()),
                        MeshMaterial3d(brake_mat.clone()),
                        Transform::from_xyz(x, -0.1, z),
                        BrakeLight,
                    ));
                }
            });

            // Wheels at the four corners, resting on the ground (radius 0.15
            // => center y = 0.15). Axle lies along X via from_rotation_z.
            for &(x, z) in &[(0.6, 0.7), (-0.6, 0.7), (0.6, -0.7), (-0.6, -0.7)] {
                car.spawn((
                    Mesh3d(wheel_mesh.clone()),
                    MeshMaterial3d(wheel_mat.clone()),
                    Transform::from_xyz(x, 0.15, z)
                        .with_rotation(Quat::from_rotation_z(FRAC_PI_2)),
                    Wheel { spin: 0.0 },
                ));
            }

            // Blob shadow, flat on the ground under the car. Plane3d::default()
            // already lies in the XZ plane (normal +Y), so no extra rotation is
            // needed — only the parent's heading rotation orients the footprint.
            // y is kept just above the ground; too low (e.g. 0.02) z-fights with
            // the ground plane under the ortho camera's depth precision.
            car.spawn((
                Mesh3d(shadow_mesh.clone()),
                MeshMaterial3d(shadow_mat.clone()),
                Transform::from_xyz(0.0, 0.06, 0.0),
            ));
        });
}

fn move_car(
    mut car: Query<(&mut Car, &mut Transform)>,
    keys: Res<ButtonInput<KeyCode>>,
    cfg: Res<GameConfig>,
    time: Res<Time>,
    input_frozen: Res<InputFrozen>,
) {
    // Countdown / freeze gate: the car holds still (and the round timer stops
    // burning) while a countdown overlay is active.
    if input_frozen.0 {
        return;
    }
    let Ok((mut car, mut tf)) = car.single_mut() else {
        return;
    };
    let dt = time.delta_secs();

    let forward_in = keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp);
    let back_in = keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown);
    let brake_in = keys.pressed(KeyCode::Space);

    // Eased approach to a target speed. Brake dominates, then accel, then
    // capped reverse, then coast. `rate` controls how quickly `speed`
    // converges to `target`.
    let (target, rate) = if brake_in {
        (0.0, 14.0)
    } else if forward_in {
        (cfg.max_speed, 3.0)
    } else if back_in {
        (-cfg.max_speed * 0.5, 3.0)
    } else {
        (0.0, 2.0)
    };

    car.speed += (target - car.speed) * rate * dt;
    car.speed = car.speed.clamp(-cfg.max_speed, cfg.max_speed);
    if car.speed.abs() < 0.01 && target == 0.0 {
        car.speed = 0.0;
    }

    let steer = if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        1.0
    } else if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        -1.0
    } else {
        0.0
    };

    // Steering scales with speed so the car can't spin in place.
    car.heading += steer * cfg.turn_rate * dt * (car.speed / cfg.max_speed);

    let forward = Vec3::new(-car.heading.sin(), 0.0, -car.heading.cos());
    tf.translation += forward * car.speed * dt;
    // Infinite road along Z: clamp only X (keep the car on the grass strip
    // beside the road). Z is unbounded so chunks recycle endlessly.
    tf.translation.x = tf.translation.x.clamp(-24.0, 24.0);
    tf.rotation = Quat::from_rotation_y(car.heading);
}

fn spin_wheels(
    cars: Query<&Car>,
    mut wheels: Query<(&mut Transform, &mut Wheel)>,
    time: Res<Time>,
) {
    let Ok(car) = cars.single() else {
        return;
    };
    let dt = time.delta_secs();
    // Rolling: distance travelled / radius => radians.
    let spin_delta = car.speed.abs() * dt / 0.15;
    for (mut tf, mut wheel) in &mut wheels {
        wheel.spin += spin_delta;
        // Lay the axle along X (Rz) and roll around the axle = local Y (Ry).
        tf.rotation = Quat::from_rotation_z(FRAC_PI_2) * Quat::from_rotation_y(wheel.spin);
    }
}

fn roll_body(
    cars: Query<&Car>,
    mut bodies: Query<&mut Transform, With<CarBody>>,
    keys: Res<ButtonInput<KeyCode>>,
    cfg: Res<GameConfig>,
) {
    let Ok(car) = cars.single() else {
        return;
    };
    let steer = if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        1.0
    } else if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        -1.0
    } else {
        0.0
    };
    let speed_frac = (car.speed / cfg.max_speed).clamp(-1.0, 1.0);
    // Lean into the turn: tilt around the car's longitudinal (Z) axis.
    let tilt = -steer * speed_frac * 0.12;
    for mut tf in &mut bodies {
        tf.rotation = Quat::from_rotation_z(tilt);
    }
}

fn brake_lights(
    keys: Res<ButtonInput<KeyCode>>,
    brake_q: Query<&MeshMaterial3d<StandardMaterial>, With<BrakeLight>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let braking = keys.pressed(KeyCode::KeyS)
        || keys.pressed(KeyCode::ArrowDown)
        || keys.pressed(KeyCode::Space);
    let intensity = if braking { 1.0 } else { 0.25 };
    for mat in &brake_q {
        if let Some(mut m) = materials.get_mut(mat) {
            m.emissive = LinearRgba::new(
                0.8 * intensity,
                0.05 * intensity,
                0.05 * intensity,
                1.0,
            );
        }
    }
}

/// Ground-level physics + obstacle collisions, run right after `move_car`:
/// - hop the car up onto any raised curb it drives over (smoothed Y lerp);
/// - push the car out of any solid obstacle (buildings / trees / lamp posts)
///   via circle-vs-AABB and kill speed into the wall, emitting an
///   `ObstacleHit` message so the health system can apply damage.
pub fn physics_collisions(
    mut car: Query<(&mut Car, &mut Transform), With<Car>>,
    curbs: Query<(&Curb, &GlobalTransform), (With<Curb>, Without<Car>, Without<Collider>)>,
    obstacles: Query<(&Collider, &GlobalTransform), (With<Collider>, Without<Car>, Without<Curb>)>,
    time: Res<Time>,
    mut obstacle_hits: MessageWriter<ObstacleHit>,
) {
    let Ok((mut car, mut tf)) = car.single_mut() else {
        return;
    };
    let dt = time.delta_secs();
    const CAR_RADIUS: f32 = 0.9;

    // --- Ground height: hop up onto any curb the car is over. ---
    // Curbs (and obstacles/coins) are children of chunk roots, so their
    // `Transform` is LOCAL to the chunk — use `GlobalTransform` for world
    // positions or collision won't line up with the visuals.
    let mut target_y = 0.0_f32;
    for (curb, ct) in &curbs {
        let cpos = ct.translation();
        let dx = tf.translation.x - cpos.x;
        let dz = tf.translation.z - cpos.z;
        if dx.abs() <= curb.half_x && dz.abs() <= curb.half_z {
            target_y = target_y.max(curb.height);
        }
    }
    tf.translation.y += (target_y - tf.translation.y) * (1.0 - (-10.0 * dt).exp());

    // --- Obstacle collision: circle-vs-AABB pushout + kill speed into wall. ---
    let forward = Vec3::new(-car.heading.sin(), 0.0, -car.heading.cos());
    // Minimum speed for a hit to deal damage — low-speed wall taps (e.g.
    // pressing into a wall from a standstill) shouldn't hurt, and this also
    // ignores the tiny speed a freshly-spawned car has.
    const MIN_IMPACT_SPEED: f32 = 3.0;
    for (collider, ot) in &obstacles {
        let opos = ot.translation();
        // Skip colliders whose GlobalTransform hasn't propagated yet (still
        // IDENTITY at the world origin). No real obstacle sits at the origin
        // (all are at |x| >= 6), so this filters the 1-frame stale transform
        // after chunk spawn that otherwise piles every collider onto the car.
        if *ot == GlobalTransform::IDENTITY {
            continue;
        }
        let dx = tf.translation.x - opos.x;
        let dz = tf.translation.z - opos.z;
        let closest_x = dx.clamp(-collider.half_x, collider.half_x);
        let closest_z = dz.clamp(-collider.half_z, collider.half_z);
        let px = dx - closest_x;
        let pz = dz - closest_z;
        let dist2 = px * px + pz * pz;
        if dist2 < CAR_RADIUS * CAR_RADIUS {
            let (nx, nz, pen) = if dist2 > 1e-6 {
                let dist = dist2.sqrt();
                (px / dist, pz / dist, CAR_RADIUS - dist)
            } else {
                // Center inside the box: eject along the least-penetrated axis.
                let pen_x = collider.half_x - dx.abs();
                let pen_z = collider.half_z - dz.abs();
                if pen_x < pen_z {
                    let s = if dx >= 0.0 { 1.0 } else { -1.0 };
                    (s, 0.0, pen_x + CAR_RADIUS)
                } else {
                    let s = if dz >= 0.0 { 1.0 } else { -1.0 };
                    (0.0, s, pen_z + CAR_RADIUS)
                }
            };
            tf.translation.x += nx * pen;
            tf.translation.z += nz * pen;
            let into = forward.x * nx + forward.z * nz;
            if car.speed.abs() > MIN_IMPACT_SPEED
                && ((into < 0.0 && car.speed > 0.0) || (into > 0.0 && car.speed < 0.0))
            {
                // The car was driving into the wall fast enough to hurt:
                // report the impact for damage, then kill the speed.
                obstacle_hits.write(ObstacleHit {
                    impact_speed: car.speed.abs(),
                });
                car.speed = 0.0;
            }
        }
    }
}
