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
use crate::world::{
    Collider, Cone, ConeMotion, ConeState, Curb, cone_hit_speed, cone_initial_lifetime,
    cone_launch_velocity, cone_spin_axis, cone_spin_rate,
};

#[derive(Component)]
pub struct Car {
    pub speed: f32,
    pub heading: f32,
    /// Arcade drift slip angle (radians). The car visually faces `heading`
    /// but travels along `heading + drift`. Built only while the handbrake
    /// is held with steering and forward speed; decays to zero otherwise.
    /// Hard-clamped to `±DRIFT_MAX` so it can never grow unbounded.
    pub drift: f32,
}

/// Freeze car input (and round-timer burn) while a countdown is active. Set
/// by T6's countdown plugin; `move_car` early-returns while this is true.
#[derive(Resource, Default)]
pub struct InputFrozen(pub bool);

/// Centralized player driving intent. Keyboard input populates this resource;
/// other input methods can write the same normalized controls later.
///
/// The handbrake (drift trigger) lives in a sibling [`Handbrake`] resource
/// rather than a field here so existing `PlayerInput` struct literals in other
/// modules stay source-compatible. It is populated and cleared alongside
/// this resource by `read_keyboard_input`.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Default)]
pub struct PlayerInput {
    /// Reverse (-1.0) through forward (1.0).
    pub throttle: f32,
    /// Right (-1.0) through left (1.0), matching the car's steering sign.
    pub steer: f32,
    /// Active braking. This takes precedence over throttle in `move_car`.
    pub brake: bool,
}

/// Handbrake (drift) intent, mapped from both Shift keys by
/// `read_keyboard_input`. Like `PlayerInput`, it is cleared while frozen or
/// outside `Playing`. The handbrake **never** acts as a service brake —
/// `next_speed` is unaware of it — so Shift alone never zeroes speed; it only
/// enables arcade drift (tighter turning + bounded lateral slip) in `move_car`.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Handbrake(pub bool);

/// The side a drift is latched to. Derived from the steer sign that first
/// breaks traction: steering left (steer > 0) latches [`Left`], steering
/// right (steer < 0) latches [`Right`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DriftSide {
    Left,
    Right,
}

/// Arcade drift side latch. Once the handbrake breaks rear traction with
/// steering, the drift side locks until the handbrake releases — even if the
/// player counter-steers or centers the wheel. This prevents mid-drift
/// direction flips and lets `move_car` apply a wide baseline steer so the car
/// holds its slide through the corner.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DriftLatch {
    side: Option<DriftSide>,
}

// Exponential speed-response rates (per second). Service braking is
// deliberately stronger than acceleration/coasting without snapping the car
// to a halt: from speed 12 it takes about 1.75 s to reach the stop threshold,
// leaving enough braking distance for the rear skid marks to read clearly.
const ACCEL_RESPONSE_RATE: f32 = 3.0;
const COAST_RESPONSE_RATE: f32 = 2.0;
const BRAKE_RESPONSE_RATE: f32 = 4.0;
const STOP_SPEED_THRESHOLD: f32 = 0.01;
/// Static-obstacle and relative-traffic impacts must exceed this speed to
/// damage the player. Collision pushout remains unconditional on contact.
const MIN_OBSTACLE_DAMAGE_SPEED: f32 = 5.0;

// Arcade handbrake drift tuning. Drift is a bounded slip angle between the
// car's facing (`heading`) and its travel direction (`heading + drift`). It
// only builds while the handbrake is held with steering and forward speed,
// recovers smoothly on release, and is hard-clamped so it can never grow
// unbounded. The handbrake never touches speed integration, so Shift alone
// never service-brakes / zeroes speed — it only widens turning and adds slip.
/// Peak slip angle (radians, ~28°). Hard clamp bound for `Car::drift`.
const DRIFT_MAX: f32 = 0.5;
/// Slip approach rate while drifting (1/s). Exponential, frame-rate
/// independent. Speed-scaled by `speed/max_speed` in `next_drift` so a
/// low-speed drift creeps in and a high-speed one snaps in.
const DRIFT_BUILD_RATE: f32 = 6.0;
/// Slip recovery rate on release (1/s). Exponential decay to zero.
const DRIFT_DECAY_RATE: f32 = 5.0;
/// Heading-change multiplier while drifting — a tighter turn radius.
const DRIFT_TURN_BOOST: f32 = 1.8;
/// Forward speed required to break rear traction and begin a drift.
const DRIFT_MIN_SPEED: f32 = 1.0;
/// Below this magnitude, recovering slip snaps to exactly zero.
const DRIFT_SNAP: f32 = 1e-4;
/// Baseline effective steer while latched and drifting with no active steering
/// input. The car holds a wide arc through the corner even with the wheel
/// centered; steering same-side tightens further, counter-steering clamps
/// back to this wide baseline.
const DRIFT_WIDE_STEER: f32 = 0.22;
/// Ignore analog noise and non-finite input until steering is deliberate.
const DRIFT_STEER_DEADZONE: f32 = 0.05;

// Pure player-car geometry shared by spawning and footprint tests. Keeping the
// wheel/chassis/fascia dimensions together makes it difficult for a cosmetic
// tweak to separate the running gear from the body again.
const CAR_RADIUS: f32 = 0.9;
const BODY_AXES: Vec3 = Vec3::new(0.5, 0.25, 1.0);
const BODY_CENTER_Y: f32 = 0.35;
const CHASSIS_WIDTH: f32 = 0.82;
const CHASSIS_HEIGHT: f32 = 0.16;
const CHASSIS_LENGTH: f32 = 1.55;
const CHASSIS_Y: f32 = 0.20;
const ROCKER_LENGTH: f32 = 1.02;
const WHEEL_RADIUS: f32 = 0.15;
const WHEEL_WIDTH: f32 = 0.18;
const WHEEL_X: f32 = 0.47;
const WHEEL_Y: f32 = 0.17;
const WHEEL_Z: f32 = 0.66;
const BUMPER_WIDTH: f32 = 0.94;
const BUMPER_DEPTH: f32 = 0.08;
const BUMPER_Z: f32 = 0.90;
const SHADOW_BODY_WIDTH: f32 = 1.02;
const SHADOW_BODY_LENGTH: f32 = 2.0;
const SHADOW_WHEEL_WIDTH: f32 = 0.28;
const SHADOW_WHEEL_LENGTH: f32 = 0.34;
const WHEEL_POSITIONS: [(f32, f32); 4] = [
    (WHEEL_X, WHEEL_Z),
    (-WHEEL_X, WHEEL_Z),
    (WHEEL_X, -WHEEL_Z),
    (-WHEEL_X, -WHEEL_Z),
];

#[cfg(test)]
const fn max_f32(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

#[cfg(test)]
const fn bumper_outer_z() -> f32 {
    BUMPER_Z + BUMPER_DEPTH * 0.5
}

/// Half-extents of everything that contacts or overhangs the ground plane.
#[cfg(test)]
const fn car_footprint_half_extents() -> (f32, f32) {
    (
        max_f32(BODY_AXES.x, WHEEL_X + WHEEL_WIDTH * 0.5),
        max_f32(BODY_AXES.z, WHEEL_Z + WHEEL_RADIUS),
    )
}

/// Aggregate half-extents of the central body patch plus wheel patches.
#[cfg(test)]
const fn shadow_footprint_half_extents() -> (f32, f32) {
    (
        max_f32(SHADOW_BODY_WIDTH * 0.5, WHEEL_X + SHADOW_WHEEL_WIDTH * 0.5),
        max_f32(
            SHADOW_BODY_LENGTH * 0.5,
            WHEEL_Z + SHADOW_WHEEL_LENGTH * 0.5,
        ),
    )
}

/// Pure speed integration shared by gameplay and tests. Exponential response
/// keeps the feel consistent across frame rates and makes braking progressively
/// ease toward rest rather than applying an abrupt fixed-speed cut.
///
/// The handbrake is deliberately **not** a parameter here: drift never feeds
/// back into speed, so Shift alone coasts/accelerates exactly as without it
/// and never service-brakes or zeroes speed.
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

/// Whether the car is actively drifting this frame: handbrake held, steering
/// non-zero, and forward speed above the breakaway threshold. Pure so the
/// emitter, the turn boost, and tests all agree on the exact same predicate.
/// Reverse is intentionally excluded so normal reverse semantics are preserved.
fn is_drifting(speed: f32, input: PlayerInput, handbrake: bool) -> bool {
    handbrake && input.steer.abs() > 0.0 && speed > DRIFT_MIN_SPEED
}

/// Pure, frame-rate-independent drift slip integration. While drifting, slip
/// approaches a bounded target opposite the steer direction (the nose swings
/// past the travel direction — the car steps out like a real handbrake drift).
/// Otherwise it decays exponentially to zero and snaps once negligible. A
/// final hard clamp guarantees slip can never grow unbounded regardless of the
/// incoming value, so tuning or caller mistakes cannot overshoot the bound.
///
/// Two guards prevent an "entry curvature reversal" — where a too-fast slip
/// build curves the travel direction (heading + drift) opposite the intended
/// turn the moment the handbrake is grabbed:
/// - The slip build rate is speed-scaled (`DRIFT_BUILD_RATE * speed/max_speed`),
///   so at low speed slip creeps in and at high speed it snaps in.
/// - The per-frame slip change is capped to half the per-frame heading delta.
///   During a build Δdrift is opposite Δheading, so |Δdrift| ≤ ½|Δheading|
///   keeps travel curvature co-directed with the turn (travel changes by at
///   least half the heading change, same sign). The cap only binds at entry;
///   once slip nears its target the exponential approach takes over, so
///   bounded steady slip is preserved.
fn next_drift(
    current: f32,
    speed: f32,
    input: PlayerInput,
    handbrake: bool,
    dt: f32,
    turn_rate: f32,
    max_speed: f32,
) -> f32 {
    if is_drifting(speed, input, handbrake) {
        // Target slip sign is opposite the steer sign: steering left (steer > 0)
        // drives drift negative, so travel = heading + drift lags the nose —
        // the rear steps out to the right of the corner.
        let steer = input.steer.clamp(-1.0, 1.0);
        let target = -steer * DRIFT_MAX;
        // Speed-scale the build: the rear tires scrub harder the faster the
        // car goes, so slip approaches its target quicker at speed and creeps
        // in near the breakaway threshold.
        let speed_frac = (speed / max_speed).clamp(0.0, 1.0);
        let rate = DRIFT_BUILD_RATE * speed_frac;
        let alpha = 1.0 - (-rate * dt.max(0.0)).exp();
        let proposed = current + (target - current) * alpha;

        // Reversal cap: limit |Δdrift| this frame to half the heading delta.
        // Travel = heading + drift; during a build Δdrift is opposite Δheading,
        // so this bound keeps travel from curving back through the corner.
        let heading_delta = steer * turn_rate * DRIFT_TURN_BOOST * dt * speed_frac;
        let max_change = 0.5 * heading_delta.abs();
        let change = (proposed - current).clamp(-max_change, max_change);
        (current + change).clamp(-DRIFT_MAX, DRIFT_MAX)
    } else {
        // Smooth recovery: exponential decay toward zero, then an exact snap
        // so residual slip cannot linger indefinitely on release.
        let decay = (-DRIFT_DECAY_RATE * dt.max(0.0)).exp();
        let d = current * decay;
        if d.abs() < DRIFT_SNAP { 0.0 } else { d }
    }
}

/// Pure latch state update. While the handbrake is held, the first non-zero
/// steer locks the drift side; that side persists through zero or opposing
/// steer until the handbrake releases, which unlocks immediately. A zero steer
/// with no existing lock stays unlocked.
fn next_drift_latch(current: DriftLatch, handbrake: bool, steer: f32) -> DriftLatch {
    if !handbrake {
        return DriftLatch::default();
    }
    if current.side.is_some() {
        return current;
    }
    // No lock yet: latch only on deliberate, finite steering.
    let steer = if steer.is_finite() { steer } else { 0.0 };
    let side = if steer > DRIFT_STEER_DEADZONE {
        Some(DriftSide::Left)
    } else if steer < -DRIFT_STEER_DEADZONE {
        Some(DriftSide::Right)
    } else {
        None
    };
    DriftLatch { side }
}

/// Effective steer while latched and drifting. The wide baseline keeps the car
/// turning into the locked corner even with the wheel centered or
/// counter-steered; steering same-side tightens beyond the wide baseline.
/// Outside a latched drift (no handbrake, not drifting, or unlocked) the raw
/// steer passes through unchanged. NaN steer collapses to the wide baseline.
fn latched_drift_steer(latch: DriftLatch, drifting: bool, handbrake: bool, steer: f32) -> f32 {
    let steer = if steer.is_finite() {
        steer.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    if !drifting || !handbrake {
        return steer;
    }
    let Some(side) = latch.side else {
        return steer;
    };
    let sign = match side {
        DriftSide::Left => 1.0,
        DriftSide::Right => -1.0,
    };
    let wide = DRIFT_WIDE_STEER * sign;
    if sign > 0.0 {
        steer.max(wide)
    } else {
        steer.min(wide)
    }
}

/// Clear residual slip that opposes a freshly latched drift side. A new Left
/// latch (negative slip target) zeroes any lingering positive slip, and vice
/// versa for Right, so the drift builds cleanly from the locked direction
/// without fighting stale opposite-side slip.
fn sanitize_slip_for_latch(slip: f32, side: DriftSide) -> f32 {
    match side {
        DriftSide::Left => {
            if slip <= 0.0 {
                slip
            } else {
                0.0
            }
        }
        DriftSide::Right => {
            if slip >= 0.0 {
                slip
            } else {
                0.0
            }
        }
    }
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

/// Map both Shift keys to a single handbrake flag. Either key triggers drift,
/// so a player using whichever Shift is convenient gets identical behavior.
/// Pure and tested independently of the keyboard resource.
fn map_handbrake(shift_left: bool, shift_right: bool) -> bool {
    shift_left || shift_right
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
            .init_resource::<Handbrake>()
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
                // then knock cones flying, then the juice systems read the
                // fresh speed.
                (
                    move_car,
                    physics_collisions,
                    cone_collisions,
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
            let p = sphere_position * (BODY_AXES / 0.5);
            let n = Vec3::new(
                p.x / BODY_AXES.x.powi(2),
                p.y / BODY_AXES.y.powi(2),
                p.z / BODY_AXES.z.powi(2),
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

    // Front and rear fascia overlap the ellipsoid; lower valances overlap both
    // fascia and chassis so the bumpers have no visible air gap beneath them.
    let fascia_mesh = meshes.add(Cuboid::new(0.84, 0.17, 0.10));
    let bumper_mesh = meshes.add(Cuboid::new(BUMPER_WIDTH, 0.075, BUMPER_DEPTH));
    let valance_mesh = meshes.add(Cuboid::new(0.78, 0.10, 0.16));
    let chassis_mesh = meshes.add(Cuboid::new(CHASSIS_WIDTH, CHASSIS_HEIGHT, CHASSIS_LENGTH));
    let rocker_mesh = meshes.add(Cuboid::new(0.10, 0.15, ROCKER_LENGTH));
    let axle_mesh = meshes.add(Cylinder::new(0.025, 0.88));
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

    // Wheels: cylinders with the axle along X, tire-black. Their inner sidewalls
    // overlap the chassis/axles slightly, while the tread remains clear of the
    // body shell. One shared hub mesh exposes a metallic cap on each outside.
    let wheel_mesh = meshes.add(Cylinder::new(WHEEL_RADIUS, WHEEL_WIDTH));
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

    // Composite fake shadow: a restrained central body patch plus four shared
    // tire-contact patches follows the actual footprint much more closely than
    // the old oversized rectangle. All assets are allocated once at startup.
    let body_shadow_mesh = meshes.add(
        Plane3d::default()
            .mesh()
            .size(SHADOW_BODY_WIDTH, SHADOW_BODY_LENGTH),
    );
    let wheel_shadow_mesh = meshes.add(
        Plane3d::default()
            .mesh()
            .size(SHADOW_WHEEL_WIDTH, SHADOW_WHEEL_LENGTH),
    );
    let body_shadow_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.0, 0.0, 0.0, 0.24),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 1.0,
        ..default()
    });
    let wheel_shadow_mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.0, 0.0, 0.0, 0.30),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 1.0,
        ..default()
    });

    commands
        .spawn((
            Transform::from_xyz(0.0, 0.0, 0.0),
            Visibility::default(),
            Car {
                speed: 0.0,
                heading: 0.0,
                drift: 0.0,
            },
            DriftLatch::default(),
        ))
        .with_children(|car| {
            // Painted body shell (car paint). Cabin + glass + lights nest
            // under it so the whole upper structure rolls together.
            car.spawn((
                Mesh3d(meshes.add(car_body_mesh())),
                MeshMaterial3d(textures.car_paint.clone()),
                Transform::from_xyz(0.0, BODY_CENTER_Y, 0.0),
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

                // Painted fascia, seated bumper, and subtle lower valance are
                // shared nose-to-tail. These remain body children so every
                // painted/lit upper detail follows visual pitch and roll.
                for z in [-0.88_f32, 0.88] {
                    let sign = z.signum();
                    body.spawn((
                        Mesh3d(fascia_mesh.clone()),
                        MeshMaterial3d(textures.car_paint.clone()),
                        Transform::from_xyz(0.0, -0.07, z),
                    ));
                    body.spawn((
                        Mesh3d(bumper_mesh.clone()),
                        MeshMaterial3d(trim_mat.clone()),
                        Transform::from_xyz(0.0, -0.14, sign * BUMPER_Z),
                    ));
                    body.spawn((
                        Mesh3d(valance_mesh.clone()),
                        MeshMaterial3d(trim_mat.clone()),
                        Transform::from_xyz(0.0, -0.19, sign * 0.84),
                    ));
                }
                // The recessed black grille marks the nose (front is -Z).
                body.spawn((
                    Mesh3d(grille_mesh.clone()),
                    MeshMaterial3d(grille_mat.clone()),
                    Transform::from_xyz(0.0, -0.07, -0.942),
                ));
                for x in [-0.27, 0.27] {
                    body.spawn((
                        Mesh3d(headlight_mesh.clone()),
                        MeshMaterial3d(headlight_mat.clone()),
                        Transform::from_xyz(x, -0.055, -0.941),
                    ));
                    body.spawn((
                        Mesh3d(brake_mesh.clone()),
                        MeshMaterial3d(brake_mat.clone()),
                        Transform::from_xyz(x, -0.055, 0.941),
                        BrakeLight,
                    ));
                }
            });

            // The dark running gear stays root-level: pitch/roll is visual body
            // motion only. Chassis and rockers overlap the wheel inner faces and
            // the lower valances, making one connected silhouette.
            car.spawn((
                Mesh3d(chassis_mesh.clone()),
                MeshMaterial3d(trim_mat.clone()),
                Transform::from_xyz(0.0, CHASSIS_Y, 0.0),
            ));
            for x in [-0.43, 0.43] {
                car.spawn((
                    Mesh3d(rocker_mesh.clone()),
                    MeshMaterial3d(trim_mat.clone()),
                    Transform::from_xyz(x, 0.27, 0.0),
                ));
            }
            for z in [-WHEEL_Z, WHEEL_Z] {
                car.spawn((
                    Mesh3d(axle_mesh.clone()),
                    MeshMaterial3d(trim_mat.clone()),
                    Transform::from_xyz(0.0, WHEEL_Y, z)
                        .with_rotation(Quat::from_rotation_z(FRAC_PI_2)),
                ));
            }

            // Wheels sit modestly inward/up and overlap the axle ends without
            // penetrating the smooth painted shell. Negative Z remains front.
            for &(x, z) in &WHEEL_POSITIONS {
                let mut wheel = car.spawn((
                    Mesh3d(wheel_mesh.clone()),
                    MeshMaterial3d(wheel_mat.clone()),
                    Transform::from_xyz(x, WHEEL_Y, z)
                        .with_rotation(Quat::from_rotation_z(FRAC_PI_2)),
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

            // Plane3d lies in XZ. Separate heights avoid coplanar alpha z-fight;
            // both stay high enough to avoid the ground plane's depth precision.
            car.spawn((
                Mesh3d(body_shadow_mesh.clone()),
                MeshMaterial3d(body_shadow_mat.clone()),
                Transform::from_xyz(0.0, 0.058, 0.0),
            ));
            for &(x, z) in &WHEEL_POSITIONS {
                car.spawn((
                    Mesh3d(wheel_shadow_mesh.clone()),
                    MeshMaterial3d(wheel_shadow_mat.clone()),
                    Transform::from_xyz(x, 0.062, z),
                ));
            }
        });
}

fn read_keyboard_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut input: ResMut<PlayerInput>,
    mut handbrake: ResMut<Handbrake>,
    state: Res<State<GameState>>,
    input_frozen: Res<InputFrozen>,
) {
    if *state.get() != GameState::Playing || input_frozen.0 {
        *input = PlayerInput::default();
        handbrake.0 = false;
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
    handbrake.0 = map_handbrake(
        keys.pressed(KeyCode::ShiftLeft),
        keys.pressed(KeyCode::ShiftRight),
    );
}

fn move_car(
    mut car: Query<(&mut Car, &mut Transform, &mut DriftLatch)>,
    input: Res<PlayerInput>,
    handbrake: Res<Handbrake>,
    cfg: Res<GameConfig>,
    time: Res<Time>,
    input_frozen: Res<InputFrozen>,
) {
    // Countdown / freeze gate: the car holds still (and the round timer stops
    // burning) while a countdown overlay is active.
    if input_frozen.0 {
        return;
    }
    let Ok((mut car, mut tf, mut latch)) = car.single_mut() else {
        return;
    };
    let dt = time.delta_secs();

    car.speed = next_speed(car.speed, cfg.max_speed, *input, dt);

    let raw_steer = if input.steer.is_finite() {
        input.steer.clamp(-1.0, 1.0)
    } else {
        0.0
    };

    // Arcade handbrake drift latch: lock the drift side on the first steer
    // while the handbrake is held, then hold it until release. A freshly
    // locked side clears any opposing residual slip so the drift builds
    // cleanly from the new direction.
    let prev_side = latch.side;
    *latch = next_drift_latch(*latch, handbrake.0, raw_steer);
    if prev_side.is_none() && latch.side.is_some() {
        if let Some(side) = latch.side {
            car.drift = sanitize_slip_for_latch(car.drift, side);
        }
    }

    // Effective steer: while latched and drifting, a wide baseline keeps the
    // car turning into the locked corner; same-side steer tightens, counter-
    // steer clamps back to wide. This also feeds drift integration so the
    // slip target follows the locked turn direction.
    let drifting = is_drifting(car.speed, *input, handbrake.0)
        || (handbrake.0 && latch.side.is_some() && car.speed > DRIFT_MIN_SPEED);
    let effective_steer = latched_drift_steer(*latch, drifting, handbrake.0, raw_steer);
    let drift_input = PlayerInput {
        steer: effective_steer,
        ..*input
    };
    car.drift = next_drift(
        car.drift,
        car.speed,
        drift_input,
        handbrake.0,
        dt,
        cfg.turn_rate,
        cfg.max_speed,
    );

    // Steering scales with speed so the car can't spin in place. While
    // drifting the handbrake breaks rear traction and lets the nose rotate
    // faster — a tighter turn radius — without changing speed integration.
    let turn_scale = if drifting { DRIFT_TURN_BOOST } else { 1.0 };
    car.heading += effective_steer * cfg.turn_rate * turn_scale * dt * (car.speed / cfg.max_speed);

    // Travel direction is the heading plus the drift slip angle; the body
    // still visually faces `heading` (set below), so the car slides through
    // corners while its nose points into the slide.
    let travel = car.heading + car.drift;
    let forward = Vec3::new(-travel.sin(), 0.0, -travel.cos());
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
    let spin_delta = car.speed.abs() * dt / WHEEL_RADIUS;
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
#[cfg(test)]
fn obstacle_impact_speed(player: Vec2, traffic: Option<Vec2>) -> f32 {
    (player - traffic.unwrap_or(Vec2::ZERO)).length()
}

/// Damage uses a strict boundary: a 5 u/s tap is harmless, while any impact
/// above it qualifies. Kept separate from overlap resolution so changing the
/// damage threshold can never disable collision pushout.
fn is_damaging_obstacle_impact(impact_speed: f32) -> bool {
    impact_speed > MIN_OBSTACLE_DAMAGE_SPEED
}

/// A solid contact evaluated from the player's pre-resolution position and
/// velocity. Keeping the immutable snapshot in every contact prevents an
/// earlier query item from changing the impact reported for a later one.
#[derive(Clone, Copy, Debug)]
struct SolidContact {
    normal: Vec2,
    penetration: f32,
    relative_velocity: Vec2,
    impact_speed: f32,
    /// Entity index: deterministic resolution order and equal-impact tie-break.
    tie_breaker: u32,
}

impl SolidContact {
    fn is_closing(self) -> bool {
        self.relative_velocity.dot(self.normal) < 0.0
    }
}

#[derive(Debug)]
struct CollisionOutcome {
    pushout: Vec2,
    stop_player: bool,
    strongest_hit: Option<SolidContact>,
}

/// Resolve a complete frame's contacts in stable entity order. Pushout is
/// accumulated for every overlap, while every closing overlap stops inward
/// player motion regardless of whether it clears the damage threshold. Only
/// the strongest damaging closing contact is retained; equal impacts use the
/// lower entity index so query iteration order can never affect the result.
fn collision_outcome(mut contacts: Vec<SolidContact>) -> CollisionOutcome {
    contacts.sort_by_key(|contact| contact.tie_breaker);

    let mut pushout = Vec2::ZERO;
    let mut stop_player = false;
    let mut strongest_hit: Option<SolidContact> = None;
    for contact in contacts {
        pushout += contact.normal * contact.penetration;
        if !contact.is_closing() {
            continue;
        }
        stop_player = true;
        if !contact.impact_speed.is_finite() || !is_damaging_obstacle_impact(contact.impact_speed) {
            continue;
        }
        let replace = match strongest_hit {
            None => true,
            Some(current) => {
                contact
                    .impact_speed
                    .total_cmp(&current.impact_speed)
                    .is_gt()
                    || (contact
                        .impact_speed
                        .total_cmp(&current.impact_speed)
                        .is_eq()
                        && contact.tie_breaker < current.tie_breaker)
            }
        };
        if replace {
            strongest_hit = Some(contact);
        }
    }

    CollisionOutcome {
        pushout,
        stop_player,
        strongest_hit,
    }
}

/// Circle-vs-AABB contact from immutable world-space positions.
fn solid_contact_geometry(player: Vec2, obstacle: Vec2, half_extents: Vec2) -> Option<(Vec2, f32)> {
    let delta = player - obstacle;
    let closest = delta.clamp(-half_extents, half_extents);
    let outside = delta - closest;
    let dist2 = outside.length_squared();
    if dist2 >= CAR_RADIUS * CAR_RADIUS {
        return None;
    }

    if dist2 > 1e-6 {
        let dist = dist2.sqrt();
        return Some((outside / dist, CAR_RADIUS - dist));
    }

    // Center inside the box: eject along the least-penetrated axis. Exact
    // corner ties consistently choose Z.
    let pen_x = half_extents.x - delta.x.abs();
    let pen_z = half_extents.y - delta.y.abs();
    if pen_x < pen_z {
        let sign = if delta.x >= 0.0 { 1.0 } else { -1.0 };
        Some((Vec2::new(sign, 0.0), pen_x + CAR_RADIUS))
    } else {
        let sign = if delta.y >= 0.0 { 1.0 } else { -1.0 };
        Some((Vec2::new(0.0, sign), pen_z + CAR_RADIUS))
    }
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
        (Entity, &Collider, &GlobalTransform, Option<&Traffic>),
        (With<Collider>, Without<Car>, Without<Curb>, Without<Cone>),
    >,
    time: Res<Time>,
    mut obstacle_hits: MessageWriter<ObstacleHit>,
) {
    let Ok((mut car, mut tf)) = car.single_mut() else {
        return;
    };
    let dt = time.delta_secs();
    // --- Ground height: hop up onto any raised curb it drives over. ---
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

    // --- Obstacle collision: snapshot, evaluate all contacts, then resolve. ---
    // Neither pushout nor a previous stop may alter another contact's impact.
    // This is especially important for moving traffic, whose relative impact
    // must use the player's velocity at the start of collision resolution.
    let player_pos = Vec2::new(tf.translation.x, tf.translation.z);
    let player_speed = car.speed;
    let player_vel = player_velocity(car.heading + car.drift, player_speed);
    let mut contacts = Vec::new();
    for (entity, collider, ot, traffic) in &obstacles {
        // Skip colliders whose GlobalTransform hasn't propagated yet (still
        // IDENTITY at the world origin). No real obstacle sits at the origin
        // (all are at |x| >= 6), so this filters the 1-frame stale transform
        // after chunk spawn that otherwise piles every collider onto the car.
        if *ot == GlobalTransform::IDENTITY {
            continue;
        }
        let opos = ot.translation();
        let obstacle_pos = Vec2::new(opos.x, opos.z);
        let Some((normal, penetration)) = solid_contact_geometry(
            player_pos,
            obstacle_pos,
            Vec2::new(collider.half_x, collider.half_z),
        ) else {
            continue;
        };
        let traffic_vel =
            traffic.map(|traffic| traffic_velocity(traffic.axis, traffic.dir, traffic.speed));
        contacts.push(SolidContact {
            normal,
            penetration,
            relative_velocity: player_vel - traffic_vel.unwrap_or(Vec2::ZERO),
            impact_speed: traffic_vel.map_or(player_speed.abs(), |velocity| {
                (player_vel - velocity).length()
            }),
            tie_breaker: entity.index().index(),
        });
    }

    let outcome = collision_outcome(contacts);
    let pushout = if outcome.pushout.is_finite() {
        outcome.pushout
    } else {
        Vec2::ZERO
    };
    tf.translation.x += pushout.x;
    tf.translation.z += pushout.y;
    if outcome.stop_player {
        // `Car` stores scalar longitudinal motion, so stopping it is the only
        // representation-safe way to remove every inward normal component.
        // This applies to harmless <=5 u/s wall contacts as well as damage.
        car.speed = 0.0;
    }
    if let Some(hit) = outcome.strongest_hit {
        obstacle_hits.write(ObstacleHit {
            impact_speed: hit.impact_speed,
        });
    }
}

/// Car-vs-traffic-cone collisions. An idle cone is knocked flying on its
/// existing entity (launch + tip + spin) with a modest car speed bleed —
/// never a concrete stop, never a pushout, and never a damaging `ObstacleHit`
/// (cones are harmless). Flying cones are skipped so they cannot re-hit the
/// car. Cones are excluded from `physics_collisions`' generic obstacle loop
/// (`Without<Cone>`), so this is the sole cone contact path. Runs in the
/// driving chain right after `physics_collisions` so it uses the post-pushout
/// car position.
fn cone_collisions(
    mut car: Query<(&mut Car, &Transform), (With<Car>, Without<Cone>)>,
    mut cones: Query<(&Collider, &GlobalTransform, &mut ConeMotion), (With<Cone>, Without<Car>)>,
) {
    let Ok((mut car, car_t)) = car.single_mut() else {
        return;
    };
    // Snapshot the player velocity once so every cone launched this frame
    // uses the same pre-bleed speed — launch results are then independent of
    // query iteration order (fully deterministic). The speed bleed is a
    // multiplicative scalar accumulated in `bled_speed` and written back once,
    // which is also order-independent. Travel direction includes any active
    // drift slip so cones launch the way the car is actually moving.
    let travel_angle = car.heading + car.drift;
    let player_vel = player_velocity(travel_angle, car.speed);
    let mut bled_speed = car.speed;
    for (collider, ct, mut motion) in &mut cones {
        if motion.state != ConeState::Idle {
            continue; // flying cones cannot re-hit the car
        }
        // Cones are block-root children -> use GlobalTransform for the world
        // position (the local Transform is relative to the block root). Skip
        // the one-frame stale IDENTITY right after spawn/recycle.
        if *ct == GlobalTransform::IDENTITY {
            continue;
        }
        let cpos = ct.translation();
        let dx = car_t.translation.x - cpos.x;
        let dz = car_t.translation.z - cpos.z;
        let closest_x = dx.clamp(-collider.half_x, collider.half_x);
        let closest_z = dz.clamp(-collider.half_z, collider.half_z);
        let px = dx - closest_x;
        let pz = dz - closest_z;
        let dist2 = px * px + pz * pz;
        if dist2 < CAR_RADIUS * CAR_RADIUS {
            // Contact normal pointing from the car toward the cone (the
            // direction the cone flies away). For a head-on hit this is the
            // car's forward direction; for a side clip it points outward.
            let normal = if dist2 > 1e-6 {
                let dist = dist2.sqrt();
                Vec2::new(-px / dist, -pz / dist)
            } else {
                // Centers coincide: launch along the car's travel direction.
                player_velocity(travel_angle, 1.0).normalize_or_zero()
            };
            // Launch the cone (bounded, deterministic) on its existing entity.
            motion.vel = cone_launch_velocity(player_vel, normal);
            motion.spin_axis = cone_spin_axis(normal);
            motion.spin = cone_spin_rate(player_vel);
            motion.lifetime = cone_initial_lifetime();
            motion.state = ConeState::Flying;
            // Modest speed bleed — cones are harmless: no stop, no pushout,
            // no ObstacleHit. Accumulated order-independently below.
            bled_speed = cone_hit_speed(bled_speed);
        }
    }
    if bled_speed != car.speed {
        car.speed = bled_speed;
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
    fn drift_latch_locks_first_side_until_handbrake_release() {
        let unlocked = DriftLatch::default();
        assert_eq!(next_drift_latch(unlocked, true, 0.0), unlocked);
        assert_eq!(next_drift_latch(unlocked, true, 0.01), unlocked);
        assert_eq!(next_drift_latch(unlocked, true, f32::NAN), unlocked);
        assert_eq!(next_drift_latch(unlocked, true, f32::INFINITY), unlocked);
        let left = next_drift_latch(unlocked, true, 1.0);
        assert_eq!(left.side, Some(DriftSide::Left));
        assert_eq!(next_drift_latch(left, true, 0.0), left);
        assert_eq!(next_drift_latch(left, true, -1.0), left);
        assert_eq!(next_drift_latch(left, false, -1.0), unlocked);
        let right = next_drift_latch(unlocked, true, -1.0);
        assert_eq!(right.side, Some(DriftSide::Right));
    }

    #[test]
    fn latched_drift_steer_tightens_same_side_and_widens_otherwise() {
        let left = DriftLatch {
            side: Some(DriftSide::Left),
        };
        let wide = latched_drift_steer(left, true, true, 0.0);
        assert_eq!(wide, DRIFT_WIDE_STEER);
        assert_eq!(latched_drift_steer(left, true, true, -1.0), wide);
        assert!(latched_drift_steer(left, true, true, 0.7) > wide);
        assert_eq!(latched_drift_steer(left, true, false, -0.6), -0.6);
        assert_eq!(latched_drift_steer(left, true, true, f32::NAN), wide);
    }

    #[test]
    fn newly_latched_side_clears_only_opposing_residual_slip() {
        assert_eq!(sanitize_slip_for_latch(-0.3, DriftSide::Left), -0.3);
        assert_eq!(sanitize_slip_for_latch(0.3, DriftSide::Left), 0.0);
        assert_eq!(sanitize_slip_for_latch(0.3, DriftSide::Right), 0.3);
        assert_eq!(sanitize_slip_for_latch(-0.3, DriftSide::Right), 0.0);
    }

    fn simulate_latched_sequence(dt: f32) -> (f32, f32) {
        let speed = 10.0;
        let max_speed = 12.0;
        let turn_rate = 2.5;
        let mut latch = DriftLatch::default();
        let mut heading = 0.0;
        let mut drift = 0.0;
        let phases = [(0.7, true), (0.0, true), (-0.8, true), (0.0, false)];
        for (raw_steer, handbrake) in phases {
            let steps = (0.4 / dt).round() as usize;
            for _ in 0..steps {
                let previous_side = latch.side;
                latch = next_drift_latch(latch, handbrake, raw_steer);
                if previous_side.is_none()
                    && let Some(side) = latch.side
                {
                    drift = sanitize_slip_for_latch(drift, side);
                }
                let active = handbrake && latch.side.is_some() && speed > DRIFT_MIN_SPEED;
                let steer = latched_drift_steer(latch, active, handbrake, raw_steer);
                let input = PlayerInput { steer, ..default() };
                let previous_travel = heading + drift;
                drift = next_drift(drift, speed, input, handbrake, dt, turn_rate, max_speed);
                let turn_scale = if active { DRIFT_TURN_BOOST } else { 1.0 };
                heading += steer * turn_rate * turn_scale * dt * (speed / max_speed);
                if handbrake {
                    assert!(
                        heading + drift > previous_travel,
                        "latched left drift must never straighten or reverse"
                    );
                }
            }
        }
        (heading, drift)
    }

    #[test]
    fn neutral_and_opposite_steer_never_reverse_latched_travel_curvature() {
        simulate_latched_sequence(1.0 / 60.0);
    }

    #[test]
    fn latched_drift_sequence_is_frame_rate_stable() {
        let at_30 = simulate_latched_sequence(1.0 / 30.0);
        let at_60 = simulate_latched_sequence(1.0 / 60.0);
        let at_120 = simulate_latched_sequence(1.0 / 120.0);
        assert!((at_30.0 - at_60.0).abs() < 1e-4);
        assert!((at_60.0 - at_120.0).abs() < 1e-4);
        assert!((at_30.1 - at_60.1).abs() < 2e-3);
        assert!((at_60.1 - at_120.1).abs() < 2e-3);
    }

    #[test]
    fn chassis_spans_both_axles_and_reaches_the_wheels() {
        let chassis_half_length = CHASSIS_LENGTH * 0.5;
        let chassis_half_width = CHASSIS_WIDTH * 0.5;
        let wheel_inner_x = WHEEL_X - WHEEL_WIDTH * 0.5;

        assert!(chassis_half_length >= WHEEL_Z);
        assert!(chassis_half_width >= wheel_inner_x);
        assert!((CHASSIS_Y - WHEEL_Y).abs() <= CHASSIS_HEIGHT * 0.5);
    }

    #[test]
    fn wheel_layout_is_symmetric_and_inside_bumper_footprint() {
        for &(x, z) in &WHEEL_POSITIONS {
            assert!(WHEEL_POSITIONS.contains(&(-x, z)));
            assert!(WHEEL_POSITIONS.contains(&(x, -z)));
            assert!(x.abs() <= BUMPER_WIDTH * 0.5 + f32::EPSILON);
            assert!(z.abs() + WHEEL_RADIUS <= bumper_outer_z());
        }

        // Preserve the driving convention: exactly two front wheels are -Z.
        assert_eq!(WHEEL_POSITIONS.iter().filter(|(_, z)| *z < 0.0).count(), 2);
    }

    #[test]
    fn bumpers_are_seated_close_to_the_ellipsoid_tips() {
        let bumper_inner_z = BUMPER_Z - BUMPER_DEPTH * 0.5;
        assert!(bumper_inner_z < BODY_AXES.z);
        assert!(bumper_outer_z() <= BODY_AXES.z);
        assert!(BODY_AXES.z - bumper_outer_z() <= 0.1);
        assert!(BUMPER_WIDTH <= BODY_AXES.x * 2.0);
        assert!(BODY_AXES.x * 2.0 - BUMPER_WIDTH <= 0.1);
    }

    #[test]
    fn composite_shadow_covers_footprint_without_excess_margin() {
        let car = car_footprint_half_extents();
        let shadow = shadow_footprint_half_extents();
        assert!(shadow.0 >= car.0 && shadow.1 >= car.1);
        assert!(shadow.0 - car.0 <= 0.1);
        assert!(shadow.1 - car.1 <= 0.1);
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
    fn obstacle_damage_threshold_is_strictly_above_five() {
        assert!(!is_damaging_obstacle_impact(5.0));
        let just_above_five = f32::from_bits(5.0_f32.to_bits() + 1);
        assert!(is_damaging_obstacle_impact(just_above_five));
    }

    #[test]
    fn subthreshold_closing_wall_contact_still_stops_player() {
        let outcome = collision_outcome(vec![SolidContact {
            normal: Vec2::Y,
            penetration: 0.2,
            relative_velocity: Vec2::new(0.0, -4.0),
            impact_speed: 4.0,
            tie_breaker: 7,
        }]);

        assert!(outcome.stop_player);
        assert!(outcome.strongest_hit.is_none());
        assert_eq!(outcome.pushout, Vec2::new(0.0, 0.2));
    }

    #[test]
    fn strongest_multi_hit_is_independent_of_contact_order() {
        let weak = SolidContact {
            normal: Vec2::X,
            penetration: 0.1,
            relative_velocity: Vec2::new(-7.0, 0.0),
            impact_speed: 7.0,
            tie_breaker: 20,
        };
        let strongest_high_tie = SolidContact {
            normal: Vec2::Y,
            penetration: 0.2,
            relative_velocity: Vec2::new(0.0, -11.0),
            impact_speed: 11.0,
            tie_breaker: 30,
        };
        let strongest_low_tie = SolidContact {
            tie_breaker: 10,
            ..strongest_high_tie
        };

        for contacts in [
            vec![weak, strongest_high_tie, strongest_low_tie],
            vec![strongest_low_tie, weak, strongest_high_tie],
            vec![strongest_high_tie, strongest_low_tie, weak],
        ] {
            let outcome = collision_outcome(contacts);
            let hit = outcome.strongest_hit.expect("a damaging contact");
            assert_eq!(hit.impact_speed, 11.0);
            assert_eq!(hit.tie_breaker, 10);
            assert!(outcome.stop_player);
            assert_eq!(outcome.pushout, Vec2::new(0.1, 0.4));
        }
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

    // --- Handbrake drift -------------------------------------------------
    // The drift model is pure and frame-rate independent, so these tests drive
    // `map_handbrake`, `is_drifting`, and `next_drift` directly plus a small
    // heading simulator that mirrors `move_car`'s turn logic.

    fn simulate_drift(
        initial: f32,
        speed: f32,
        input: PlayerInput,
        handbrake: bool,
        dt: f32,
        duration: f32,
        turn_rate: f32,
        max_speed: f32,
    ) -> f32 {
        let steps = (duration / dt).round() as usize;
        (0..steps).fold(initial, |d, _| {
            next_drift(d, speed, input, handbrake, dt, turn_rate, max_speed)
        })
    }

    /// Mirror `move_car`'s heading integration at a fixed speed so the tighter
    /// turn radius while drifting is testable without Bevy resources.
    fn simulate_heading(
        initial_heading: f32,
        speed: f32,
        input: PlayerInput,
        handbrake: bool,
        turn_rate: f32,
        max_speed: f32,
        dt: f32,
        steps: usize,
    ) -> f32 {
        let mut heading = initial_heading;
        let mut drift = 0.0;
        for _ in 0..steps {
            let drifting = is_drifting(speed, input, handbrake);
            drift = next_drift(drift, speed, input, handbrake, dt, turn_rate, max_speed);
            let scale = if drifting { DRIFT_TURN_BOOST } else { 1.0 };
            heading += input.steer.clamp(-1.0, 1.0) * turn_rate * scale * dt * (speed / max_speed);
        }
        heading
    }

    #[test]
    fn both_shift_keys_map_to_handbrake() {
        assert!(!map_handbrake(false, false));
        assert!(map_handbrake(true, false));
        assert!(map_handbrake(false, true));
        assert!(map_handbrake(true, true));
    }

    #[test]
    fn handbrake_alone_does_not_brake_or_zero_speed() {
        // The handbrake is invisible to `next_speed`: holding Shift with no
        // throttle/brake coasts exactly like releasing all input. It never
        // enters the service-brake branch, never zeroes speed, and builds no
        // slip without steering.
        let dt = 1.0 / 60.0;
        let max_speed = 12.0;
        let coast = PlayerInput::default();
        let mut speed_no_hb = 12.0;
        let mut speed_hb = 12.0;
        let mut drift = 0.0;
        for _ in 0..60 {
            speed_no_hb = next_speed(speed_no_hb, max_speed, coast, dt);
            speed_hb = next_speed(speed_hb, max_speed, coast, dt);
            drift = next_drift(drift, speed_hb, coast, true, dt, 2.5, max_speed);
        }
        assert!(
            (speed_no_hb - speed_hb).abs() < 1e-9,
            "handbrake must not change speed"
        );
        assert!(speed_hb > 0.0, "handbrake must not zero speed");
        assert_eq!(drift, 0.0, "handbrake without steering builds no slip");
        let braked = simulate_speed(
            12.0,
            max_speed,
            PlayerInput {
                brake: true,
                ..default()
            },
            dt,
            1.0,
        );
        assert!(
            speed_hb > braked,
            "handbrake must not brake like the service brake"
        );
    }

    #[test]
    fn handbrake_drift_turns_tighter_than_normal() {
        let max_speed = 12.0;
        let turn_rate = 2.5;
        let speed = 10.0;
        let dt = 1.0 / 60.0;
        let steps = 60; // 1 second
        let steer = PlayerInput {
            steer: 1.0,
            ..default()
        };
        let normal = simulate_heading(0.0, speed, steer, false, turn_rate, max_speed, dt, steps);
        let drift = simulate_heading(0.0, speed, steer, true, turn_rate, max_speed, dt, steps);
        assert!(normal.abs() > 0.0);
        // Constant speed + handbrake boosts every step, so the accumulated
        // heading is exactly DRIFT_TURN_BOOST times the normal turn.
        assert!((drift - normal * DRIFT_TURN_BOOST).abs() < 1e-4);
        assert!(drift.abs() > normal.abs());
    }

    #[test]
    fn drift_slip_is_bounded_and_cannot_grow_unbounded() {
        let steer = PlayerInput {
            steer: 1.0,
            ..default()
        };
        // Sustained drift: slip asymptotes to the target and never exceeds it.
        let mut drift = 0.0;
        for _ in 0..5_000 {
            drift = next_drift(drift, 10.0, steer, true, 1.0 / 60.0, 2.5, 12.0);
        }
        assert!(drift <= DRIFT_MAX);
        assert!(drift >= -DRIFT_MAX);
        assert!(
            drift < 0.0,
            "left steer swings the nose past travel (negative slip)"
        );
        assert!(drift.abs() > DRIFT_MAX * 0.95);
        // Hard-clamp safety net: an out-of-range incoming value is clamped, so
        // no tuning or caller mistake can grow slip unbounded.
        assert!(next_drift(100.0, 10.0, steer, true, 1.0 / 60.0, 2.5, 12.0) <= DRIFT_MAX);
        assert!(next_drift(-100.0, 10.0, steer, true, 1.0 / 60.0, 2.5, 12.0) >= -DRIFT_MAX);
    }

    #[test]
    fn drift_slip_sign_follows_steer_direction() {
        let left = PlayerInput {
            steer: 1.0,
            ..default()
        };
        let right = PlayerInput {
            steer: -1.0,
            ..default()
        };
        let mut d_left = 0.0;
        let mut d_right = 0.0;
        for _ in 0..300 {
            d_left = next_drift(d_left, 10.0, left, true, 1.0 / 60.0, 2.5, 12.0);
            d_right = next_drift(d_right, 10.0, right, true, 1.0 / 60.0, 2.5, 12.0);
        }
        assert!(d_left < 0.0, "left steer -> negative slip");
        assert!(d_right > 0.0, "right steer -> positive slip");
        assert!((d_left + d_right).abs() < 1e-4, "slip should be symmetric");
    }

    #[test]
    fn drift_slip_recovers_smoothly_on_release() {
        let released = PlayerInput::default();
        let mut drift = -DRIFT_MAX;
        let mut prev = drift.abs();
        let mut snapped = false;
        for _ in 0..300 {
            // 5 s at 60 fps is plenty for the exponential decay to snap.
            drift = next_drift(drift, 10.0, released, false, 1.0 / 60.0, 2.5, 12.0);
            assert!(
                drift.abs() <= prev + 1e-9,
                "slip must decay monotonically on release"
            );
            prev = drift.abs();
            if drift.abs() < 1e-6 {
                snapped = true;
            }
        }
        assert!(snapped, "slip should snap to zero after release");
        assert_eq!(drift, 0.0);
    }

    #[test]
    fn drift_dynamics_are_frame_rate_independent() {
        let steer = PlayerInput {
            steer: 1.0,
            ..default()
        };
        let at_30 = simulate_drift(0.0, 10.0, steer, true, 1.0 / 30.0, 0.5, 2.5, 12.0);
        let at_60 = simulate_drift(0.0, 10.0, steer, true, 1.0 / 60.0, 0.5, 2.5, 12.0);
        let at_120 = simulate_drift(0.0, 10.0, steer, true, 1.0 / 120.0, 0.5, 2.5, 12.0);
        assert!((at_30 - at_60).abs() < 1e-4);
        assert!((at_60 - at_120).abs() < 1e-4);
    }

    #[test]
    fn handbrake_without_steering_builds_no_slip() {
        let no_steer = PlayerInput {
            steer: 0.0,
            ..default()
        };
        assert!(!is_drifting(10.0, no_steer, true));
        let mut drift = 0.0;
        for _ in 0..300 {
            drift = next_drift(drift, 10.0, no_steer, true, 1.0 / 60.0, 2.5, 12.0);
        }
        assert_eq!(drift, 0.0, "handbrake without steering never builds slip");
    }

    #[test]
    fn handbrake_without_speed_builds_no_slip() {
        let steer = PlayerInput {
            steer: 1.0,
            ..default()
        };
        assert!(!is_drifting(0.0, steer, true));
        // Breakaway is strictly greater than DRIFT_MIN_SPEED, not >=.
        assert!(!is_drifting(DRIFT_MIN_SPEED, steer, true));
        assert!(!is_drifting(DRIFT_MIN_SPEED * 0.5, steer, true));
        // With no speed, pre-existing slip recovers instead of building.
        let before = -0.4;
        let after = next_drift(before, 0.0, steer, true, 1.0 / 60.0, 2.5, 12.0);
        assert!(after.abs() < before.abs());
        assert!((after - before * (-DRIFT_DECAY_RATE * (1.0 / 60.0)).exp()).abs() < 1e-9);
    }

    #[test]
    fn per_frame_slip_change_capped_to_half_heading_delta() {
        // The reversal guard limits |Δdrift| to ½|Δheading| every frame during
        // a build, so travel (= heading + drift) can never curve opposite the
        // turn at entry. Verified directly against the heading-step formula.
        let max_speed = 12.0;
        let turn_rate = 2.5;
        let dt = 1.0 / 60.0;
        let speed = 10.0;
        let input = PlayerInput {
            steer: 1.0,
            ..default()
        };
        let mut drift = 0.0;
        for _ in 0..60 {
            let heading_delta =
                input.steer * turn_rate * DRIFT_TURN_BOOST * dt * (speed / max_speed);
            let prev = drift;
            drift = next_drift(drift, speed, input, true, dt, turn_rate, max_speed);
            let slip_delta = (drift - prev).abs();
            assert!(
                slip_delta <= 0.5 * heading_delta.abs() + 1e-9,
                "slip change {slip_delta} exceeded half heading delta {}",
                0.5 * heading_delta.abs()
            );
        }
    }

    #[test]
    fn slip_build_is_speed_scaled() {
        // The build rate is multiplied by speed/max_speed, and the reversal
        // cap tracks the (speed-scaled) heading delta, so a low-speed drift
        // builds slip gentler than a high-speed drift from the very first
        // frame.
        let max_speed = 12.0;
        let turn_rate = 2.5;
        let dt = 1.0 / 60.0;
        let input = PlayerInput {
            steer: 1.0,
            ..default()
        };
        let low = next_drift(
            0.0,
            DRIFT_MIN_SPEED + 0.5,
            input,
            true,
            dt,
            turn_rate,
            max_speed,
        )
        .abs();
        let high = next_drift(0.0, max_speed, input, true, dt, turn_rate, max_speed).abs();
        assert!(low > 0.0 && high > 0.0);
        assert!(
            low < high,
            "low-speed slip should build slower: low={low} high={high}"
        );
    }

    #[test]
    fn travel_curvature_never_reverses_during_entry_across_speeds() {
        // Travel = heading + drift. During a drift entry Δdrift is opposite
        // Δheading, so without the per-frame cap travel could curve back
        // through the corner. The cap keeps travel co-directed with the steer
        // at every driving speed, from just above breakaway to top speed.
        let max_speed = 12.0;
        let turn_rate = 2.5;
        let dt = 1.0 / 60.0;
        let speeds = [DRIFT_MIN_SPEED + 0.5, 3.0, 6.0, 9.0, max_speed];
        for &speed in &speeds {
            for steer_sign in [1.0, -1.0] {
                let input = PlayerInput {
                    steer: steer_sign,
                    ..default()
                };
                let mut heading = 0.0;
                let mut drift = 0.0;
                let mut prev_travel = heading + drift;
                for step in 0..120 {
                    let drifting = is_drifting(speed, input, true);
                    drift = next_drift(drift, speed, input, true, dt, turn_rate, max_speed);
                    let scale = if drifting { DRIFT_TURN_BOOST } else { 1.0 };
                    heading += input.steer * turn_rate * scale * dt * (speed / max_speed);
                    let travel = heading + drift;
                    let travel_delta = travel - prev_travel;
                    assert!(
                        travel_delta * steer_sign > 0.0,
                        "travel reversed at speed {speed}, steer {steer_sign}, step {step}"
                    );
                    prev_travel = travel;
                }
            }
        }
    }

    #[test]
    fn steady_slip_remains_bounded_and_approaches_target_across_speeds() {
        // The reversal cap only binds at entry; once slip nears its target the
        // exponential approach takes over. So bounded steady slip is preserved
        // at every speed — slip asymptotes to the (capped) target and never
        // grows unbounded.
        let max_speed = 12.0;
        let turn_rate = 2.5;
        let input = PlayerInput {
            steer: 1.0,
            ..default()
        };
        for &speed in &[DRIFT_MIN_SPEED + 0.5, 6.0, max_speed] {
            let mut drift = 0.0;
            for _ in 0..10_000 {
                drift = next_drift(drift, speed, input, true, 1.0 / 60.0, turn_rate, max_speed);
            }
            assert!(drift.abs() <= DRIFT_MAX, "slip unbounded at speed {speed}");
            assert!(drift < 0.0, "left steer -> negative slip at speed {speed}");
            assert!(
                drift.abs() > DRIFT_MAX * 0.9,
                "slip should approach the target at speed {speed}, got {drift}"
            );
        }
    }
}
