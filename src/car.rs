use bevy::color::LinearRgba;
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use std::f32::consts::{FRAC_PI_2, TAU};

use crate::difficulty::Traffic;
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

/// Centralized player driving intent. Keyboard input populates this resource;
/// other input methods can write the same normalized controls later.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Default)]
pub struct PlayerInput {
    /// Reverse (-1.0) through forward (1.0).
    pub throttle: f32,
    /// Right (-1.0) through left (1.0), matching the car's steering sign.
    pub steer: f32,
    /// Active braking. This takes precedence over throttle in `move_car`.
    pub brake: bool,
}

// Exponential speed-response rates (per second). Service braking is
// deliberately stronger than acceleration/coasting without snapping the car
// to a halt: from speed 12 it takes about 1.75 s to reach the stop threshold,
// leaving enough braking distance for the rear skid marks to read clearly.
const ACCEL_RESPONSE_RATE: f32 = 3.0;
const COAST_RESPONSE_RATE: f32 = 2.0;
const BRAKE_RESPONSE_RATE: f32 = 4.0;
const STOP_SPEED_THRESHOLD: f32 = 0.01;

/// Pure speed integration shared by gameplay and tests. Exponential response
/// keeps the feel consistent across frame rates and makes braking progressively
/// ease toward rest rather than applying an abrupt fixed-speed cut.
fn next_speed(current: f32, max_speed: f32, input: PlayerInput, dt: f32) -> f32 {
    // Brake dominates, then forward acceleration, capped reverse, then coast.
    let (target, rate) = if input.brake {
        (0.0, BRAKE_RESPONSE_RATE)
    } else if input.throttle > 0.0 {
        (
            max_speed * input.throttle.clamp(0.0, 1.0),
            ACCEL_RESPONSE_RATE,
        )
    } else if input.throttle < 0.0 {
        (
            max_speed * 0.5 * input.throttle.clamp(-1.0, 0.0),
            ACCEL_RESPONSE_RATE,
        )
    } else {
        (0.0, COAST_RESPONSE_RATE)
    };

    let alpha = 1.0 - (-rate * dt.max(0.0)).exp();
    let mut speed = (current + (target - current) * alpha).clamp(-max_speed, max_speed);
    if speed.abs() < STOP_SPEED_THRESHOLD && target == 0.0 {
        speed = 0.0;
    }
    speed
}

/// Convert the keyboard's individual bindings into normalized driving intent.
/// Opposite directions cancel, while duplicate bindings for one direction are
/// combined and clamped to a single unit of input.
fn map_keyboard_input(
    w: bool,
    up: bool,
    s: bool,
    down: bool,
    a: bool,
    left: bool,
    d: bool,
    right: bool,
    space: bool,
) -> PlayerInput {
    let forward = w || up;
    let reverse = s || down;
    let steer_left = a || left;
    let steer_right = d || right;

    PlayerInput {
        throttle: ((forward as i8 - reverse as i8) as f32).clamp(-1.0, 1.0),
        steer: ((steer_left as i8 - steer_right as i8) as f32).clamp(-1.0, 1.0),
        brake: space,
    }
}

/// Tag for the car's painted body shell. Tilted by `roll_body` for a subtle
/// weight-shift when cornering; the cabin, glass and lights are nested under
/// it so they lean together.
#[derive(Component)]
struct CarBody;

/// Smoothed visual suspension state. This is deliberately kept on the body
/// child so pitch and roll never feed back into the car's driving transform.
#[derive(Component, Default)]
struct BodyMotion {
    roll: f32,
    pitch: f32,
    previous_speed: f32,
}

/// A single wheel. `spin` accumulates rolling rotation (radians) driven by
/// `spin_wheels` from the car's speed.
#[derive(Component)]
struct Wheel {
    spin: f32,
    steer: f32,
}

/// Front wheels yaw for steering in addition to sharing the tire roll logic.
#[derive(Component)]
struct FrontWheel;

/// Tag for brake-light children so `brake_lights` can find their shared
/// material and brighten it while braking.
#[derive(Component)]
struct BrakeLight;

/// Update ordering shared by keyboard, touch, and car simulation. Touch input
/// augments the keyboard-populated [`PlayerInput`] before driving consumes it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct KeyboardInputSet;

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TouchInputSet;

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DrivingSet;

pub struct CarPlugin;

impl Plugin for CarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InputFrozen>()
            .init_resource::<PlayerInput>()
            .configure_sets(
                Update,
                (KeyboardInputSet, TouchInputSet, DrivingSet).chain(),
            )
            .add_systems(Startup, spawn_car)
            // Keep this reader active in every state so menu/pause/freeze
            // transitions immediately clear input instead of retaining a held
            // value from the previous Playing frame.
            .add_systems(Update, read_keyboard_input.in_set(KeyboardInputSet))
            .add_systems(
                Update,
                // move_car first, then resolve curb hops + obstacle collisions,
                // then the juice systems read the fresh speed.
                (
                    move_car,
                    physics_collisions,
                    spin_wheels,
                    roll_body,
                    brake_lights,
                )
                    .chain()
                    .in_set(DrivingSet)
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

/// Build a smooth ellipsoid body with the car's dimensions baked into its
/// vertices. Baking avoids non-uniform `Transform` scale (which distorts mesh
/// normals) and the analytic ellipsoid normals give the paint shader a real,
/// continuous reflection sweep instead of one flat color per cuboid face.
fn car_body_mesh() -> Mesh {
    const AXES: Vec3 = Vec3::new(0.5, 0.25, 1.0);

    let mut mesh = Sphere::new(0.5)
        .mesh()
        .ico(4)
        .expect("car body icosphere subdivision is valid");
    let positions = match mesh.remove_attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(values)) => values,
        _ => panic!("icosphere positions must be Float32x3"),
    };
    let (positions, normals): (Vec<[f32; 3]>, Vec<[f32; 3]>) = positions
        .into_iter()
        .map(|position| {
            let sphere_position = Vec3::from_array(position);
            let p = sphere_position * (AXES / 0.5);
            let n = Vec3::new(
                p.x / AXES.x.powi(2),
                p.y / AXES.y.powi(2),
                p.z / AXES.z.powi(2),
            )
            .normalize();
            (p.to_array(), n.to_array())
        })
        .unzip();
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh
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

    // A few shared primitives make a readable greenhouse without generating
    // unique meshes or materials per window (important for the web build).
    let end_glass_mesh = meshes.add(Cuboid::new(0.68, 0.23, 0.025));
    let side_glass_mesh = meshes.add(Cuboid::new(0.025, 0.22, 0.56));
    let roof_mesh = meshes.add(Cuboid::new(0.72, 0.07, 0.74));
    let glass_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.035, 0.065, 0.1),
        perceptual_roughness: 0.08,
        metallic: 0.55,
        ..default()
    });

    // Front and rear fascia share the paint and geometry. Thin lamps sit just
    // proud of their face rather than floating at the ellipsoid's tips.
    let fascia_mesh = meshes.add(Cuboid::new(0.82, 0.16, 0.09));
    let bumper_mesh = meshes.add(Cuboid::new(0.9, 0.065, 0.07));
    let trim_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.06, 0.07, 0.075),
        metallic: 0.65,
        perceptual_roughness: 0.3,
        ..default()
    });
    let grille_mesh = meshes.add(Cuboid::new(0.32, 0.075, 0.025));
    let grille_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.008, 0.01, 0.012),
        perceptual_roughness: 0.9,
        ..default()
    });

    // Headlights: warm emissive lenses seated in the front fascia.
    let headlight_mesh = meshes.add(Cuboid::new(0.18, 0.1, 0.025));
    let headlight_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.9, 0.6),
        emissive: LinearRgba::new(1.0, 0.9, 0.6, 1.0),
        perceptual_roughness: 0.18,
        ..default()
    });

    // Brake lights: red emissive lenses at the rear. Both children share one
    // material handle so `brake_lights` can dim/brighten them in one place.
    let brake_mesh = meshes.add(Cuboid::new(0.18, 0.1, 0.025));
    let brake_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.3, 0.02, 0.02),
        emissive: LinearRgba::new(0.8, 0.05, 0.05, 1.0),
        perceptual_roughness: 0.22,
        ..default()
    });

    // Wheels: cylinders with the axle along X, tire-black. Width 0.18 (not 0.3) so
    // they read as tires, not fat blocks. One slightly wider hub cylinder exposes
    // a metallic cap on both outside faces without extra entities per side.
    let wheel_mesh = meshes.add(Cylinder::new(0.15, 0.18));
    let wheel_mat = materials.add(StandardMaterial {
        base_color: palette::CAR_WHEEL,
        perceptual_roughness: 0.9,
        ..default()
    });
    let hub_mesh = meshes.add(Cylinder::new(0.066, 0.19));
    let hub_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.42, 0.45, 0.48),
        metallic: 0.9,
        perceptual_roughness: 0.2,
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
                Mesh3d(meshes.add(car_body_mesh())),
                MeshMaterial3d(textures.car_paint.clone()),
                Transform::from_xyz(0.0, 0.35, 0.0),
                CarBody,
                BodyMotion::default(),
            ))
            .with_children(|body| {
                // Cabin core, four dark glass faces, and a painted roof make a
                // compact greenhouse whose front/rear slope reads at a glance.
                body.spawn((
                    Mesh3d(cabin_mesh.clone()),
                    MeshMaterial3d(cabin_mat.clone()),
                    Transform::from_xyz(0.0, 0.35, 0.2),
                ));
                body.spawn((
                    Mesh3d(end_glass_mesh.clone()),
                    MeshMaterial3d(glass_mat.clone()),
                    Transform::from_xyz(0.0, 0.39, -0.305)
                        .with_rotation(Quat::from_rotation_x(0.24)),
                ));
                body.spawn((
                    Mesh3d(end_glass_mesh.clone()),
                    MeshMaterial3d(glass_mat.clone()),
                    Transform::from_xyz(0.0, 0.39, 0.705)
                        .with_rotation(Quat::from_rotation_x(-0.24)),
                ));
                for x in [-0.405, 0.405] {
                    body.spawn((
                        Mesh3d(side_glass_mesh.clone()),
                        MeshMaterial3d(glass_mat.clone()),
                        Transform::from_xyz(x, 0.39, 0.2),
                    ));
                }
                body.spawn((
                    Mesh3d(roof_mesh.clone()),
                    MeshMaterial3d(textures.car_paint.clone()),
                    Transform::from_xyz(0.0, 0.575, 0.2),
                ));

                // Painted fascia and dark lower bumper are shared nose-to-tail.
                for z in [-0.87, 0.87] {
                    body.spawn((
                        Mesh3d(fascia_mesh.clone()),
                        MeshMaterial3d(textures.car_paint.clone()),
                        Transform::from_xyz(0.0, -0.08, z),
                    ));
                    body.spawn((
                        Mesh3d(bumper_mesh.clone()),
                        MeshMaterial3d(trim_mat.clone()),
                        Transform::from_xyz(0.0, -0.18, z.signum() * 0.9),
                    ));
                }
                // The recessed black grille marks the nose (front is -Z).
                body.spawn((
                    Mesh3d(grille_mesh.clone()),
                    MeshMaterial3d(grille_mat.clone()),
                    Transform::from_xyz(0.0, -0.1, -0.922),
                ));
                for x in [-0.27, 0.27] {
                    body.spawn((
                        Mesh3d(headlight_mesh.clone()),
                        MeshMaterial3d(headlight_mat.clone()),
                        Transform::from_xyz(x, -0.055, -0.929),
                    ));
                    body.spawn((
                        Mesh3d(brake_mesh.clone()),
                        MeshMaterial3d(brake_mat.clone()),
                        Transform::from_xyz(x, -0.055, 0.929),
                        BrakeLight,
                    ));
                }
            });

            // Wheels at the four corners, resting on the ground (radius 0.15
            // => center y = 0.15). Axle lies along X via from_rotation_z.
            for &(x, z) in &[(0.6, 0.7), (-0.6, 0.7), (0.6, -0.7), (-0.6, -0.7)] {
                let mut wheel = car.spawn((
                    Mesh3d(wheel_mesh.clone()),
                    MeshMaterial3d(wheel_mat.clone()),
                    Transform::from_xyz(x, 0.15, z).with_rotation(Quat::from_rotation_z(FRAC_PI_2)),
                    Wheel {
                        spin: 0.0,
                        steer: 0.0,
                    },
                ));
                if z < 0.0 {
                    wheel.insert(FrontWheel);
                }
                wheel.with_children(|wheel| {
                    wheel.spawn((
                        Mesh3d(hub_mesh.clone()),
                        MeshMaterial3d(hub_mat.clone()),
                        Transform::default(),
                    ));
                });
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

fn read_keyboard_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut input: ResMut<PlayerInput>,
    state: Res<State<GameState>>,
    input_frozen: Res<InputFrozen>,
) {
    if *state.get() != GameState::Playing || input_frozen.0 {
        *input = PlayerInput::default();
        return;
    }

    *input = map_keyboard_input(
        keys.pressed(KeyCode::KeyW),
        keys.pressed(KeyCode::ArrowUp),
        keys.pressed(KeyCode::KeyS),
        keys.pressed(KeyCode::ArrowDown),
        keys.pressed(KeyCode::KeyA),
        keys.pressed(KeyCode::ArrowLeft),
        keys.pressed(KeyCode::KeyD),
        keys.pressed(KeyCode::ArrowRight),
        keys.pressed(KeyCode::Space),
    );
}

fn move_car(
    mut car: Query<(&mut Car, &mut Transform)>,
    input: Res<PlayerInput>,
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

    car.speed = next_speed(car.speed, cfg.max_speed, *input, dt);

    // Steering scales with speed so the car can't spin in place.
    car.heading += input.steer.clamp(-1.0, 1.0) * cfg.turn_rate * dt * (car.speed / cfg.max_speed);

    let forward = Vec3::new(-car.heading.sin(), 0.0, -car.heading.cos());
    tf.translation += forward * car.speed * dt;
    // 2D city grid (T14): the car can drive freely in X and Z — the grid
    // recycles in all 4 directions, so no clamp is needed.
    tf.rotation = Quat::from_rotation_y(car.heading);
}

fn spin_wheels(
    cars: Query<&Car>,
    mut wheels: Query<(&mut Transform, &mut Wheel, Option<&FrontWheel>)>,
    input: Res<PlayerInput>,
    time: Res<Time>,
) {
    let Ok(car) = cars.single() else {
        return;
    };
    let dt = time.delta_secs();
    let steer_input = input.steer.clamp(-1.0, 1.0);
    // Rolling: distance travelled / radius => radians. Rebuild the quaternion
    // from independent yaw/base/roll terms every frame so steering cannot
    // accumulate into a tumbling wheel.
    let spin_delta = car.speed.abs() * dt / 0.15;
    let steer_alpha = 1.0 - (-14.0 * dt).exp();
    for (mut tf, mut wheel, front) in &mut wheels {
        wheel.spin = (wheel.spin + spin_delta).rem_euclid(TAU);
        let target_steer = if front.is_some() {
            steer_input * 0.36
        } else {
            0.0
        };
        wheel.steer += (target_steer - wheel.steer) * steer_alpha;
        tf.rotation = Quat::from_rotation_y(wheel.steer)
            * Quat::from_rotation_z(FRAC_PI_2)
            * Quat::from_rotation_y(wheel.spin);
    }
}

fn roll_body(
    cars: Query<&Car>,
    mut bodies: Query<(&mut Transform, &mut BodyMotion), With<CarBody>>,
    input: Res<PlayerInput>,
    cfg: Res<GameConfig>,
    time: Res<Time>,
) {
    let Ok(car) = cars.single() else {
        return;
    };
    let steer = input.steer.clamp(-1.0, 1.0);
    let dt = time.delta_secs();
    let speed_frac = (car.speed / cfg.max_speed).clamp(-1.0, 1.0);
    let target_roll = -steer * speed_frac * 0.12;
    for (mut tf, mut motion) in &mut bodies {
        // Longitudinal acceleration is sampled only for presentation. Positive
        // acceleration lifts the -Z nose; braking settles it down.
        let acceleration = if dt > f32::EPSILON {
            (car.speed - motion.previous_speed) / dt
        } else {
            0.0
        };
        motion.previous_speed = car.speed;
        let target_pitch = (acceleration * 0.0015).clamp(-0.045, 0.045);
        motion.roll += (target_roll - motion.roll) * (1.0 - (-9.0 * dt).exp());
        motion.pitch += (target_pitch - motion.pitch) * (1.0 - (-7.0 * dt).exp());
        tf.rotation = Quat::from_rotation_x(motion.pitch) * Quat::from_rotation_z(motion.roll);
    }
}

fn brake_lights(
    input: Res<PlayerInput>,
    brake_q: Query<&MeshMaterial3d<StandardMaterial>, With<BrakeLight>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let braking = input.throttle < 0.0 || input.brake;
    let intensity = if braking { 1.0 } else { 0.25 };
    for mat in &brake_q {
        if let Some(mut m) = materials.get_mut(mat) {
            m.emissive = LinearRgba::new(0.8 * intensity, 0.05 * intensity, 0.05 * intensity, 1.0);
        }
    }
}

/// Player velocity in the world XZ plane. Keeping this separate from the
/// collision query makes signed reverse speed and heading behavior explicit.
fn player_velocity(heading: f32, speed: f32) -> Vec2 {
    Vec2::new(-heading.sin(), -heading.cos()) * speed
}

/// Traffic velocity in the world XZ plane from its axis/direction contract.
fn traffic_velocity(axis: bool, dir: f32, speed: f32) -> Vec2 {
    if axis {
        Vec2::new(dir * speed, 0.0)
    } else {
        Vec2::new(0.0, dir * speed)
    }
}

/// Impact magnitude against either an immobile obstacle or moving traffic.
/// Static obstacles retain the player's absolute speed; traffic uses relative
/// velocity, covering a parked player being rammed as well as closing speeds.
fn obstacle_impact_speed(player: Vec2, traffic: Option<Vec2>) -> f32 {
    (player - traffic.unwrap_or(Vec2::ZERO)).length()
}

/// Ground-level physics + obstacle collisions, run right after `move_car`:
/// - hop the car up onto any raised curb it drives over (smoothed Y lerp);
/// - push the car out of any solid static obstacle or traffic car via
///   circle-vs-AABB and kill speed into it, emitting an `ObstacleHit` message
///   whose impact is the player speed for static objects and relative speed
///   for traffic.
pub fn physics_collisions(
    mut car: Query<(&mut Car, &mut Transform), (With<Car>, Without<Traffic>)>,
    curbs: Query<(&Curb, &GlobalTransform), (With<Curb>, Without<Car>, Without<Collider>)>,
    obstacles: Query<
        (&Collider, &GlobalTransform, Option<&Traffic>),
        (With<Collider>, Without<Car>, Without<Curb>),
    >,
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

    // --- Obstacle collision: circle-vs-AABB pushout + kill speed into it. ---
    // Minimum relative speed for a hit to deal damage — low-speed wall taps
    // and gentle traffic contacts should not hurt.
    const MIN_IMPACT_SPEED: f32 = 3.0;
    for (collider, ot, traffic) in &obstacles {
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

            let player_vel = player_velocity(car.heading, car.speed);
            let traffic_vel =
                traffic.map(|traffic| traffic_velocity(traffic.axis, traffic.dir, traffic.speed));
            let relative_velocity = player_vel - traffic_vel.unwrap_or(Vec2::ZERO);
            let impact_speed = obstacle_impact_speed(player_vel, traffic_vel);
            let collision_normal = Vec2::new(nx, nz);
            if impact_speed > MIN_IMPACT_SPEED && relative_velocity.dot(collision_normal) < 0.0 {
                // The player and obstacle are closing fast enough to hurt.
                // Report relative impact for traffic, then kill player speed
                // exactly as for a static obstacle.
                obstacle_hits.write(ObstacleHit { impact_speed });
                car.speed = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapped(keys: [bool; 9]) -> PlayerInput {
        map_keyboard_input(
            keys[0], keys[1], keys[2], keys[3], keys[4], keys[5], keys[6], keys[7], keys[8],
        )
    }

    #[test]
    fn player_input_defaults_to_zero() {
        assert_eq!(
            PlayerInput::default(),
            PlayerInput {
                throttle: 0.0,
                steer: 0.0,
                brake: false,
            }
        );
        assert_eq!(mapped([false; 9]), PlayerInput::default());
    }

    #[test]
    fn each_forward_and_reverse_key_has_the_expected_sign() {
        for index in [0, 1] {
            let mut keys = [false; 9];
            keys[index] = true;
            assert_eq!(mapped(keys).throttle, 1.0);
        }
        for index in [2, 3] {
            let mut keys = [false; 9];
            keys[index] = true;
            assert_eq!(mapped(keys).throttle, -1.0);
        }
    }

    #[test]
    fn each_left_and_right_key_has_the_existing_steering_sign() {
        for index in [4, 5] {
            let mut keys = [false; 9];
            keys[index] = true;
            assert_eq!(mapped(keys).steer, 1.0);
        }
        for index in [6, 7] {
            let mut keys = [false; 9];
            keys[index] = true;
            assert_eq!(mapped(keys).steer, -1.0);
        }
    }

    #[test]
    fn opposing_direction_keys_cancel() {
        for forward in [0, 1] {
            for reverse in [2, 3] {
                let mut keys = [false; 9];
                keys[forward] = true;
                keys[reverse] = true;
                assert_eq!(mapped(keys).throttle, 0.0);
            }
        }
        for left in [4, 5] {
            for right in [6, 7] {
                let mut keys = [false; 9];
                keys[left] = true;
                keys[right] = true;
                assert_eq!(mapped(keys).steer, 0.0);
            }
        }
    }

    #[test]
    fn duplicate_bindings_are_clamped_not_summed() {
        assert_eq!(
            mapped([true, true, false, false, false, false, false, false, false]).throttle,
            1.0
        );
        assert_eq!(
            mapped([false, false, true, true, false, false, false, false, false]).throttle,
            -1.0
        );
        assert_eq!(
            mapped([false, false, false, false, true, true, false, false, false]).steer,
            1.0
        );
        assert_eq!(
            mapped([false, false, false, false, false, false, true, true, false]).steer,
            -1.0
        );
    }

    #[test]
    fn space_sets_brake_without_changing_axes() {
        assert_eq!(
            mapped([false, false, false, false, false, false, false, false, true]),
            PlayerInput {
                throttle: 0.0,
                steer: 0.0,
                brake: true,
            }
        );
        let input = mapped([true, false, false, false, true, false, false, false, true]);
        assert_eq!(input.throttle, 1.0);
        assert_eq!(input.steer, 1.0);
        assert!(input.brake);
    }

    fn simulate_speed(
        initial: f32,
        max_speed: f32,
        input: PlayerInput,
        dt: f32,
        duration: f32,
    ) -> f32 {
        let steps = (duration / dt).round() as usize;
        (0..steps).fold(initial, |speed, _| next_speed(speed, max_speed, input, dt))
    }

    #[test]
    fn acceleration_coasting_and_braking_have_distinct_responses() {
        let initial = 12.0;
        let accelerating = simulate_speed(
            initial,
            20.0,
            PlayerInput {
                throttle: 1.0,
                ..default()
            },
            1.0 / 60.0,
            0.5,
        );
        let coasting = simulate_speed(initial, 20.0, PlayerInput::default(), 1.0 / 60.0, 0.5);
        let braking = simulate_speed(
            initial,
            20.0,
            PlayerInput {
                brake: true,
                ..default()
            },
            1.0 / 60.0,
            0.5,
        );

        assert!(accelerating > initial);
        assert!(braking < coasting && coasting < initial);
    }

    #[test]
    fn service_braking_decelerates_monotonically() {
        let input = PlayerInput {
            brake: true,
            ..default()
        };
        let mut speed = 12.0;
        for _ in 0..120 {
            let previous = speed;
            speed = next_speed(speed, 20.0, input, 1.0 / 120.0);
            assert!(speed <= previous);
            assert!(speed >= 0.0);
        }
    }

    #[test]
    fn braking_is_progressive_but_stops_in_a_reasonable_time() {
        let input = PlayerInput {
            brake: true,
            ..default()
        };
        let after_tenth = simulate_speed(12.0, 20.0, input, 1.0 / 120.0, 0.1);
        assert!(after_tenth > 5.0, "braking was effectively instantaneous");

        let dt = 1.0 / 120.0;
        let mut speed = 12.0;
        let mut elapsed = 0.0;
        while speed != 0.0 && elapsed < 2.0 {
            speed = next_speed(speed, 20.0, input, dt);
            elapsed += dt;
        }
        assert!((1.5..=2.0).contains(&elapsed), "stop took {elapsed}s");
    }

    #[test]
    fn braking_has_sane_frame_rate_independence() {
        let input = PlayerInput {
            brake: true,
            ..default()
        };
        let at_30 = simulate_speed(12.0, 20.0, input, 1.0 / 30.0, 0.5);
        let at_60 = simulate_speed(12.0, 20.0, input, 1.0 / 60.0, 0.5);
        let at_120 = simulate_speed(12.0, 20.0, input, 1.0 / 120.0, 0.5);
        assert!((at_30 - at_60).abs() < 1e-4);
        assert!((at_60 - at_120).abs() < 1e-4);
    }

    #[test]
    fn brake_dominates_throttle_and_reverse_remains_capped() {
        let brake_and_throttle = next_speed(
            12.0,
            20.0,
            PlayerInput {
                throttle: 1.0,
                brake: true,
                ..default()
            },
            0.1,
        );
        let brake_only = next_speed(
            12.0,
            20.0,
            PlayerInput {
                brake: true,
                ..default()
            },
            0.1,
        );
        assert!((brake_and_throttle - brake_only).abs() < f32::EPSILON);

        let reverse = simulate_speed(
            0.0,
            20.0,
            PlayerInput {
                throttle: -1.0,
                ..default()
            },
            1.0 / 60.0,
            10.0,
        );
        assert!(reverse >= -10.0 && reverse < -9.9);
    }

    #[test]
    fn static_obstacle_impact_is_absolute_player_speed() {
        let player = player_velocity(0.0, -7.0);
        assert!((obstacle_impact_speed(player, None) - 7.0).abs() < 1e-5);
    }

    #[test]
    fn parked_player_rammed_by_traffic_has_traffic_impact() {
        let player = player_velocity(0.0, 0.0);
        let traffic = traffic_velocity(true, 1.0, 6.0);
        assert!((obstacle_impact_speed(player, Some(traffic)) - 6.0).abs() < 1e-5);
    }

    #[test]
    fn head_on_traffic_impact_sums_speeds() {
        let player = player_velocity(0.0, 8.0);
        let traffic = traffic_velocity(false, 1.0, 5.0);
        assert!((obstacle_impact_speed(player, Some(traffic)) - 13.0).abs() < 1e-5);
    }

    #[test]
    fn same_direction_traffic_impact_is_speed_difference() {
        let player = player_velocity(0.0, 8.0);
        let traffic = traffic_velocity(false, -1.0, 5.0);
        assert!((obstacle_impact_speed(player, Some(traffic)) - 3.0).abs() < 1e-5);
    }

    #[test]
    fn orthogonal_traffic_impact_uses_vector_relative_speed() {
        let player = player_velocity(0.0, 8.0);
        let traffic = traffic_velocity(true, 1.0, 6.0);
        assert!((obstacle_impact_speed(player, Some(traffic)) - 10.0).abs() < 1e-5);
    }
}
