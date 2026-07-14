use bevy::asset::RenderAssetUsages;
use bevy::color::LinearRgba;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
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

// Pure player-car geometry shared by spawning and geometry tests.
const CAR_RADIUS: f32 = 0.9;
const BODY_AXES: Vec3 = Vec3::new(0.5, 0.25, 1.0);
const BODY_CENTER_Y: f32 = 0.35;
// Iteration 9 retains the wheel centers but replaces the thin wheel-space
// annulus with a broad, body-rooted shoulder blister. Its complete perimeter
// returns tangentially to the ellipsoid; the outboard half is a rounded return,
// not an exposed lip, blade, or ridge.
const WHEEL_RADIUS: f32 = 0.18;
const WHEEL_WIDTH: f32 = 0.16;
const WHEEL_X: f32 = 0.49;
const WHEEL_Y: f32 = 0.18;
const WHEEL_Z: f32 = 0.65;
const FENDER_ROOT_X: f32 = 0.12;
const FENDER_WELD_INSET: f32 = 0.0025;
const FENDER_Z_HALF_SPAN: f32 = 0.21;
const FENDER_END_ROUND: f32 = 0.08;
const FENDER_BULGE: f32 = 0.25;
const FENDER_BULGE_RISE: f32 = -0.08;
const FENDER_Z_STEPS: usize = 20;
const FENDER_X_STEPS: usize = 12;
const FASCIA_LIGHT_X: f32 = 0.22;
const FASCIA_LIGHT_WIDTH: f32 = 0.12;
const FASCIA_LIGHT_HEIGHT: f32 = 0.07;
const FASCIA_LIGHT_Y: f32 = -0.015;
const FASCIA_GRILLE_WIDTH: f32 = 0.26;
const FASCIA_GRILLE_HEIGHT: f32 = 0.06;
const FASCIA_GRILLE_Y: f32 = FASCIA_LIGHT_Y;
const FASCIA_SURFACE_LIFT: f32 = 0.002;

// Greenhouse profiles are authored in the painted body's local
// space. A nearest-point ellipsoid weld and short analytic tangent keep the
// cowl/shoulder join compact and smooth. The rear remains a little narrower,
// with a rake deliberately distinct from the windscreen.
const GREENHOUSE_SILL_Y: f32 = 0.13;
const GREENHOUSE_ROOF_BASE_Y: f32 = 0.49;
const GREENHOUSE_ROOF_CENTER_CROWN: f32 = 0.095;
#[cfg(test)]
const GREENHOUSE_TOP_Y: f32 = GREENHOUSE_ROOF_BASE_Y + GREENHOUSE_ROOF_CENTER_CROWN;
const GREENHOUSE_FRONT_SILL_Z: f32 = -0.39;
const GREENHOUSE_FRONT_TOP_Z: f32 = -0.12;
const GREENHOUSE_REAR_SILL_Z: f32 = 0.55;
const GREENHOUSE_REAR_TOP_Z: f32 = 0.43;
const GREENHOUSE_FRONT_SILL_HALF_WIDTH: f32 = 0.39;
const GREENHOUSE_REAR_SILL_HALF_WIDTH: f32 = 0.35;
const GREENHOUSE_FRONT_TOP_HALF_WIDTH: f32 = 0.27;
const GREENHOUSE_REAR_TOP_HALF_WIDTH: f32 = 0.245;
#[cfg(test)]
const GREENHOUSE_TRANSITION_SEGMENTS: usize = 12;
const GREENHOUSE_TANGENT_LENGTH: f32 = 0.018;
const GREENHOUSE_WELD_INSET: f32 = 0.001;
// Keep only a narrow painted sill above the body weld. Lowering the sill
// without lowering the aperture would recreate the broad perimeter shelf.
const GREENHOUSE_WINDOW_BOTTOM_Y: f32 = 0.145;
const GREENHOUSE_WINDOW_TOP_Y: f32 = 0.465;
const GREENHOUSE_B_PILLAR_Z: f32 = 0.12;
const GREENHOUSE_B_PILLAR_HALF_WIDTH: f32 = 0.018;
const GREENHOUSE_CORNER_BAND: f32 = 0.08;
// Panes now sit well behind the painted aperture rather than nearly flush with
// it. The backing retreats another 22 mm, keeping either layer's cut edge out
// of every oblique sightline while retaining almost 90% of the aperture height.
const GREENHOUSE_GLASS_INSET: f32 = 0.018;
const GREENHOUSE_BACKING_INSET: f32 = 0.040;
// Preserve the accepted painted lower/upper seal dimensions independently of
// the deeper backing; iteration 9 changes containment, not frame geometry.
const GREENHOUSE_SEAL_BAND: f32 = 0.024;
const GREENHOUSE_SEAL_OVERLAP: f32 = 0.004;
// Glazing and backing also continue 25 mm beneath both faces of every painted
// corner pillar. This is independent from the face-normal depth: adjacent
// raked panes retreat in different directions and need a generous hidden plan
// overlap as well as depth to guarantee a painted containment margin.
const GREENHOUSE_CORNER_OVERLAP: f32 = 0.025;
const GREENHOUSE_GLASS_ROUGHNESS: f32 = 0.16;
const WHEEL_POSITIONS: [(f32, f32); 4] = [
    (WHEEL_X, WHEEL_Z),
    (-WHEEL_X, WHEEL_Z),
    (WHEEL_X, -WHEEL_Z),
    (-WHEEL_X, -WHEEL_Z),
];

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

/// Tags make the three coherent greenhouse layers explicit: a painted frame,
/// dielectric glazing, and a small dark volume visible through the glass.
#[derive(Component)]
struct GreenhouseFrame;
#[derive(Component)]
struct GreenhouseGlass;
#[derive(Component)]
struct GreenhouseInterior;

/// Update ordering shared by keyboard, touch, and car simulation. Touch input
/// augments the keyboard-populated [`PlayerInput`] before driving consumes it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct KeyboardInputSet;

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TouchInputSet;

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DrivingSet;

pub struct CarPlugin;

/// Spawn only the production car visual.  The opt-in review harness uses this
/// instead of `CarPlugin`, so it cannot install input, gameplay, collisions or
/// timer-dependent systems.
pub struct CarReviewPlugin;

impl Plugin for CarReviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_car);
    }
}

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
            let n = body_normal(p);
            (p.to_array(), n.to_array())
        })
        .unzip();
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh
}

/// Position on a broad shoulder cap. `along` runs nose-to-tail and `across`
/// runs from the upper-body root to the outer flank. The unmodified base spans
/// the ellipsoid all the way to its side silhouette. A separable sin² dome
/// pushes the middle outward and down around the tire shoulder, while value and
/// first derivative vanish on all four edges. Consequently the whole perimeter
/// is a tangent body weld and neither longitudinal end collapses to a point.
fn fender_point(side: f32, wheel_z: f32, along: f32, across: f32) -> Vec3 {
    let along = along.clamp(0.0, 1.0);
    let across = across.clamp(0.0, 1.0);
    let longitudinal_round = (std::f32::consts::PI * along).sin().powi(4);
    let half_span = FENDER_Z_HALF_SPAN
        - FENDER_END_ROUND * (1.0 - (std::f32::consts::PI * across).sin());
    let z = wheel_z + half_span * (along * 2.0 - 1.0);
    let side_limit = BODY_AXES.x * (1.0 - z.powi(2) / BODY_AXES.z.powi(2)).max(0.0).sqrt();
    let abs_x = FENDER_ROOT_X + (side_limit - FENDER_ROOT_X) * across;
    let surface = Vec3::new(side * abs_x, body_surface_y(side * abs_x, z), z);
    let base = surface - body_normal(surface) * FENDER_WELD_INSET;
    let lateral_round = (std::f32::consts::PI * across).sin();
    let direction = Vec3::new(side, FENDER_BULGE_RISE, 0.0).normalize();
    base + direction * (FENDER_BULGE * longitudinal_round * lateral_round)
}

/// Position and geometric normal on the rounded shoulder. Centered numerical
/// derivatives keep the normal tied to the actual multi-ring surface; at the
/// boundary one-sided derivatives converge to the analytic ellipsoid tangent
/// because the sin² displacement has zero slope there.
fn fender_point_normal(side: f32, wheel_z: f32, along: f32, across: f32) -> (Vec3, Vec3) {
    const H: f32 = 1e-3;
    let point = fender_point(side, wheel_z, along, across);
    if along <= f32::EPSILON
        || along >= 1.0 - f32::EPSILON
        || across <= f32::EPSILON
        || across >= 1.0 - f32::EPSILON
    {
        return (point, body_normal(point));
    }
    let along0 = (along - H).max(0.0);
    let along1 = (along + H).min(1.0);
    let across0 = (across - H).max(0.0);
    let across1 = (across + H).min(1.0);
    let dz =
        fender_point(side, wheel_z, along1, across) - fender_point(side, wheel_z, along0, across);
    let dx =
        fender_point(side, wheel_z, along, across1) - fender_point(side, wheel_z, along, across0);
    let mut normal = if side > 0.0 {
        dz.cross(dx)
    } else {
        dx.cross(dz)
    }
    .normalize();
    if normal.dot(Vec3::new(side, 1.0, 0.0)) < 0.0 {
        normal = -normal;
    }
    (point, normal)
}

/// One closed-perimeter shoulder patch per wheel. There is no inner/outer
/// annulus and no closure wall: every edge is already buried tangentially in
/// the body, while the dense two-dimensional surface reads as a rounded cap.
fn fender_mesh(side: f32, wheel_z: f32) -> Mesh {
    let mut mesh = GreenhouseMeshBuilder::default();
    for iz in 0..FENDER_Z_STEPS {
        for ix in 0..FENDER_X_STEPS {
            let z0 = iz as f32 / FENDER_Z_STEPS as f32;
            let z1 = (iz + 1) as f32 / FENDER_Z_STEPS as f32;
            let x0 = ix as f32 / FENDER_X_STEPS as f32;
            let x1 = (ix + 1) as f32 / FENDER_X_STEPS as f32;
            let (a, na) = fender_point_normal(side, wheel_z, z0, x0);
            let (b, nb) = fender_point_normal(side, wheel_z, z0, x1);
            let (c, nc) = fender_point_normal(side, wheel_z, z1, x1);
            let (d, nd) = fender_point_normal(side, wheel_z, z1, x0);
            mesh.quad_with_normals_outward([a, b, c, d], [na, nb, nc, nd]);
        }
    }
    mesh.finish()
}

/// Geometry/material role for one cached greenhouse layer. Frame includes the
/// tangent-matched lower transition, sill, corner pillars, B-pillars and roof;
/// glass is offset slightly to prevent coplanar flicker; interior is a closed
/// dark box. All three meshes and their materials are allocated only once by
/// `spawn_car`, then reused through cloned asset handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GreenhouseMeshPart {
    Frame,
    Glass,
    Interior,
}

/// Small indexed-mesh builder. Most vertices are duplicated per face for
/// crisp glazing/pillar breaks. The welded transition instead supplies shared
/// analytic-to-sill normals, keeping its paint sweep coherent with the rounded
/// body rather than producing a stack of hard slabs.
#[derive(Default)]
struct GreenhouseMeshBuilder {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

impl GreenhouseMeshBuilder {
    fn quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
        let normal = (b - a).cross(c - a).normalize_or_zero();
        debug_assert!(normal.length_squared() > 0.99, "degenerate greenhouse quad");
        let base = self.positions.len() as u32;
        self.positions
            .extend([a.to_array(), b.to_array(), c.to_array(), d.to_array()]);
        self.normals.extend([normal.to_array(); 4]);
        self.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    fn quad_with_normals(&mut self, points: [Vec3; 4], normals: [Vec3; 4]) {
        let geometric = (points[1] - points[0]).cross(points[2] - points[0]);
        debug_assert!(
            geometric.length_squared() > 1e-10,
            "degenerate greenhouse quad"
        );
        debug_assert!(
            geometric.dot(normals[0]) > 0.0,
            "greenhouse winding faces inward"
        );
        let base = self.positions.len() as u32;
        self.positions.extend(points.map(|point| point.to_array()));
        self.normals
            .extend(normals.map(|n| n.normalize().to_array()));
        self.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    fn triangle_with_normals_outward(&mut self, mut points: [Vec3; 3], mut normals: [Vec3; 3]) {
        // Endpoint-tapered curved surfaces intentionally collapse to a point.
        // Do not emit zero-area triangles at those poles.
        if (points[1] - points[0])
            .cross(points[2] - points[0])
            .length_squared()
            <= 1e-10
        {
            return;
        }
        let average = normals.into_iter().sum::<Vec3>().normalize_or_zero();
        if (points[1] - points[0])
            .cross(points[2] - points[0])
            .dot(average)
            < 0.0
        {
            points.swap(1, 2);
            normals.swap(1, 2);
        }
        let base = self.positions.len() as u32;
        self.positions.extend(points.map(|point| point.to_array()));
        self.normals
            .extend(normals.map(|normal| normal.normalize().to_array()));
        self.indices.extend([base, base + 1, base + 2]);
    }

    /// Add a smooth quad while selecting winding from its supplied outward
    /// normals. Useful for planar fender closure faces whose orientation changes
    /// by side/end and would otherwise duplicate fragile winding branches.
    fn quad_with_normals_outward(&mut self, points: [Vec3; 4], normals: [Vec3; 4]) {
        // Curved quads are not necessarily planar. Orient each triangle from
        // its own three vertex normals instead of choosing one winding from a
        // quad-wide average that can be wrong for the second half.
        self.triangle_with_normals_outward(
            [points[0], points[1], points[2]],
            [normals[0], normals[1], normals[2]],
        );
        self.triangle_with_normals_outward(
            [points[0], points[2], points[3]],
            [normals[0], normals[2], normals[3]],
        );
    }

    fn cuboid(&mut self, min: Vec3, max: Vec3) {
        let p000 = Vec3::new(min.x, min.y, min.z);
        let p001 = Vec3::new(min.x, min.y, max.z);
        let p010 = Vec3::new(min.x, max.y, min.z);
        let p011 = Vec3::new(min.x, max.y, max.z);
        let p100 = Vec3::new(max.x, min.y, min.z);
        let p101 = Vec3::new(max.x, min.y, max.z);
        let p110 = Vec3::new(max.x, max.y, min.z);
        let p111 = Vec3::new(max.x, max.y, max.z);
        self.quad(p000, p001, p011, p010); // -X
        self.quad(p100, p110, p111, p101); // +X
        self.quad(p000, p100, p101, p001); // -Y
        self.quad(p010, p011, p111, p110); // +Y
        self.quad(p000, p010, p110, p100); // -Z
        self.quad(p001, p101, p111, p011); // +Z
    }

    fn finish(self) -> Mesh {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals);
        mesh.insert_indices(Indices::U32(self.indices));
        mesh
    }
}

fn profile_lerp(y: f32, sill: f32, top: f32) -> f32 {
    let t =
        ((y - GREENHOUSE_SILL_Y) / (GREENHOUSE_ROOF_BASE_Y - GREENHOUSE_SILL_Y)).clamp(0.0, 1.0);
    sill + (top - sill) * t
}

fn front_z(y: f32) -> f32 {
    profile_lerp(y, GREENHOUSE_FRONT_SILL_Z, GREENHOUSE_FRONT_TOP_Z)
}

fn rear_z(y: f32) -> f32 {
    profile_lerp(y, GREENHOUSE_REAR_SILL_Z, GREENHOUSE_REAR_TOP_Z)
}

fn front_half_width(y: f32) -> f32 {
    profile_lerp(
        y,
        GREENHOUSE_FRONT_SILL_HALF_WIDTH,
        GREENHOUSE_FRONT_TOP_HALF_WIDTH,
    )
}

fn rear_half_width(y: f32) -> f32 {
    profile_lerp(
        y,
        GREENHOUSE_REAR_SILL_HALF_WIDTH,
        GREENHOUSE_REAR_TOP_HALF_WIDTH,
    )
}

fn side_x(y: f32, z: f32, side: f32) -> f32 {
    let t = ((z - front_z(y)) / (rear_z(y) - front_z(y))).clamp(0.0, 1.0);
    side * (front_half_width(y) + (rear_half_width(y) - front_half_width(y)) * t)
}

fn body_surface_y(x: f32, z: f32) -> f32 {
    let radial = 1.0 - x.powi(2) / BODY_AXES.x.powi(2) - z.powi(2) / BODY_AXES.z.powi(2);
    BODY_AXES.y * radial.max(0.0).sqrt()
}

fn body_normal(p: Vec3) -> Vec3 {
    Vec3::new(
        p.x / BODY_AXES.x.powi(2),
        p.y / BODY_AXES.y.powi(2),
        p.z / BODY_AXES.z.powi(2),
    )
    .normalize()
}

fn body_surface_z(x: f32, y: f32) -> f32 {
    BODY_AXES.z
        * (1.0 - x.powi(2) / BODY_AXES.x.powi(2) - y.powi(2) / BODY_AXES.y.powi(2))
            .max(0.0)
            .sqrt()
}

/// A gently tessellated lens/grille patch that follows the analytic body
/// ellipsoid.  `end` is -1 for the nose and +1 for the tail.  All vertices are
/// lifted only two millimetres along Z to avoid z-fighting while remaining far
/// inside the projected silhouette; smooth normals retain the nose curvature.
fn fascia_surface_mesh(end: f32, center: Vec2, size: Vec2) -> Mesh {
    const X_STEPS: usize = 4;
    const Y_STEPS: usize = 3;
    let mut mesh = GreenhouseMeshBuilder::default();
    let sample = |ix: usize, iy: usize| {
        let x = center.x + size.x * (ix as f32 / X_STEPS as f32 - 0.5);
        let y = center.y + size.y * (iy as f32 / Y_STEPS as f32 - 0.5);
        let z = end * (body_surface_z(x, y) + FASCIA_SURFACE_LIFT);
        let p = Vec3::new(x, y, z);
        (p, body_normal(Vec3::new(x, y, end * body_surface_z(x, y))))
    };
    for iy in 0..Y_STEPS {
        for ix in 0..X_STEPS {
            let (a, na) = sample(ix, iy);
            let (b, nb) = sample(ix + 1, iy);
            let (c, nc) = sample(ix + 1, iy + 1);
            let (d, nd) = sample(ix, iy + 1);
            if end < 0.0 {
                mesh.quad_with_normals([a, d, c, b], [na, nd, nc, nb]);
            } else {
                mesh.quad_with_normals([a, b, c, d], [na, nb, nc, nd]);
            }
        }
    }
    mesh.finish()
}

/// A point on one of the four sill perimeter edges. The ordering is chosen so
/// each transition strip can use the same outward winding convention.
fn sill_edge(edge: usize, u: f32) -> Vec3 {
    let u = u.clamp(0.0, 1.0);
    match edge {
        0 => Vec3::new(
            -GREENHOUSE_FRONT_SILL_HALF_WIDTH + 2.0 * GREENHOUSE_FRONT_SILL_HALF_WIDTH * u,
            GREENHOUSE_SILL_Y,
            GREENHOUSE_FRONT_SILL_Z,
        ),
        1 => Vec3::new(
            GREENHOUSE_FRONT_SILL_HALF_WIDTH
                + (GREENHOUSE_REAR_SILL_HALF_WIDTH - GREENHOUSE_FRONT_SILL_HALF_WIDTH) * u,
            GREENHOUSE_SILL_Y,
            GREENHOUSE_FRONT_SILL_Z + (GREENHOUSE_REAR_SILL_Z - GREENHOUSE_FRONT_SILL_Z) * u,
        ),
        2 => Vec3::new(
            GREENHOUSE_REAR_SILL_HALF_WIDTH - 2.0 * GREENHOUSE_REAR_SILL_HALF_WIDTH * u,
            GREENHOUSE_SILL_Y,
            GREENHOUSE_REAR_SILL_Z,
        ),
        _ => Vec3::new(
            -GREENHOUSE_REAR_SILL_HALF_WIDTH
                + (-GREENHOUSE_FRONT_SILL_HALF_WIDTH + GREENHOUSE_REAR_SILL_HALF_WIDTH) * u,
            GREENHOUSE_SILL_Y,
            GREENHOUSE_REAR_SILL_Z + (GREENHOUSE_FRONT_SILL_Z - GREENHOUSE_REAR_SILL_Z) * u,
        ),
    }
}

fn sill_edge_derivative(edge: usize) -> Vec3 {
    match edge {
        0 => Vec3::new(2.0 * GREENHOUSE_FRONT_SILL_HALF_WIDTH, 0.0, 0.0),
        1 => Vec3::new(
            GREENHOUSE_REAR_SILL_HALF_WIDTH - GREENHOUSE_FRONT_SILL_HALF_WIDTH,
            0.0,
            GREENHOUSE_REAR_SILL_Z - GREENHOUSE_FRONT_SILL_Z,
        ),
        2 => Vec3::new(-2.0 * GREENHOUSE_REAR_SILL_HALF_WIDTH, 0.0, 0.0),
        _ => Vec3::new(
            GREENHOUSE_REAR_SILL_HALF_WIDTH - GREENHOUSE_FRONT_SILL_HALF_WIDTH,
            0.0,
            GREENHOUSE_FRONT_SILL_Z - GREENHOUSE_REAR_SILL_Z,
        ),
    }
}

/// Euclidean nearest point on the upper ellipsoid and its exact derivative
/// along a sill edge. At the solution sill-surface is parallel to the normal;
/// unlike constant-height radial projection this produces a compact weld.
fn nearest_ellipsoid_projection(sill: Vec3, dsill: Vec3) -> (Vec3, Vec3, Vec3, Vec3) {
    let axes2 = BODY_AXES * BODY_AXES;
    let constraint = |lambda: f32| {
        axes2.x * sill.x.powi(2) / (lambda + axes2.x).powi(2)
            + axes2.y * sill.y.powi(2) / (lambda + axes2.y).powi(2)
            + axes2.z * sill.z.powi(2) / (lambda + axes2.z).powi(2)
    };
    let (mut low, mut high) = if constraint(0.0) < 1.0 {
        // The sill can lie inside the shell near edge midpoints. The upper,
        // closest branch is bracketed before the Y-axis pole.
        (-axes2.y + 1e-7, 0.0)
    } else {
        (0.0, 1.0)
    };
    while constraint(high) > 1.0 {
        high *= 2.0;
    }
    for _ in 0..48 {
        let mid = (low + high) * 0.5;
        if constraint(mid) > 1.0 {
            low = mid;
        } else {
            high = mid;
        }
    }
    let lambda = (low + high) * 0.5;
    let denom = axes2 + Vec3::splat(lambda);
    let surface = axes2 * sill / denom;
    let lambda_numerator = axes2.x * sill.x * dsill.x / denom.x.powi(2)
        + axes2.y * sill.y * dsill.y / denom.y.powi(2)
        + axes2.z * sill.z * dsill.z / denom.z.powi(2);
    let lambda_denominator = axes2.x * sill.x.powi(2) / denom.x.powi(3)
        + axes2.y * sill.y.powi(2) / denom.y.powi(3)
        + axes2.z * sill.z.powi(2) / denom.z.powi(3);
    let dlambda = lambda_numerator / lambda_denominator;
    let dsurface = axes2 * (dsill * denom - sill * dlambda) / (denom * denom);
    let gradient = surface / axes2;
    let normal = gradient.normalize();
    let dgradient = dsurface / axes2;
    let dnormal = (dgradient - normal * normal.dot(dgradient)) / gradient.length();
    (surface, dsurface, normal, dnormal)
}

/// Cubic nearest-point weld with exact parametric derivatives. Its generated
/// normal is the analytic cross product of the Hermite surface derivatives.
fn transition_point_normal(edge: usize, u: f32, t: f32) -> (Vec3, Vec3) {
    let sill = sill_edge(edge, u);
    let dsill = sill_edge_derivative(edge);
    let (surface, dsurface, n0, dn0) = nearest_ellipsoid_projection(sill, dsill);
    let p0 = surface - n0 * GREENHOUSE_WELD_INSET;
    let dp0 = dsurface - dn0 * GREENHOUSE_WELD_INSET;
    // Follow the downward direction projected into the ellipsoid tangent
    // plane. Most of the low Y=.13 sill is buried into the upper shell, so
    // this avoids the upward loop/perimeter shelf of the old high sill. The
    // tiny exterior corner spans remain smooth because the endpoint tangent
    // below uses their exact signed vertical displacement.
    let vertical_delta = GREENHOUSE_SILL_Y - p0.y;
    let tangent = -Vec3::Y + n0 * n0.y;
    let dtangent = dn0 * n0.y + n0 * dn0.y;
    let tangent_length = tangent.length();
    let unit_tangent = tangent / tangent_length;
    let dunit_tangent = (dtangent - unit_tangent * unit_tangent.dot(dtangent)) / tangent_length;
    let m0 = unit_tangent * GREENHOUSE_TANGENT_LENGTH;
    let dm0 = dunit_tangent * GREENHOUSE_TANGENT_LENGTH;
    let m1 = Vec3::Y * vertical_delta;
    let dm1 = -Vec3::Y * dp0.y;

    let t = t.clamp(0.0, 1.0);
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    let p = p0 * h00 + m0 * h10 + sill * h01 + m1 * h11;
    let _du = dp0 * h00 + dm0 * h10 + dsill * h01 + dm1 * h11;
    let _dt = p0 * (6.0 * t2 - 6.0 * t)
        + m0 * (3.0 * t2 - 4.0 * t + 1.0)
        + sill * (-6.0 * t2 + 6.0 * t)
        + m1 * (3.0 * t2 - 2.0 * t);
    // Exact perimeter corners have a singular derivative basis even though the
    // nearest-point weld is smooth. Interpolate shading from the analytic body
    // normal to the sill's radial frame normal to avoid a reflection flip.
    let frame_normal = Vec3::new(sill.x, 0.0, sill.z).normalize_or_zero();
    let smooth_t = t2 * (3.0 - 2.0 * t);
    (p, n0.lerp(frame_normal, smooth_t).normalize_or_zero())
}

fn add_side_patch(
    mesh: &mut GreenhouseMeshBuilder,
    side: f32,
    y0: f32,
    y1: f32,
    z0_at_y0: f32,
    z1_at_y0: f32,
    z0_at_y1: f32,
    z1_at_y1: f32,
    offset: f32,
) {
    let a = Vec3::new(side_x(y0, z0_at_y0, side), y0, z0_at_y0);
    let b = Vec3::new(side_x(y0, z1_at_y0, side), y0, z1_at_y0);
    let c = Vec3::new(side_x(y1, z1_at_y1, side), y1, z1_at_y1);
    let d = Vec3::new(side_x(y1, z0_at_y1, side), y1, z0_at_y1);
    // Offset the complete pane along its own raked face normal, rather than
    // along a world axis, so every edge remains parallel to its backing.
    let mut points = if side > 0.0 {
        [d, c, b, a]
    } else {
        [a, b, c, d]
    };
    let normal = (points[1] - points[0])
        .cross(points[2] - points[0])
        .normalize();
    for point in &mut points {
        *point += normal * offset;
    }
    mesh.quad(points[0], points[1], points[2], points[3]);
}

fn add_end_patch(
    mesh: &mut GreenhouseMeshBuilder,
    front: bool,
    y0: f32,
    y1: f32,
    x0_at_y0: f32,
    x1_at_y0: f32,
    x0_at_y1: f32,
    x1_at_y1: f32,
    offset: f32,
) {
    let z0 = if front { front_z(y0) } else { rear_z(y0) };
    let z1 = if front { front_z(y1) } else { rear_z(y1) };
    let a = Vec3::new(x0_at_y0, y0, z0);
    let b = Vec3::new(x1_at_y0, y0, z0);
    let c = Vec3::new(x1_at_y1, y1, z1);
    let d = Vec3::new(x0_at_y1, y1, z1);
    let mut points = if front { [d, c, b, a] } else { [a, b, c, d] };
    let normal = (points[1] - points[0])
        .cross(points[2] - points[0])
        .normalize();
    for point in &mut points {
        *point += normal * offset;
    }
    mesh.quad(points[0], points[1], points[2], points[3]);
}

/// Point and smooth shading normal on a front/rear header. Four Hermite rings
/// turn the raked end surface into the roof rather than bridging them with one
/// flat span. The lower derivative follows the end profile, while the upper
/// derivative lies in the roof's longitudinal tangent plane. Consequently the
/// first ring joins the lower header smoothly and the final ring copies the
/// exact position and normal returned by `roof_sample`.
fn end_header_sample(front: bool, y0: f32, u: f32, t: f32) -> (Vec3, Vec3) {
    let v = if front { 0.0 } else { 1.0 };
    let x_unit = u * 2.0 - 1.0;
    let lower_half = if front {
        front_half_width(y0)
    } else {
        rear_half_width(y0)
    };
    let lower_z = if front { front_z(y0) } else { rear_z(y0) };
    let p0 = Vec3::new(x_unit * lower_half, y0, lower_z);
    let (p1, roof_du, roof_dv) = roof_sample(u, v);

    let profile_height = GREENHOUSE_ROOF_BASE_Y - GREENHOUSE_SILL_Y;
    let dhalf_dy = if front {
        (GREENHOUSE_FRONT_TOP_HALF_WIDTH - GREENHOUSE_FRONT_SILL_HALF_WIDTH) / profile_height
    } else {
        (GREENHOUSE_REAR_TOP_HALF_WIDTH - GREENHOUSE_REAR_SILL_HALF_WIDTH) / profile_height
    };
    let dz_dy = if front {
        (GREENHOUSE_FRONT_TOP_Z - GREENHOUSE_FRONT_SILL_Z) / profile_height
    } else {
        (GREENHOUSE_REAR_TOP_Z - GREENHOUSE_REAR_SILL_Z) / profile_height
    };
    let end_du = Vec3::new(2.0 * lower_half, 0.0, 0.0);
    let end_dy = Vec3::new(x_unit * dhalf_dy, 1.0, dz_dy);
    let lower_normal = if front {
        end_dy.cross(end_du)
    } else {
        end_du.cross(end_dy)
    }
    .normalize();
    let roof_normal = roof_dv.cross(roof_du).normalize();

    let span = p0.distance(p1);
    let m0 = end_dy.normalize() * span;
    // Approaching the roof follows its longitudinal tangent toward the cabin.
    // The rear transition parameter approaches its seam in the opposite
    // geometric direction, hence its sign differs from the front.
    let roof_direction = if front { roof_dv } else { -roof_dv };
    let m1 = roof_direction.normalize() * span;
    let t = t.clamp(0.0, 1.0);
    let t2 = t * t;
    let t3 = t2 * t;
    let point = p0 * (2.0 * t3 - 3.0 * t2 + 1.0)
        + m0 * (t3 - 2.0 * t2 + t)
        + p1 * (-2.0 * t3 + 3.0 * t2)
        + m1 * (t3 - t2);
    let smooth_t = t2 * (3.0 - 2.0 * t);
    let normal = lower_normal.lerp(roof_normal, smooth_t).normalize();
    (point, normal)
}

/// Four-ring curved/beveled transition between each end header and the crown.
fn add_end_header(mesh: &mut GreenhouseMeshBuilder, front: bool, y0: f32) {
    const HEADER_STEPS: usize = 10;
    const HEADER_RINGS: usize = 4;
    for ix in 0..HEADER_STEPS {
        for ring in 0..HEADER_RINGS {
            let u0 = ix as f32 / HEADER_STEPS as f32;
            let u1 = (ix + 1) as f32 / HEADER_STEPS as f32;
            let t0 = ring as f32 / HEADER_RINGS as f32;
            let t1 = (ring + 1) as f32 / HEADER_RINGS as f32;
            let (a, na) = end_header_sample(front, y0, u0, t0);
            let (b, nb) = end_header_sample(front, y0, u1, t0);
            let (c, nc) = end_header_sample(front, y0, u1, t1);
            let (d, nd) = end_header_sample(front, y0, u0, t1);
            mesh.quad_with_normals_outward([a, b, c, d], [na, nb, nc, nd]);
        }
    }
}

/// Cross-car crowned roof and exact derivatives. The `sin²(u)` crown reaches
/// both front/rear boundaries, where tessellated headers share these samples;
/// only the side rails return to roof-base height.
fn roof_sample(u: f32, v: f32) -> (Vec3, Vec3, Vec3) {
    let half = GREENHOUSE_FRONT_TOP_HALF_WIDTH
        + (GREENHOUSE_REAR_TOP_HALF_WIDTH - GREENHOUSE_FRONT_TOP_HALF_WIDTH) * v;
    let x_unit = u * 2.0 - 1.0;
    let su = (std::f32::consts::PI * u).sin();
    let cu = (std::f32::consts::PI * u).cos();
    let crown = GREENHOUSE_ROOF_CENTER_CROWN * su.powi(2);
    let dz = GREENHOUSE_REAR_TOP_Z - GREENHOUSE_FRONT_TOP_Z;
    let dhalf = GREENHOUSE_REAR_TOP_HALF_WIDTH - GREENHOUSE_FRONT_TOP_HALF_WIDTH;
    let point = Vec3::new(
        x_unit * half,
        GREENHOUSE_ROOF_BASE_Y + crown,
        GREENHOUSE_FRONT_TOP_Z + dz * v,
    );
    let du = Vec3::new(
        2.0 * half,
        2.0 * std::f32::consts::PI * GREENHOUSE_ROOF_CENTER_CROWN * su * cu,
        0.0,
    );
    let dv = Vec3::new(x_unit * dhalf, 0.0, dz);
    (point, du, dv)
}

fn greenhouse_frame_mesh() -> Mesh {
    let mut mesh = GreenhouseMeshBuilder::default();
    let sill_top = GREENHOUSE_WINDOW_BOTTOM_Y;
    let glass_top = GREENHOUSE_WINDOW_TOP_Y;
    let roof_base = GREENHOUSE_ROOF_BASE_Y;
    let split_half = GREENHOUSE_B_PILLAR_HALF_WIDTH;
    let corner_band = GREENHOUSE_CORNER_BAND;

    // A finely sampled, continuous weld from the body surface to the sill.
    // The first ring is an ellipsoid profile and receives exactly its analytic
    // normals; Hermite tangents prevent the angular collar of iteration 2.
    const EDGE_STEPS: usize = 8;
    const RING_STEPS: usize = 4;
    for edge in 0..4 {
        for iu in 0..EDGE_STEPS {
            for it in 0..RING_STEPS {
                let u0 = iu as f32 / EDGE_STEPS as f32;
                let u1 = (iu + 1) as f32 / EDGE_STEPS as f32;
                let t0 = it as f32 / RING_STEPS as f32;
                let t1 = (it + 1) as f32 / RING_STEPS as f32;
                let (p00, n00) = transition_point_normal(edge, u0, t0);
                let (p10, n10) = transition_point_normal(edge, u1, t0);
                let (p01, n01) = transition_point_normal(edge, u0, t1);
                let (p11, n11) = transition_point_normal(edge, u1, t1);
                // This Hermite transition cell is non-planar. Orient its two
                // triangles independently so both halves follow the analytic
                // outward normals.
                mesh.triangle_with_normals_outward([p10, p00, p01], [n10, n00, n01]);
                mesh.triangle_with_normals_outward([p10, p01, p11], [n10, n01, n11]);
            }
        }
    }

    // Close the short painted sill belt between the smooth transition and all
    // four glazing/pillar surfaces.  Its overlaps are buried behind the corner
    // pillars, eliminating the tiny dark corner wedges of the prior iteration.
    for side in [-1.0, 1.0] {
        add_side_patch(
            &mut mesh,
            side,
            GREENHOUSE_SILL_Y,
            sill_top,
            GREENHOUSE_FRONT_SILL_Z,
            GREENHOUSE_REAR_SILL_Z,
            front_z(sill_top),
            rear_z(sill_top),
            0.0,
        );
    }
    for front in [true, false] {
        let low_half = if front {
            GREENHOUSE_FRONT_SILL_HALF_WIDTH
        } else {
            GREENHOUSE_REAR_SILL_HALF_WIDTH
        };
        let high_half = if front {
            front_half_width(sill_top)
        } else {
            rear_half_width(sill_top)
        };
        add_end_patch(
            &mut mesh,
            front,
            GREENHOUSE_SILL_Y,
            sill_top,
            -low_half,
            low_half,
            -high_half,
            high_half,
            0.0,
        );
    }

    // A-, B- and C-pillars on both inward-sloping side surfaces.
    for side in [-1.0, 1.0] {
        add_side_patch(
            &mut mesh,
            side,
            sill_top,
            glass_top,
            front_z(sill_top),
            front_z(sill_top) + corner_band,
            front_z(glass_top),
            front_z(glass_top) + corner_band,
            0.0,
        );
        add_side_patch(
            &mut mesh,
            side,
            sill_top,
            glass_top,
            GREENHOUSE_B_PILLAR_Z - split_half,
            GREENHOUSE_B_PILLAR_Z + split_half,
            GREENHOUSE_B_PILLAR_Z - split_half,
            GREENHOUSE_B_PILLAR_Z + split_half,
            0.0,
        );
        add_side_patch(
            &mut mesh,
            side,
            sill_top,
            glass_top,
            rear_z(sill_top) - corner_band,
            rear_z(sill_top),
            rear_z(glass_top) - corner_band,
            rear_z(glass_top),
            0.0,
        );
        // The roof boundary meets roof_base exactly behind this header.
        add_side_patch(
            &mut mesh,
            side,
            glass_top,
            roof_base,
            front_z(glass_top),
            rear_z(glass_top),
            GREENHOUSE_FRONT_TOP_Z,
            GREENHOUSE_REAR_TOP_Z,
            0.0,
        );
    }

    // Front/rear corner pillars and the short header below the roof.
    for front in [true, false] {
        let low_half = if front {
            front_half_width(sill_top)
        } else {
            rear_half_width(sill_top)
        };
        let high_half = if front {
            front_half_width(glass_top)
        } else {
            rear_half_width(glass_top)
        };
        for side in [-1.0, 1.0] {
            let low_outer = side * low_half;
            let low_inner = side * (low_half - corner_band);
            let high_outer = side * high_half;
            let high_inner = side * (high_half - corner_band);
            if side > 0.0 {
                add_end_patch(
                    &mut mesh, front, sill_top, glass_top, low_inner, low_outer, high_inner,
                    high_outer, 0.0,
                );
            } else {
                add_end_patch(
                    &mut mesh, front, sill_top, glass_top, low_outer, low_inner, high_outer,
                    high_inner, 0.0,
                );
            }
        }
        // Four curved rings turn the lower end face into every exact
        // `roof_sample(u, 0/1)` crown position and normal, avoiding both a
        // flat cross-car strip and a shading seam at the roof boundary.
        add_end_header(&mut mesh, front, glass_top);
    }

    // Continuous painted lower and upper seals overlap both pane and backing
    // boundaries on all four faces. Their own-face offset puts paint just
    // outside the glazing without axis-dependent gaps at the raked corners.
    let seal_lower_y0 = GREENHOUSE_WINDOW_BOTTOM_Y;
    let seal_lower_y1 = GREENHOUSE_WINDOW_BOTTOM_Y + GREENHOUSE_SEAL_BAND + GREENHOUSE_SEAL_OVERLAP;
    let seal_upper_y0 = GREENHOUSE_WINDOW_TOP_Y - GREENHOUSE_SEAL_BAND - GREENHOUSE_SEAL_OVERLAP;
    let seal_upper_y1 = GREENHOUSE_WINDOW_TOP_Y;
    for (y0, y1) in [
        (seal_lower_y0, seal_lower_y1),
        (seal_upper_y0, seal_upper_y1),
    ] {
        for side in [-1.0, 1.0] {
            add_side_patch(
                &mut mesh,
                side,
                y0,
                y1,
                front_z(y0),
                rear_z(y0),
                front_z(y1),
                rear_z(y1),
                0.001,
            );
        }
        for front in [true, false] {
            let low_half = if front {
                front_half_width(y0)
            } else {
                rear_half_width(y0)
            };
            let high_half = if front {
                front_half_width(y1)
            } else {
                rear_half_width(y1)
            };
            add_end_patch(
                &mut mesh, front, y0, y1, -low_half, low_half, -high_half, high_half, 0.001,
            );
        }
    }

    // A flush cross-car crown continues unchanged through both end headers.
    // Eleven cross-car and nine longitudinal samples make the silhouette
    // genuinely round. Duplicate grid vertices share analytic normals.
    const Z_RINGS: usize = 9;
    const X_RINGS: usize = 11;
    for iz in 0..Z_RINGS - 1 {
        for ix in 0..X_RINGS - 1 {
            let sample = |iz: usize, ix: usize| {
                let v = iz as f32 / (Z_RINGS - 1) as f32;
                let u = ix as f32 / (X_RINGS - 1) as f32;
                let (point, du, dv) = roof_sample(u, v);
                (point, dv.cross(du).normalize())
            };
            let (a, na) = sample(iz, ix);
            let (b, nb) = sample(iz + 1, ix);
            let (c, nc) = sample(iz + 1, ix + 1);
            let (d, nd) = sample(iz, ix + 1);
            mesh.quad_with_normals_outward([a, b, c, d], [na, nb, nc, nd]);
        }
    }
    mesh.finish()
}

fn greenhouse_glass_mesh() -> Mesh {
    let mut mesh = GreenhouseMeshBuilder::default();
    let y0 = GREENHOUSE_WINDOW_BOTTOM_Y + GREENHOUSE_GLASS_INSET;
    let y1 = GREENHOUSE_WINDOW_TOP_Y - GREENHOUSE_GLASS_INSET;
    let corner_gap = GREENHOUSE_CORNER_BAND - GREENHOUSE_CORNER_OVERLAP;
    let pillar_gap = GREENHOUSE_B_PILLAR_HALF_WIDTH + GREENHOUSE_SEAL_OVERLAP;
    // The inset keeps glazing well behind the paint. At corners both this pane
    // and its adjacent end pane continue 25 mm beneath the pillar, independently
    // of their differing face-normal offsets, so no viewing angle sees a slot.
    let surface_offset = -GREENHOUSE_GLASS_INSET;

    // Two side panes per side share the exact same raked envelope and leave a
    // real painted B-pillar between them.
    for side in [-1.0, 1.0] {
        add_side_patch(
            &mut mesh,
            side,
            y0,
            y1,
            front_z(y0) + corner_gap,
            GREENHOUSE_B_PILLAR_Z - pillar_gap,
            front_z(y1) + corner_gap,
            GREENHOUSE_B_PILLAR_Z - pillar_gap,
            surface_offset,
        );
        add_side_patch(
            &mut mesh,
            side,
            y0,
            y1,
            GREENHOUSE_B_PILLAR_Z + pillar_gap,
            rear_z(y0) - corner_gap,
            GREENHOUSE_B_PILLAR_Z + pillar_gap,
            rear_z(y1) - corner_gap,
            surface_offset,
        );
    }

    // Front/rear panes are coherent trapezoids sharing the same side taper and
    // fore/aft rake as the frame. Each extends beneath both painted corner
    // pillars before its own face-normal inset is applied.
    for front in [true, false] {
        let x0 = (if front {
            front_half_width(y0)
        } else {
            rear_half_width(y0)
        }) - corner_gap;
        let x1 = (if front {
            front_half_width(y1)
        } else {
            rear_half_width(y1)
        }) - corner_gap;
        add_end_patch(&mut mesh, front, y0, y1, -x0, x0, -x1, x1, surface_offset);
    }
    mesh.finish()
}

/// Opaque glazing still needs a complete dark backing to read as a cabin from
/// every review angle. This outer backing follows every pane, offset inward;
/// the closed inner box below fills any oblique gaps between pane backs.
fn greenhouse_glass_backing_mesh() -> Mesh {
    let mut mesh = GreenhouseMeshBuilder::default();
    // The backing has tighter vertical bounds and a substantially deeper
    // face-normal inset. Oblique views therefore hit paint or glass before
    // dark backing and cannot reveal wedges.
    let y0 = GREENHOUSE_WINDOW_BOTTOM_Y + GREENHOUSE_BACKING_INSET;
    let y1 = GREENHOUSE_WINDOW_TOP_Y - GREENHOUSE_BACKING_INSET;
    let corner_gap = GREENHOUSE_CORNER_BAND - GREENHOUSE_CORNER_OVERLAP;
    let pillar_gap = GREENHOUSE_B_PILLAR_HALF_WIDTH + GREENHOUSE_SEAL_OVERLAP;
    let inset = -GREENHOUSE_BACKING_INSET;
    for side in [-1.0, 1.0] {
        add_side_patch(
            &mut mesh,
            side,
            y0,
            y1,
            front_z(y0) + corner_gap,
            GREENHOUSE_B_PILLAR_Z - pillar_gap,
            front_z(y1) + corner_gap,
            GREENHOUSE_B_PILLAR_Z - pillar_gap,
            inset,
        );
        add_side_patch(
            &mut mesh,
            side,
            y0,
            y1,
            GREENHOUSE_B_PILLAR_Z + pillar_gap,
            rear_z(y0) - corner_gap,
            GREENHOUSE_B_PILLAR_Z + pillar_gap,
            rear_z(y1) - corner_gap,
            inset,
        );
    }
    for front in [true, false] {
        let x0 = (if front {
            front_half_width(y0)
        } else {
            rear_half_width(y0)
        }) - corner_gap;
        let x1 = (if front {
            front_half_width(y1)
        } else {
            rear_half_width(y1)
        }) - corner_gap;
        add_end_patch(&mut mesh, front, y0, y1, -x0, x0, -x1, x1, inset);
    }
    mesh.finish()
}

fn greenhouse_interior_mesh() -> Mesh {
    let mut mesh = GreenhouseMeshBuilder::default();
    // Deliberately compact and inset beyond the backing on every boundary, so
    // no dark corner can escape through a glass/pillar join at an oblique view.
    mesh.cuboid(
        Vec3::new(-0.18, GREENHOUSE_WINDOW_BOTTOM_Y + 0.035, -0.02),
        Vec3::new(0.18, GREENHOUSE_WINDOW_TOP_Y - 0.045, 0.28),
    );
    mesh.finish()
}

fn greenhouse_material(part: GreenhouseMeshPart) -> StandardMaterial {
    match part {
        GreenhouseMeshPart::Glass => StandardMaterial {
            base_color: Color::srgb(0.025, 0.055, 0.085),
            metallic: 0.0,
            perceptual_roughness: GREENHOUSE_GLASS_ROUGHNESS,
            reflectance: 0.5,
            alpha_mode: AlphaMode::Opaque,
            ..default()
        },
        GreenhouseMeshPart::Interior => StandardMaterial {
            base_color: Color::srgb(0.008, 0.009, 0.012),
            metallic: 0.0,
            perceptual_roughness: 0.92,
            ..default()
        },
        GreenhouseMeshPart::Frame => StandardMaterial {
            base_color: Color::srgb(0.62, 0.025, 0.02),
            metallic: 0.9,
            perceptual_roughness: 0.16,
            clearcoat: 1.0,
            clearcoat_perceptual_roughness: 0.10,
            ..default()
        },
    }
}

fn spawn_car(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    textures: Res<TextureAssets>,
) {
    // --- Shared meshes/materials for the body's nested children ---
    // One cached mesh per greenhouse layer replaces the hard cabin box and
    // floating window plates. Frame and panes are generated from the same
    // taper/rake functions, so their seams remain coherent by construction.
    let greenhouse_frame_mesh = meshes.add(greenhouse_frame_mesh());
    let greenhouse_glass_mesh = meshes.add(greenhouse_glass_mesh());
    let greenhouse_interior_mesh = meshes.add(greenhouse_interior_mesh());
    let greenhouse_backing_mesh = meshes.add(greenhouse_glass_backing_mesh());
    let glass_mat = materials.add(greenhouse_material(GreenhouseMeshPart::Glass));
    let interior_mat = materials.add(greenhouse_material(GreenhouseMeshPart::Interior));
    // This keeps the paint contract centralized/testable; runtime frame
    // entities intentionally share the existing textured car-paint handle.
    let _paint_contract = greenhouse_material(GreenhouseMeshPart::Frame);

    // No painted undertray/platform or spawned skirt cuboids: the smooth body
    // and tapered fenders alone define the lower painted silhouette.
    let fender_meshes = [
        [
            meshes.add(fender_mesh(-1.0, -WHEEL_Z)),
            meshes.add(fender_mesh(-1.0, WHEEL_Z)),
        ],
        [
            meshes.add(fender_mesh(1.0, -WHEEL_Z)),
            meshes.add(fender_mesh(1.0, WHEEL_Z)),
        ],
    ];
    let grille_mesh = meshes.add(fascia_surface_mesh(
        -1.0,
        Vec2::new(0.0, FASCIA_GRILLE_Y),
        Vec2::new(FASCIA_GRILLE_WIDTH, FASCIA_GRILLE_HEIGHT),
    ));
    let grille_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.008, 0.01, 0.012),
        perceptual_roughness: 0.9,
        ..default()
    });

    // Fascia lenses are individually conformed to the actual ellipsoid near
    // |z|=.95.  Their baked vertices stay flush and visibly face the camera.
    let headlight_meshes = [-FASCIA_LIGHT_X, FASCIA_LIGHT_X].map(|x| {
        meshes.add(fascia_surface_mesh(
            -1.0,
            Vec2::new(x, FASCIA_LIGHT_Y),
            Vec2::new(FASCIA_LIGHT_WIDTH, FASCIA_LIGHT_HEIGHT),
        ))
    });
    let headlight_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.9, 0.6),
        emissive: LinearRgba::new(1.0, 0.9, 0.6, 1.0),
        perceptual_roughness: 0.18,
        ..default()
    });

    // Brake lights use the same surface construction at the tail. Both
    // children share one material so `brake_lights` can dim/brighten them.
    let brake_meshes = [-FASCIA_LIGHT_X, FASCIA_LIGHT_X].map(|x| {
        meshes.add(fascia_surface_mesh(
            1.0,
            Vec2::new(x, FASCIA_LIGHT_Y),
            Vec2::new(FASCIA_LIGHT_WIDTH, FASCIA_LIGHT_HEIGHT),
        ))
    });
    let brake_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.3, 0.02, 0.02),
        emissive: LinearRgba::new(0.8, 0.05, 0.05, 1.0),
        perceptual_roughness: 0.22,
        ..default()
    });

    // Wheels: cylinders with the axle along X, tire-black. Their inner
    // sidewalls tuck beneath the new fender volume and overlap only hidden
    // axle ends. A shared hub exposes a metallic cap on each outside.
    let wheel_mesh = meshes.add(Cylinder::new(WHEEL_RADIUS, WHEEL_WIDTH));
    let wheel_mat = materials.add(StandardMaterial {
        base_color: palette::CAR_WHEEL,
        perceptual_roughness: 0.9,
        ..default()
    });
    let hub_mesh = meshes.add(Cylinder::new(0.066, WHEEL_WIDTH));
    let hub_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.42, 0.45, 0.48),
        metallic: 0.9,
        perceptual_roughness: 0.2,
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
                // The frame begins on the smooth body with matched tangent and
                // analytic normal. A compact dark interior spawns first, then
                // contained glass and the painted pillar/roof shell surround it.
                body.spawn((
                    Mesh3d(greenhouse_interior_mesh.clone()),
                    MeshMaterial3d(interior_mat.clone()),
                    Transform::IDENTITY,
                    GreenhouseInterior,
                ));
                body.spawn((
                    Mesh3d(greenhouse_backing_mesh.clone()),
                    MeshMaterial3d(interior_mat.clone()),
                    Transform::IDENTITY,
                    GreenhouseInterior,
                ));
                body.spawn((
                    Mesh3d(greenhouse_glass_mesh.clone()),
                    MeshMaterial3d(glass_mat.clone()),
                    Transform::IDENTITY,
                    GreenhouseGlass,
                ));
                body.spawn((
                    Mesh3d(greenhouse_frame_mesh.clone()),
                    MeshMaterial3d(textures.car_paint.clone()),
                    Transform::IDENTITY,
                    GreenhouseFrame,
                ));

                // Surface-conforming fascia has no carrier plate or bumper.
                // Its baked points sit at the actual |z|≈.95 nose/tail skin.
                body.spawn((
                    Mesh3d(grille_mesh.clone()),
                    MeshMaterial3d(grille_mat.clone()),
                    Transform::IDENTITY,
                ));
                for (index, _x) in [-FASCIA_LIGHT_X, FASCIA_LIGHT_X].into_iter().enumerate() {
                    body.spawn((
                        Mesh3d(headlight_meshes[index].clone()),
                        MeshMaterial3d(headlight_mat.clone()),
                        Transform::IDENTITY,
                    ));
                    body.spawn((
                        Mesh3d(brake_meshes[index].clone()),
                        MeshMaterial3d(brake_mat.clone()),
                        Transform::IDENTITY,
                        BrakeLight,
                    ));
                }

                // Broad body-rooted shoulder caps sweep over each upper tire.
                // Their rounded outer returns and wide tangent roots contain no
                // annular hoop, exposed edge, or pointed longitudinal endpoint.
                for (side_index, _side) in [-1.0_f32, 1.0].into_iter().enumerate() {
                    for (z_index, _z) in [-WHEEL_Z, WHEEL_Z].into_iter().enumerate() {
                        body.spawn((
                            Mesh3d(fender_meshes[side_index][z_index].clone()),
                            MeshMaterial3d(textures.car_paint.clone()),
                            Transform::IDENTITY,
                        ));
                    }
                }
            });

            // Wheels tuck inward beneath the connected fender volumes and
            // overlap hidden axle ends. Negative Z remains front.
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
    mut car: Query<(&mut Car, &mut Transform)>,
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
    let Ok((mut car, mut tf)) = car.single_mut() else {
        return;
    };
    let dt = time.delta_secs();

    car.speed = next_speed(car.speed, cfg.max_speed, *input, dt);

    // Arcade handbrake drift: build/recover bounded slip and flag active drift
    // for the tighter turn radius below. The handbrake never affects speed.
    let drifting = is_drifting(car.speed, *input, handbrake.0);
    car.drift = next_drift(
        car.drift,
        car.speed,
        *input,
        handbrake.0,
        dt,
        cfg.turn_rate,
        cfg.max_speed,
    );

    // Steering scales with speed so the car can't spin in place. While
    // drifting the handbrake breaks rear traction and lets the nose rotate
    // faster — a tighter turn radius — without changing speed integration.
    let turn_scale = if drifting { DRIFT_TURN_BOOST } else { 1.0 };
    car.heading += input.steer.clamp(-1.0, 1.0)
        * cfg.turn_rate
        * turn_scale
        * dt
        * (car.speed / cfg.max_speed);

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

    fn mesh_positions(mesh: &Mesh) -> &Vec<[f32; 3]> {
        match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            Some(VertexAttributeValues::Float32x3(values)) => values,
            _ => panic!("greenhouse positions must be Float32x3"),
        }
    }

    fn mesh_normals(mesh: &Mesh) -> &Vec<[f32; 3]> {
        match mesh.attribute(Mesh::ATTRIBUTE_NORMAL) {
            Some(VertexAttributeValues::Float32x3(values)) => values,
            _ => panic!("greenhouse normals must be Float32x3"),
        }
    }

    fn mesh_bounds(mesh: &Mesh) -> (Vec3, Vec3) {
        mesh_positions(mesh).iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(min, max), p| {
                let p = Vec3::from_array(*p);
                (min.min(p), max.max(p))
            },
        )
    }

    #[test]
    fn welded_transition_starts_on_ellipsoid_with_analytic_normal_and_tangent() {
        for edge in 0..4 {
            for i in 0..=GREENHOUSE_TRANSITION_SEGMENTS {
                let u = i as f32 / GREENHOUSE_TRANSITION_SEGMENTS as f32;
                let (p0, n0) = transition_point_normal(edge, u, 0.0);
                let sill = sill_edge(edge, u);
                let (surface, _, expected_normal, _) =
                    nearest_ellipsoid_projection(sill, sill_edge_derivative(edge));
                assert!((p0 - (surface - expected_normal * GREENHOUSE_WELD_INSET)).length() < 1e-6);
                let ellipsoid = surface.x.powi(2) / BODY_AXES.x.powi(2)
                    + surface.y.powi(2) / BODY_AXES.y.powi(2)
                    + surface.z.powi(2) / BODY_AXES.z.powi(2);
                assert!((ellipsoid - 1.0).abs() < 1e-5);
                assert!((sill - surface).normalize().dot(expected_normal).abs() > 0.99999);
                assert!(n0.dot(expected_normal) > 0.99999);
                let (p1, _) = transition_point_normal(edge, u, 1e-3);
                let derivative = (p1 - p0).normalize_or_zero();
                assert!(
                    derivative.dot(n0).abs() < 0.035,
                    "transition is not tangent"
                );
            }
        }
    }

    #[test]
    fn transition_uses_nearest_projection_compact_span_and_smooth_analytic_normals() {
        assert!(GREENHOUSE_TANGENT_LENGTH <= 0.02);
        for edge in 0..4 {
            for u in [0.0, 0.37, 1.0] {
                let sill = sill_edge(edge, u);
                let (surface, _, expected, _) =
                    nearest_ellipsoid_projection(sill, sill_edge_derivative(edge));
                let (p0, n0) = transition_point_normal(edge, u, 0.0);
                assert!(p0.distance(surface) <= GREENHOUSE_WELD_INSET + 1e-6);
                assert!(n0.dot(expected) > 0.99999);
                assert!(surface.distance(sill) < 0.14, "weld span is not compact");
                let mut previous = n0;
                for i in 1..=32 {
                    let normal = transition_point_normal(edge, u, i as f32 / 32.0).1;
                    assert!(
                        previous.dot(normal) > 0.97,
                        "transition normal jump edge={edge} u={u} step={i} dot={}",
                        previous.dot(normal)
                    );
                    previous = normal;
                }
            }
        }
    }

    #[test]
    fn greenhouse_surfaces_have_finite_unit_normals_valid_indices_and_winding() {
        for mesh in [
            greenhouse_frame_mesh(),
            greenhouse_glass_mesh(),
            greenhouse_glass_backing_mesh(),
            greenhouse_interior_mesh(),
            fender_mesh(-1.0, -WHEEL_Z),
            fender_mesh(1.0, WHEEL_Z),
            fascia_surface_mesh(
                -1.0,
                Vec2::new(FASCIA_LIGHT_X, FASCIA_LIGHT_Y),
                Vec2::new(FASCIA_LIGHT_WIDTH, FASCIA_LIGHT_HEIGHT),
            ),
        ] {
            let positions = mesh_positions(&mesh);
            let normals = mesh_normals(&mesh);
            assert!(!positions.is_empty());
            assert_eq!(positions.len(), normals.len());
            for normal in normals {
                let n = Vec3::from_array(*normal);
                assert!(n.is_finite() && (n.length() - 1.0).abs() < 1e-5);
            }
            let Some(Indices::U32(indices)) = mesh.indices() else {
                panic!("u32 indices required")
            };
            assert_eq!(indices.len() % 3, 0);
            for triangle in indices.chunks_exact(3) {
                assert!(triangle.iter().all(|&i| i < positions.len() as u32));
                let a = Vec3::from_array(positions[triangle[0] as usize]);
                let b = Vec3::from_array(positions[triangle[1] as usize]);
                let c = Vec3::from_array(positions[triangle[2] as usize]);
                let face = (b - a).cross(c - a);
                assert!(
                    face.length_squared() > 1e-10,
                    "degenerate triangle: {triangle:?}"
                );
                let expected = (Vec3::from_array(normals[triangle[0] as usize])
                    + Vec3::from_array(normals[triangle[1] as usize])
                    + Vec3::from_array(normals[triangle[2] as usize]))
                .normalize_or_zero();
                assert!(face.dot(expected) > 0.0, "triangle winding opposes normals");
            }
        }
    }

    #[test]
    fn roof_is_densely_tessellated_crowned_through_end_headers_and_smooth() {
        let frame = greenhouse_frame_mesh();
        let positions = mesh_positions(&frame);
        let normals = mesh_normals(&frame);
        let (_, max) = mesh_bounds(&frame);
        assert!((max.y - GREENHOUSE_TOP_Y).abs() < 1e-5);

        let roof_floor = GREENHOUSE_ROOF_BASE_Y;
        let roof: Vec<(Vec3, Vec3)> = positions
            .iter()
            .zip(normals)
            .map(|(p, n)| (Vec3::from_array(*p), Vec3::from_array(*n)))
            .filter(|(p, _)| {
                p.y >= roof_floor - 1e-6
                    && p.z >= GREENHOUSE_FRONT_TOP_Z - 1e-6
                    && p.z <= GREENHOUSE_REAR_TOP_Z + 1e-6
            })
            .collect();
        let distinct_x: std::collections::BTreeSet<i32> = roof
            .iter()
            .map(|(p, _)| (p.x * 10_000.0).round() as i32)
            .collect();
        let distinct_z: std::collections::BTreeSet<i32> = roof
            .iter()
            .map(|(p, _)| (p.z * 10_000.0).round() as i32)
            .collect();
        assert!(distinct_x.len() >= 11 && distinct_z.len() >= 9);

        // Side rails return to roof-base with zero crown slope. The cross-car
        // crown continues at full height through both front/rear boundaries.
        for (u, v) in [(0.0, 0.4), (1.0, 0.6)] {
            let (p, du, dv) = roof_sample(u, v);
            assert!((p.y - roof_floor).abs() < 1e-6);
            assert!(du.y.abs() < 1e-6 && dv.y.abs() < 1e-6);
        }
        for v in [0.0, 0.5, 1.0] {
            let (center, du, dv) = roof_sample(0.5, v);
            assert!((center.y - GREENHOUSE_TOP_Y).abs() < 1e-6);
            assert!(du.is_finite() && dv.is_finite() && du.cross(dv).length_squared() > 1e-6);
        }

        // Each end header is a four-ring curved transition, not a flat span.
        // Its complete upper row exactly matches roof positions and normals,
        // while its lower row matches the raked end face normal.
        for front in [true, false] {
            let mut header = GreenhouseMeshBuilder::default();
            add_end_header(&mut header, front, GREENHOUSE_WINDOW_TOP_Y);
            let header = header.finish();
            assert_eq!(mesh_positions(&header).len(), 10 * 4 * 2 * 3);
            let v = if front { 0.0 } else { 1.0 };
            for ix in 0..=10 {
                let u = ix as f32 / 10.0;
                let (expected, du, dv) = roof_sample(u, v);
                let expected_normal = dv.cross(du).normalize();
                let matching_normals: Vec<Vec3> = mesh_positions(&header)
                    .iter()
                    .zip(mesh_normals(&header))
                    .filter(|(p, _)| Vec3::from_array(**p).distance(expected) < 1e-6)
                    .map(|(_, n)| Vec3::from_array(*n))
                    .collect();
                assert!(
                    !matching_normals.is_empty(),
                    "header omitted roof boundary sample {ix}"
                );
                assert!(
                    matching_normals
                        .iter()
                        .all(|normal| normal.dot(expected_normal) > 0.99999)
                );

                let (_, lower_normal) = end_header_sample(front, GREENHOUSE_WINDOW_TOP_Y, u, 0.0);
                assert!(lower_normal.y.abs() < 0.8);
                assert!(if front {
                    lower_normal.z < -0.5
                } else {
                    lower_normal.z > 0.5
                });
                let h = 1e-4;
                let lower_tangent = (end_header_sample(front, GREENHOUSE_WINDOW_TOP_Y, u, h).0
                    - end_header_sample(front, GREENHOUSE_WINDOW_TOP_Y, u, 0.0).0)
                    / h;
                assert!(lower_tangent.dot(lower_normal).abs() < 2e-3);
            }
        }

        // Analytic derivatives agree with centered finite differences.
        for (u, v) in [(0.2, 0.3), (0.5, 0.5), (0.8, 0.7)] {
            let h = 1e-3;
            let (_, analytic_u, analytic_v) = roof_sample(u, v);
            let numeric_u = (roof_sample(u + h, v).0 - roof_sample(u - h, v).0) / (2.0 * h);
            let numeric_v = (roof_sample(u, v + h).0 - roof_sample(u, v - h).0) / (2.0 * h);
            assert!(analytic_u.distance(numeric_u) < 2e-4);
            assert!(analytic_v.distance(numeric_v) < 2e-4);
        }

        // The generated normals at every shared roof-grid vertex equal the
        // exact derivative cross product (and therefore each duplicate agrees).
        for iz in 0..9 {
            for ix in 0..11 {
                let u = ix as f32 / 10.0;
                let v = iz as f32 / 8.0;
                let (p, du, dv) = roof_sample(u, v);
                let expected = dv.cross(du).normalize();
                let matches: Vec<Vec3> = positions
                    .iter()
                    .zip(normals)
                    .map(|(candidate, normal)| {
                        (Vec3::from_array(*candidate), Vec3::from_array(*normal))
                    })
                    .filter(|(candidate, normal)| {
                        candidate.distance_squared(p) < 1e-12 && normal.dot(expected) > 0.9
                    })
                    .map(|(_, normal)| normal)
                    .collect();
                assert!(!matches.is_empty());
                assert!(matches.iter().all(|normal| normal.dot(expected) > 0.99999));
            }
        }
    }

    #[test]
    fn glazing_and_backing_overlap_corner_pillars_without_exposing_edges() {
        let glass = greenhouse_glass_mesh();
        let backing = greenhouse_glass_backing_mesh();
        for mesh in [&glass, &backing] {
            for p in mesh_positions(mesh) {
                let p = Vec3::from_array(*p);
                assert!(p.y > GREENHOUSE_WINDOW_BOTTOM_Y && p.y < GREENHOUSE_WINDOW_TOP_Y);
                assert!(p.z >= front_z(p.y) - 0.01 && p.z <= rear_z(p.y) + 0.01);
                assert!(p.x.abs() <= side_x(p.y, p.z, 1.0).abs() + 0.01);
                // Side panes (points near the side envelope) cannot enter the
                // B-pillar strip. They deliberately do enter corner pillars.
                if (p.x.abs() - side_x(p.y, p.z, 1.0).abs()).abs() < 0.02 {
                    assert!(
                        (p.z - GREENHOUSE_B_PILLAR_Z).abs()
                            >= GREENHOUSE_B_PILLAR_HALF_WIDTH + GREENHOUSE_SEAL_OVERLAP - 0.003
                    );
                }
            }
        }
        assert!((0.075..=0.085).contains(&GREENHOUSE_CORNER_BAND));
        assert!(((GREENHOUSE_B_PILLAR_HALF_WIDTH + GREENHOUSE_SEAL_OVERLAP) - 0.022).abs() < 1e-6);
        assert!(GREENHOUSE_CORNER_OVERLAP >= 0.025);
        assert!(GREENHOUSE_CORNER_OVERLAP > GREENHOUSE_SEAL_OVERLAP);
        let pane_corner_inset = GREENHOUSE_CORNER_BAND - GREENHOUSE_CORNER_OVERLAP;
        assert!(pane_corner_inset <= 0.055);
        // Both adjacent pane coordinates cross the inner edge of an 80 mm
        // pillar by at least 25 mm before their differing normal offsets.
        assert!(GREENHOUSE_CORNER_BAND - pane_corner_inset >= 0.025 - 1e-6);
        assert!(GREENHOUSE_GLASS_INSET >= 0.018);
        assert!(GREENHOUSE_BACKING_INSET - GREENHOUSE_GLASS_INSET >= 0.020);
        assert!(
            GREENHOUSE_WINDOW_BOTTOM_Y + GREENHOUSE_BACKING_INSET
                < GREENHOUSE_WINDOW_BOTTOM_Y + GREENHOUSE_BACKING_INSET + GREENHOUSE_SEAL_OVERLAP
        );
        assert!(
            GREENHOUSE_WINDOW_TOP_Y - GREENHOUSE_BACKING_INSET
                > GREENHOUSE_WINDOW_TOP_Y - GREENHOUSE_BACKING_INSET - GREENHOUSE_SEAL_OVERLAP
        );

        // The first left-side pane is displaced exclusively along its own
        // raked face normal by the requested glass inset.
        let y0 = GREENHOUSE_WINDOW_BOTTOM_Y + GREENHOUSE_GLASS_INSET;
        let y1 = GREENHOUSE_WINDOW_TOP_Y - GREENHOUSE_GLASS_INSET;
        let nominal = [
            Vec3::new(
                side_x(
                    y0,
                    front_z(y0) + GREENHOUSE_CORNER_BAND - GREENHOUSE_CORNER_OVERLAP,
                    -1.0,
                ),
                y0,
                front_z(y0) + GREENHOUSE_CORNER_BAND - GREENHOUSE_CORNER_OVERLAP,
            ),
            Vec3::new(
                side_x(
                    y0,
                    GREENHOUSE_B_PILLAR_Z
                        - GREENHOUSE_B_PILLAR_HALF_WIDTH
                        - GREENHOUSE_SEAL_OVERLAP,
                    -1.0,
                ),
                y0,
                GREENHOUSE_B_PILLAR_Z - GREENHOUSE_B_PILLAR_HALF_WIDTH - GREENHOUSE_SEAL_OVERLAP,
            ),
            Vec3::new(
                side_x(
                    y1,
                    GREENHOUSE_B_PILLAR_Z
                        - GREENHOUSE_B_PILLAR_HALF_WIDTH
                        - GREENHOUSE_SEAL_OVERLAP,
                    -1.0,
                ),
                y1,
                GREENHOUSE_B_PILLAR_Z - GREENHOUSE_B_PILLAR_HALF_WIDTH - GREENHOUSE_SEAL_OVERLAP,
            ),
            Vec3::new(
                side_x(
                    y1,
                    front_z(y1) + GREENHOUSE_CORNER_BAND - GREENHOUSE_CORNER_OVERLAP,
                    -1.0,
                ),
                y1,
                front_z(y1) + GREENHOUSE_CORNER_BAND - GREENHOUSE_CORNER_OVERLAP,
            ),
        ];
        let face_normal = (nominal[1] - nominal[0])
            .cross(nominal[2] - nominal[0])
            .normalize();
        for (actual, nominal) in mesh_positions(&glass)[0..4].iter().zip(nominal) {
            let delta = Vec3::from_array(*actual) - nominal;
            assert!(delta.cross(face_normal).length() < 1e-6);
            assert!((delta.dot(face_normal) + GREENHOUSE_GLASS_INSET).abs() < 1e-6);
        }

        // Backing is vertically tighter and substantially deeper than glass,
        // ensuring an exposed edge cannot reveal a dark wedge behind a pillar.
        let (gmin, gmax) = mesh_bounds(&glass);
        let (bmin, bmax) = mesh_bounds(&backing);
        assert!(bmin.y > gmin.y && bmax.y < gmax.y);
        assert!(bmin.x > gmin.x && bmax.x < gmax.x);
        // Own-normal offsets can move a raked pane's world-Z bound in either
        // direction; containment is guaranteed by tighter vertical edges and
        // a larger inward face-normal distance, not axis-aligned Z.

        let (imin, imax) = mesh_bounds(&greenhouse_interior_mesh());
        assert!(
            imin.x > -GREENHOUSE_REAR_TOP_HALF_WIDTH && imax.x < GREENHOUSE_REAR_TOP_HALF_WIDTH
        );
        assert!(imin.z > GREENHOUSE_FRONT_TOP_Z && imax.z < GREENHOUSE_REAR_TOP_Z);
    }

    #[test]
    fn greenhouse_has_asymmetric_rake_and_inward_sloping_sides() {
        assert!((GREENHOUSE_SILL_Y - 0.13).abs() < 1e-6);
        assert!(GREENHOUSE_WINDOW_BOTTOM_Y - GREENHOUSE_SILL_Y <= 0.02);
        assert!(front_z(GREENHOUSE_ROOF_BASE_Y) > front_z(GREENHOUSE_SILL_Y));
        assert!(rear_z(GREENHOUSE_ROOF_BASE_Y) < rear_z(GREENHOUSE_SILL_Y));
        assert!(front_half_width(GREENHOUSE_ROOF_BASE_Y) < front_half_width(GREENHOUSE_SILL_Y));
        assert!(rear_half_width(GREENHOUSE_ROOF_BASE_Y) < rear_half_width(GREENHOUSE_SILL_Y));
        let normals = mesh_normals(&greenhouse_glass_mesh()).clone();
        assert!(normals.iter().any(|n| n[2] < -0.5));
        assert!(normals.iter().any(|n| n[2] > 0.5));
        assert!(normals.iter().any(|n| n[0].abs() > 0.8));
    }

    #[test]
    fn greenhouse_materials_separate_paint_glass_and_dark_interior() {
        let frame = greenhouse_material(GreenhouseMeshPart::Frame);
        let glass = greenhouse_material(GreenhouseMeshPart::Glass);
        let interior = greenhouse_material(GreenhouseMeshPart::Interior);
        assert!(frame.metallic >= 0.8 && frame.perceptual_roughness <= 0.2);
        assert_eq!(glass.metallic, 0.0, "glass must be dielectric");
        assert!((0.14..=0.20).contains(&glass.perceptual_roughness));
        assert_eq!(glass.alpha_mode, AlphaMode::Opaque);
        assert_eq!(glass.base_color.to_srgba().alpha, 1.0);
        assert!(interior.metallic == 0.0 && interior.perceptual_roughness > 0.8);
        let glass_luma = glass.base_color.to_linear().red
            + glass.base_color.to_linear().green
            + glass.base_color.to_linear().blue;
        let interior_luma = interior.base_color.to_linear().red
            + interior.base_color.to_linear().green
            + interior.base_color.to_linear().blue;
        assert!(interior_luma < glass_luma);
    }

    #[test]
    fn iteration9_fenders_are_broad_rounded_body_rooted_shoulders() {
        assert_eq!(
            WHEEL_POSITIONS,
            [(0.49, 0.65), (-0.49, 0.65), (0.49, -0.65), (-0.49, -0.65)]
        );
        assert!(FENDER_Z_HALF_SPAN >= WHEEL_RADIUS + 0.02);
        assert!(FENDER_BULGE >= 0.24);
        assert!(FENDER_X_STEPS >= 10 && FENDER_Z_STEPS >= 16);

        for side in [-1.0, 1.0] {
            for wheel_z in [-WHEEL_Z, WHEEL_Z] {
                let mesh = fender_mesh(side, wheel_z);
                let (min, max) = mesh_bounds(&mesh);
                assert!((min.z - (wheel_z - FENDER_Z_HALF_SPAN)).abs() <= FENDER_WELD_INSET);
                assert!((max.z - (wheel_z + FENDER_Z_HALF_SPAN)).abs() <= FENDER_WELD_INSET);

                // Minimum surface breadth: even the front/rear boundary is a
                // wide line on the body, never the point of a tapered annulus.
                for along in [0.0, 0.5, 1.0] {
                    let inner = fender_point(side, wheel_z, along, 0.0);
                    let outer = fender_point(side, wheel_z, along, 1.0);
                    let chord = inner.distance(outer);
                    assert!(chord >= 0.15, "narrow shoulder chord {chord}");
                    assert!(chord / (2.0 * FENDER_Z_HALF_SPAN) >= 0.38);
                }

                // The visible longitudinal profile leaves the buried end weld
                // quickly, rounds into a broad shoulder, and remains symmetric.
                let displacement = |along: f32| {
                    let p = fender_point(side, wheel_z, along, 0.5);
                    let z = wheel_z + FENDER_Z_HALF_SPAN * (along * 2.0 - 1.0);
                    let side_limit = BODY_AXES.x
                        * (1.0 - z.powi(2) / BODY_AXES.z.powi(2)).max(0.0).sqrt();
                    let x = side * (FENDER_ROOT_X + (side_limit - FENDER_ROOT_X) * 0.5);
                    let surface = Vec3::new(x, body_surface_y(x, z), z);
                    p.distance(surface - body_normal(surface) * FENDER_WELD_INSET)
                };
                assert!(displacement(0.1) < FENDER_BULGE * 0.02);
                assert!(displacement(0.25) > FENDER_BULGE * 0.20);
                assert!((displacement(0.25) - displacement(0.75)).abs() < 1e-5);
                assert!((displacement(0.5) - FENDER_BULGE).abs() < 1e-5);

                // The middle has substantial two-dimensional area and a
                // rounded, non-blade profile above the body base.
                let center = fender_point(side, wheel_z, 0.5, 0.5);
                let center_base =
                    center - Vec3::new(side, FENDER_BULGE_RISE, 0.0).normalize() * FENDER_BULGE;
                let base_y = body_surface_y(center_base.x, center_base.z);
                assert!(center.distance(center_base) >= 0.24);
                assert!(
                    base_y - center.y > 0.01,
                    "cap does not wrap around tire"
                );
                assert!(
                    center.x.abs() > WHEEL_X,
                    "shoulder does not reach tire flank"
                );
                let center_world_y = center.y + BODY_CENTER_Y;
                assert!(center_world_y > WHEEL_Y + WHEEL_RADIUS + 0.03);
                assert!(center_world_y < WHEEL_Y + WHEEL_RADIUS + 0.12);

                // Every perimeter edge is narrowly buried beneath the body to
                // prevent coplanar z-fighting, while its generated normal stays
                // continuous with the analytic body normal.
                for i in 0..=20 {
                    let t = i as f32 / 20.0;
                    for (along, across) in [(0.0, t), (1.0, t), (t, 0.0), (t, 1.0)] {
                        let (p, n) = fender_point_normal(side, wheel_z, along, across);
                        let ellipsoid = p.x.powi(2) / BODY_AXES.x.powi(2)
                            + p.y.powi(2) / BODY_AXES.y.powi(2)
                            + p.z.powi(2) / BODY_AXES.z.powi(2);
                        assert!(ellipsoid < 0.999 && ellipsoid > 0.95);
                        assert!(n.dot(body_normal(p)) > 0.995, "fender root normal seam");
                    }
                }
            }
        }
    }

    #[test]
    fn glazing_has_painted_oblique_containment_margin() {
        let aperture_height = GREENHOUSE_WINDOW_TOP_Y - GREENHOUSE_WINDOW_BOTTOM_Y;
        let glass_height = aperture_height - 2.0 * GREENHOUSE_GLASS_INSET;
        assert!(
            glass_height / aperture_height > 0.85,
            "window readability lost"
        );
        assert!(GREENHOUSE_GLASS_INSET >= 0.018);
        assert!(GREENHOUSE_BACKING_INSET >= 0.040);
        assert!(GREENHOUSE_BACKING_INSET - GREENHOUSE_GLASS_INSET >= 0.020);
        assert!(GREENHOUSE_CORNER_OVERLAP >= 0.025);
        assert!(GREENHOUSE_CORNER_BAND - GREENHOUSE_CORNER_OVERLAP >= 0.05);

        // At every aperture corner the pane lies behind paint in all three
        // relevant dimensions: vertical seal, plan overlap, and face depth.
        for y in [
            GREENHOUSE_WINDOW_BOTTOM_Y + GREENHOUSE_GLASS_INSET,
            GREENHOUSE_WINDOW_TOP_Y - GREENHOUSE_GLASS_INSET,
        ] {
            assert!(
                (y - GREENHOUSE_WINDOW_BOTTOM_Y).min(GREENHOUSE_WINDOW_TOP_Y - y)
                    >= GREENHOUSE_GLASS_INSET - 1e-6
            );
        }
        let (gmin, gmax) = mesh_bounds(&greenhouse_glass_mesh());
        let (bmin, bmax) = mesh_bounds(&greenhouse_glass_backing_mesh());
        assert!(
            bmin.y > gmin.y && bmax.y < gmax.y,
            "vertical containment bottom={} top={}",
            bmin.y - gmin.y,
            gmax.y - bmax.y
        );
    }

    #[test]
    fn fascia_conforms_to_visible_ellipsoid_nose_and_tail() {
        assert!((FASCIA_LIGHT_X - 0.22).abs() < 1e-6);
        assert!((FASCIA_LIGHT_WIDTH - 0.12).abs() < 1e-6);
        assert!((FASCIA_LIGHT_HEIGHT - 0.07).abs() < 1e-6);
        assert!((FASCIA_GRILLE_WIDTH - 0.26).abs() < 1e-6);
        assert!((FASCIA_GRILLE_HEIGHT - 0.06).abs() < 1e-6);
        assert!((FASCIA_GRILLE_Y - FASCIA_LIGHT_Y).abs() < 1e-6);
        for end in [-1.0, 1.0] {
            for center in [
                Vec2::new(-FASCIA_LIGHT_X, FASCIA_LIGHT_Y),
                Vec2::new(FASCIA_LIGHT_X, FASCIA_LIGHT_Y),
                Vec2::new(0.0, FASCIA_GRILLE_Y),
            ] {
                let size = if center.x == 0.0 {
                    Vec2::new(FASCIA_GRILLE_WIDTH, FASCIA_GRILLE_HEIGHT)
                } else {
                    Vec2::new(FASCIA_LIGHT_WIDTH, FASCIA_LIGHT_HEIGHT)
                };
                let mesh = fascia_surface_mesh(end, center, size);
                for p in mesh_positions(&mesh) {
                    let p = Vec3::from_array(*p);
                    let surface = body_surface_z(p.x, p.y);
                    assert!((p.z.abs() - (surface + FASCIA_SURFACE_LIFT)).abs() < 1e-5);
                    assert!(p.z.signum() == end);
                    assert!(
                        (0.78..1.01).contains(&p.z.abs()),
                        "fascia is not on visible end skin"
                    );
                }
            }
        }
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
