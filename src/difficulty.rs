//! Difficulty ramp + oncoming traffic (T18).
//!
//! This module is the sole owner of the difficulty / traffic logic. It
//! provides:
//!
//! - `Difficulty { elapsed, level }` — a resource tracking how long the
//!   current round has been running (only ticks while input is NOT frozen,
//!   mirroring `tick_timeleft`) and the derived difficulty level
//!   (`level = (elapsed / 10) as u32`, capped at 6).
//! - `Traffic { speed, axis, dir, speed_roll }` — a moving car the player must
//!   avoid. Traffic entities are top-level (world `Transform`) and carry an
//!   axis-correct `world::Collider` matching the 1×2 visual footprint, so
//!   `car.rs::physics_collisions` treats them as solid obstacles — crashing into one
//!   emits `ObstacleHit` → damage. The baseline count scales with `level`
//!   (`1 + level/2`); active modifier and event multipliers are then composed,
//!   with the final population capped at 8. They drive straight along a world
//!   axis and are recycled (despawned + respawned safely offscreen ahead) once
//!   they drift ~90u from the car.
//! - A small "Lv {level}" UI node top-right, just below the minimap.
//!
//! Contracts honoured:
//! - `Difficulty` is reset on `OnEnter(Playing)` inside `crate::game::SpawnSet`
//!   and skips when `RoundActive.0` is already true (resume from `Paused`).
//! - `tick_difficulty` is gated on `InputFrozen` (doesn't ramp during the
//!   countdown), like `tick_timeleft`.
//! - Traffic is top-level so its `GlobalTransform` is never `IDENTITY` (the
//!   stale-at-spawn guard in `physics_collisions` filters the spawn frame,
//!   then real positions propagate — same shape as chunk-child obstacles).
//! - `Collider` comes from `world.rs` (read-only here); `Traffic` is defined
//!   locally so `world.rs` is never edited.
//! - UI lifecycle mirrors `minimap.rs` / `health.rs`: spawned on
//!   `OnEnter(Playing)`, despawned on `OnExit(Playing)`.

use bevy::color::LinearRgba;
use bevy::prelude::*;
use bevy::text::FontSize;
use std::cmp::Ordering;
use std::f32::consts::{FRAC_PI_2, TAU};

use crate::car::{Car, DrivingSet, InputFrozen};
use crate::game::SpawnSet;
use crate::game::TouchStateSet;
use crate::game::resources::RoundActive;
use crate::game::state::GameState;
use crate::modifiers::ActiveModifier;
use crate::run_events::ActiveEvent;
use crate::touch::{
    TOUCH_LEVEL_HEIGHT, TOUCH_LEVEL_RIGHT, TOUCH_LEVEL_TOP, TOUCH_LEVEL_WIDTH, TouchControlsActive,
};
use crate::world::{Collider, is_road_line};

// ===========================================================================
// Tuning constants
// ===========================================================================

/// Seconds of round elapsed per difficulty level. `level = (elapsed / 10)`.
const LEVEL_SECONDS: f32 = 10.0;
/// Maximum difficulty level (caps the ramp over the 60s round: 0..=6).
const MAX_LEVEL: u32 = 6;

/// Hard population cap after applying active modifier and event multipliers.
/// Rush Hour and Traffic Surge can reach this existing cap sooner, including
/// when composed, but cannot create an unbounded number of traffic entities.
const MAX_TRAFFIC: usize = 8;

/// Distance from the car (XZ) beyond which a traffic car is recycled
/// (despawned + replaced). Keeps the traffic near the endless driver.
const TRAFFIC_KEEP_RADIUS: f32 = 90.0;

/// Traffic spawn envelope ahead of the car's camera-facing heading. Candidate
/// points start 34..58u ahead; road snapping is accepted only when the final
/// lane-centred position remains at least `SPAWN_AHEAD_MIN` ahead.
const SPAWN_AHEAD_MIN: f32 = 34.0;
const SPAWN_AHEAD_RANGE: f32 = 24.0;
/// No traffic root may be created inside this XZ circle around the car, even
/// after snapping the candidate to a real road line and directional lane.
const SPAWN_SAFE_RADIUS: f32 = 26.0;
/// Small tolerance used only at floating-point comparisons/test boundaries.
const SPAWN_SAFETY_TOLERANCE: f32 = 1e-3;
/// Bounded candidate count keeps spawning deterministic and constant-time.
const SPAWN_RETRY_CANDIDATES: usize = 8;
/// The deterministic fallback aims comfortably inside the normal envelope.
const SPAWN_FALLBACK_AHEAD: f32 = 46.0;
/// Lateral jitter around the ahead-biased candidate point. The final
/// cross-road coordinate is replaced by a deterministic road line + lane.
const SPAWN_LATERAL: f32 = 3.0;
/// World spacing and half-width of the roads built in `world.rs`.
const ROAD_GRID: f32 = 40.0;
#[cfg(test)]
const ROAD_HALF: f32 = 4.0;
/// Centre of each directional lane. With the traffic half-width included,
/// every car remains comfortably inside the road's ±4u paved area.
const LANE_OFFSET: f32 = 1.5;
const TRAFFIC_HALF_WIDTH: f32 = 0.5;
const TRAFFIC_HALF_LENGTH: f32 = 1.0;

/// Base traffic speed at level 0 (u/s). The player's `max_speed` is 12.0, so
/// traffic is always slower and must be dodged, not outrun-forward forever.
const TRAFFIC_BASE_SPEED: f32 = 5.0;
/// Per-level speed gain (so later traffic is a bit quicker). At level 6 →
/// `5 + 6*0.7 = 9.2`, still under the player's cap.
const TRAFFIC_SPEED_PER_LEVEL: f32 = 0.7;
/// Per-car speed jitter band: `speed *= 0.85 + rand * 0.3` (0.85..1.15).
const TRAFFIC_SPEED_JITTER: f32 = 0.3;
const TRAFFIC_SPEED_JITTER_BASE: f32 = 0.85;
/// Fairness cap after composing modifier and event speed multipliers. This
/// retains a 0.5u/s margin below the player's documented 12.0u/s maximum.
const TRAFFIC_MAX_SPEED: f32 = 11.5;

// --- Traffic car mesh proportions ---
const BODY_W: f32 = 1.0;
const BODY_D: f32 = 2.0;
const WINDSHIELD_D: f32 = 0.03;
const TRAFFIC_WHEEL_RADIUS: f32 = 0.15;

/// Shared visual silhouettes. Selection reads the module's deterministic
/// LCG state without advancing it, so visuals do not perturb movement rolls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrafficKind {
    Sedan,
    Van,
}

impl TrafficKind {
    fn index(self) -> usize {
        match self {
            Self::Sedan => 0,
            Self::Van => 1,
        }
    }
}

// --- UI placement (top-right, below the minimap) ---
/// Minimap bottom edge = `minimap::PANEL_TOP (54) + MAP_SIZE (120)`; sit 8px
/// below it so the "Lv" label clears the panel.
const UI_TOP: f32 = 54.0 + 120.0 + 8.0; // 182
/// Right offset matches the minimap's `PANEL_RIGHT` (16) for alignment.
const UI_RIGHT: f32 = 16.0;

// ===========================================================================
// Resources
// ===========================================================================

/// Round difficulty state.
///
/// - `elapsed` — seconds the round has been actively running (frozen while
///   `InputFrozen` is set, e.g. during the countdown).
/// - `level`   — derived difficulty level `(elapsed / 10) as u32`, capped at
///   [`MAX_LEVEL`]. Drives the traffic population + speed.
#[derive(Resource, Default)]
pub struct Difficulty {
    pub elapsed: f32,
    pub level: u32,
}

/// Pre-built mesh + material handles for traffic cars. Built once via
/// `FromWorld` (resource-scoping `Assets<Mesh>` then `Assets<StandardMaterial>`,
/// mirroring `chickens.rs::ChickenAssets` / `pickups.rs::PickupAssets`).
#[derive(Resource)]
pub struct TrafficAssets {
    /// Sedan and van geometry, indexed by [`TrafficKind::index`].
    body_meshes: [Handle<Mesh>; 2],
    cabin_meshes: [Handle<Mesh>; 2],
    windshield_meshes: [Handle<Mesh>; 2],
    light_mesh: Handle<Mesh>,
    wheel_mesh: Handle<Mesh>,
    hub_mesh: Handle<Mesh>,
    shadow_mesh: Handle<Mesh>,
    /// A small shared car-paint palette, selected at spawn.
    body_mats: [Handle<StandardMaterial>; 5],
    cabin_mat: Handle<StandardMaterial>,
    windshield_mat: Handle<StandardMaterial>,
    headlight_mat: Handle<StandardMaterial>,
    rear_light_mat: Handle<StandardMaterial>,
    wheel_mat: Handle<StandardMaterial>,
    hub_mat: Handle<StandardMaterial>,
    shadow_mat: Handle<StandardMaterial>,
}

impl FromWorld for TrafficAssets {
    fn from_world(world: &mut World) -> Self {
        // Build every mesh/material exactly once. Spawns below only clone
        // these handles; they never touch either Assets collection.
        world.resource_scope::<Assets<Mesh>, _>(|world, mut meshes| {
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();

            let body_meshes = [
                meshes.add(Cuboid::new(BODY_W, 0.5, BODY_D)),
                meshes.add(Cuboid::new(BODY_W, 0.65, BODY_D)),
            ];
            let cabin_meshes = [
                meshes.add(Cuboid::new(0.8, 0.4, 1.0)),
                meshes.add(Cuboid::new(0.86, 0.65, 1.45)),
            ];
            let windshield_meshes = [
                meshes.add(Cuboid::new(0.7, 0.2, WINDSHIELD_D)),
                meshes.add(Cuboid::new(0.76, 0.38, WINDSHIELD_D)),
            ];
            let light_mesh = meshes.add(Cuboid::new(0.18, 0.12, 0.04));
            let wheel_mesh = meshes.add(Cylinder::new(TRAFFIC_WHEEL_RADIUS, 0.18));
            let hub_mesh = meshes.add(Cylinder::new(0.066, 0.19));
            let shadow_mesh = meshes.add(Plane3d::default().mesh().size(1.55, 2.35));

            let body_colors = [
                Color::srgb(0.85, 0.12, 0.10),
                Color::srgb(0.15, 0.35, 0.85),
                Color::srgb(0.18, 0.55, 0.22),
                Color::srgb(0.78, 0.78, 0.82),
                Color::srgb(0.95, 0.65, 0.08),
            ];
            let body_mats = body_colors.map(|base_color| {
                materials.add(StandardMaterial {
                    base_color,
                    metallic: 0.6,
                    // Shared glossy metallic paint responds consistently to
                    // the scene's image-based lighting without clearcoat or
                    // per-car material allocation.
                    perceptual_roughness: 0.25,
                    ..default()
                })
            });
            let cabin_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.10, 0.10, 0.18),
                perceptual_roughness: 0.4,
                metallic: 0.2,
                ..default()
            });
            let windshield_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.05, 0.08, 0.12),
                perceptual_roughness: 0.08,
                metallic: 0.6,
                ..default()
            });
            let headlight_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(1.0, 0.9, 0.6),
                emissive: LinearRgba::new(1.0, 0.9, 0.6, 1.0),
                perceptual_roughness: 0.18,
                ..default()
            });
            let rear_light_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.45, 0.015, 0.01),
                emissive: LinearRgba::new(0.8, 0.025, 0.015, 1.0),
                perceptual_roughness: 0.22,
                ..default()
            });
            let wheel_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.025, 0.025, 0.03),
                perceptual_roughness: 0.92,
                ..default()
            });
            let hub_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.5, 0.53, 0.56),
                metallic: 0.95,
                perceptual_roughness: 0.18,
                ..default()
            });
            let shadow_mat = materials.add(StandardMaterial {
                base_color: Color::srgba(0.0, 0.0, 0.0, 0.35),
                alpha_mode: AlphaMode::Blend,
                ..default()
            });

            TrafficAssets {
                body_meshes,
                cabin_meshes,
                windshield_meshes,
                light_mesh,
                wheel_mesh,
                hub_mesh,
                shadow_mesh,
                body_mats,
                cabin_mat,
                windshield_mat,
                headlight_mat,
                rear_light_mat,
                wheel_mat,
                hub_mat,
                shadow_mat,
            }
        })
    }
}

// ===========================================================================
// Components
// ===========================================================================

/// A moving traffic car the player must avoid.
///
/// - `speed` — current units per second along the movement axis.
/// - `axis`  — `true` => drives along world X; `false` => along world Z.
/// - `dir`   — `+1.0` or `-1.0` (direction along the axis).
/// - `speed_roll` — immutable deterministic jitter sampled at spawn. The
///   effective `speed` is rebuilt from this roll every frame, so difficulty,
///   modifier, and event transitions affect existing traffic immediately.
///
/// The entity is **top-level** (world `Transform`) and also carries a
/// axis-correct `Collider` matching its 1×2 footprint so `physics_collisions`
/// crashes the car into it. The root `Transform`'s rotation is set at spawn so the
/// body's front (-Z, where the headlights are) faces the movement direction;
/// `manage_traffic` rebuilds `speed` and advances `translation` each frame.
#[derive(Component)]
pub struct Traffic {
    pub(crate) speed: f32,
    pub(crate) axis: bool,
    pub(crate) dir: f32,
    pub(crate) speed_roll: f32,
}

/// A wheel directly parented to a [`Traffic`] root. Keeping spin as a scalar
/// lets the animation rebuild the axle-aligned rotation each frame instead of
/// repeatedly multiplying quaternions (which would eventually drift/tumble).
#[derive(Component, Default)]
struct TrafficWheel {
    spin_radians: f32,
}

/// Root node of the "Lv {level}" UI. Despawned on exit from `Playing`
/// (mirrors `minimap.rs` / `health.rs` UI lifecycle).
#[derive(Component)]
struct DifficultyUiRoot;

/// Dynamic number span inside the "Lv " text, refreshed each frame by
/// `update_difficulty_ui`.
#[derive(Component)]
struct DifficultyLevelText;

// ===========================================================================
// Plugin
// ===========================================================================

pub struct DifficultyPlugin;

impl Plugin for DifficultyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Difficulty>()
            .init_resource::<TrafficAssets>()
            // Fresh-round reset (skipped on resume from Paused). MUST run
            // before `reset_run` flips `RoundActive` on, so it's in
            // `SpawnSet` (which `reset_run` follows via `.after(SpawnSet)`).
            .add_systems(
                OnEnter(GameState::Playing),
                reset_difficulty.in_set(SpawnSet),
            )
            // UI lifecycle tied to the Playing state (despawned on exit so a
            // pause/resume cycle respawns it cleanly, like the HUD/minimap).
            .add_systems(OnEnter(GameState::Playing), spawn_difficulty_ui)
            .add_systems(
                OnExit(GameState::Playing),
                despawn_marker::<DifficultyUiRoot>,
            )
            // Per-frame: ramp the level, manage traffic, then animate the
            // wheel children from each owning traffic root's speed.
            .add_systems(
                Update,
                (
                    tick_difficulty,
                    manage_traffic.after(tick_difficulty).before(DrivingSet),
                    spin_traffic_wheels.after(manage_traffic),
                )
                    .run_if(in_state(GameState::Playing)),
            )
            // UI refresh runs in every state so the label recovers even while
            // paused; the query is trivial when the UI root is absent.
            .add_systems(Update, update_difficulty_ui)
            .add_systems(
                Update,
                update_difficulty_layout
                    .after(TouchStateSet)
                    .run_if(in_state(GameState::Playing)),
            )
            // Clean up traffic on round end / menu return (NOT on Paused —
            // traffic persists across pause so resume continues seamlessly).
            .add_systems(OnEnter(GameState::GameOver), cleanup_traffic)
            .add_systems(OnEnter(GameState::Menu), cleanup_traffic);
    }
}

// ===========================================================================
// Difficulty ramp
// ===========================================================================

/// Advance the round-elapsed clock and derive the difficulty level. Gated on
/// `InputFrozen` so the ramp doesn't progress during the countdown (matches
/// `tick_timeleft`). Runs only while `Playing`.
fn tick_difficulty(
    mut difficulty: ResMut<Difficulty>,
    time: Res<Time>,
    input_frozen: Res<InputFrozen>,
) {
    if input_frozen.0 {
        return;
    }
    difficulty.elapsed += time.delta_secs();
    difficulty.level = ((difficulty.elapsed / LEVEL_SECONDS) as u32).min(MAX_LEVEL);
}

/// Reset the difficulty ramp on a fresh round. Skipped when resuming from
/// `Paused` (`RoundActive` already true), per the fresh-round rule (risk E11).
/// Runs in `SpawnSet` so it precedes `reset_run`.
fn reset_difficulty(mut difficulty: ResMut<Difficulty>, round_active: Res<RoundActive>) {
    if round_active.0 {
        return;
    }
    difficulty.elapsed = 0.0;
    difficulty.level = 0;
}

// ===========================================================================
// Traffic — spawn / move / recycle / cleanup
// ===========================================================================

/// Per-frame traffic management while `Playing`:
/// 1. rebuild every car's speed from its fixed jitter and current effects;
/// 2. advance each traffic car along its axis/direction;
/// 3. recycle out-of-range cars and deterministically trim target surplus;
/// 4. top up to the level-, modifier-, and event-derived target count.
///
/// Explicit opposing `Car`/`Traffic` and root/wheel filters keep mutable
/// component accesses disjoint and prevent B0001 as these queries evolve.
fn manage_traffic(
    mut commands: Commands,
    assets: Res<TrafficAssets>,
    difficulty: Res<Difficulty>,
    modifier: Res<ActiveModifier>,
    event: Res<ActiveEvent>,
    car: Query<&Transform, (With<Car>, Without<Traffic>)>,
    mut traffic: Query<
        (Entity, &mut Traffic, &mut Transform),
        (With<Traffic>, Without<TrafficWheel>, Without<Car>),
    >,
    time: Res<Time>,
    mut seed: Local<u32>,
) {
    ensure_seeded(&mut seed, 0x0BADC0DE);
    let Ok(car_t) = car.single() else {
        return;
    };
    let car_pos = car_t.translation;
    let dt = time.delta_secs();
    let modifier_speed = modifier.traffic_speed_multiplier();
    let event_speed = event.traffic_speed_multiplier();
    let target = target_traffic_count(
        difficulty.level,
        modifier.traffic_count_multiplier(),
        event.traffic_count_multiplier(),
    );

    // --- Recompute speed, move, recycle, and trim a decreased target. ---
    // `speed_roll` is fixed at spawn, so every existing car responds to a
    // difficulty/modifier/event transition without consuming another random
    // number. Only in-radius survivors enter the surplus ordering, preventing
    // deferred despawns from being counted as alive or selected twice.
    let mut to_despawn: Vec<Entity> = Vec::new();
    let mut survivors: Vec<(Entity, f32)> = Vec::new();
    for (entity, mut traffic, mut tf) in &mut traffic {
        let speed_roll = traffic.speed_roll;
        traffic.speed =
            traffic_speed_for_roll(difficulty.level, speed_roll, modifier_speed, event_speed);
        let axis_vec = if traffic.axis {
            Vec3::new(traffic.dir, 0.0, 0.0)
        } else {
            Vec3::new(0.0, 0.0, traffic.dir)
        };
        tf.translation += axis_vec * traffic.speed * dt;

        let dx = tf.translation.x - car_pos.x;
        let dz = tf.translation.z - car_pos.z;
        let distance_squared = dx * dx + dz * dz;
        if distance_squared > TRAFFIC_KEEP_RADIUS * TRAFFIC_KEEP_RADIUS {
            to_despawn.push(entity);
        } else {
            survivors.push((entity, distance_squared));
        }
    }

    let surplus = traffic_surplus(survivors.len(), target);
    if surplus > 0 {
        survivors.sort_by(|(entity_a, distance_a), (entity_b, distance_b)| {
            traffic_despawn_order(
                (entity_a.to_bits(), *distance_a),
                (entity_b.to_bits(), *distance_b),
            )
        });
        to_despawn.extend(survivors.drain(..surplus).map(|(entity, _)| entity));
    }
    let alive = survivors.len();
    for entity in to_despawn {
        commands.entity(entity).despawn();
    }

    // --- Top up to the level-derived, modifier- and event-adjusted target. ---
    let mut needed = target.saturating_sub(alive);

    // Car forward (heading 0 => -Z) defines the camera-facing offscreen
    // envelope. Final snapped positions are validated against this heading.
    let forward = car_t.rotation * Vec3::NEG_Z;
    let forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();

    while needed > 0 {
        needed -= 1;
        let axis = rand(&mut seed) < 0.5; // true = X, false = Z
        let dir = if rand(&mut seed) < 0.5 { 1.0 } else { -1.0 };
        // Keep this roll in the original speed-roll position in the shared
        // spawn LCG sequence. Storing it changes no subsequent spawn rolls.
        let speed_roll = rand(&mut seed);
        let speed =
            traffic_speed_for_roll(difficulty.level, speed_roll, modifier_speed, event_speed);
        // Choose axis + direction before position: the cross-axis coordinate
        // is placed on a real deterministic road line, then offset into the
        // direction's lane so opposing traffic does not overlap.
        let pos = traffic_spawn_pos_on_road(car_pos, forward, axis, dir, &mut seed);
        spawn_one_traffic(
            &mut commands,
            &assets,
            pos,
            axis,
            dir,
            speed,
            speed_roll,
            &mut seed,
        );
    }
}

/// Despawn every traffic car (e.g. on GameOver / Menu). Recursive despawn in
/// 0.19 nukes the body/cabin/headlight children (safe, risk E2).
fn cleanup_traffic(mut commands: Commands, traffic: Query<Entity, With<Traffic>>) {
    for e in &traffic {
        commands.entity(e).despawn();
    }
}

/// Distance-derived wheel rotation for one frame. Traffic speed is expressed
/// in world units per second and the traffic root always faces local -Z, so a
/// positive local-X rotation gives forward rolling regardless of road axis or
/// direction.
fn traffic_wheel_spin_delta(speed: f32, delta_seconds: f32) -> f32 {
    speed * delta_seconds / TRAFFIC_WHEEL_RADIUS
}

/// Reconstruct an axle-aligned wheel rotation from its scalar spin state.
/// Applying spin around the cylinder's local Y before the fixed axle rotation
/// keeps the resulting world-space axle on local X without accumulated error.
fn traffic_wheel_rotation(spin_radians: f32) -> Quat {
    Quat::from_rotation_z(FRAC_PI_2) * Quat::from_rotation_y(spin_radians)
}

/// Animate direct wheel children from their owning traffic root. The explicit
/// opposing filters keep mutable wheel transforms disjoint from traffic-root
/// transforms in `manage_traffic` and guard against B0001 if either query is
/// expanded later.
fn spin_traffic_wheels(
    time: Res<Time>,
    mut wheels: Query<
        (&ChildOf, &mut TrafficWheel, &mut Transform),
        (With<TrafficWheel>, Without<Traffic>),
    >,
    owners: Query<&Traffic, (With<Traffic>, Without<TrafficWheel>)>,
) {
    let delta_seconds = time.delta_secs();
    for (child_of, mut wheel, mut transform) in &mut wheels {
        let Ok(traffic) = owners.get(child_of.parent()) else {
            continue;
        };
        wheel.spin_radians = (wheel.spin_radians
            + traffic_wheel_spin_delta(traffic.speed, delta_seconds))
        .rem_euclid(TAU);
        transform.rotation = traffic_wheel_rotation(wheel.spin_radians);
    }
}

// ===========================================================================
// Traffic spawn helpers
// ===========================================================================

/// Target traffic population for a given difficulty level, active modifier,
/// and active event. Applying both multipliers after deriving the baseline
/// preserves the fully neutral path exactly, while the final cap bounds even
/// Rush Hour composed with Traffic Surge.
fn target_traffic_count(
    level: u32,
    modifier_count_multiplier: usize,
    event_count_multiplier: usize,
) -> usize {
    let baseline = (1 + level / 2).min(MAX_TRAFFIC as u32) as usize;
    baseline
        .saturating_mul(modifier_count_multiplier)
        .saturating_mul(event_count_multiplier)
        .min(MAX_TRAFFIC)
}

/// Pure speed calculation for a supplied jitter roll, active modifier, and
/// active event. The fully neutral path remains unchanged; composed speeds
/// retain a margin below the player's maximum for fairness.
fn traffic_speed_for_roll(
    level: u32,
    jitter_roll: f32,
    modifier_speed_multiplier: f32,
    event_speed_multiplier: f32,
) -> f32 {
    let base = TRAFFIC_BASE_SPEED + level as f32 * TRAFFIC_SPEED_PER_LEVEL;
    let jittered = base * (TRAFFIC_SPEED_JITTER_BASE + jitter_roll * TRAFFIC_SPEED_JITTER);
    (jittered * modifier_speed_multiplier * event_speed_multiplier).min(TRAFFIC_MAX_SPEED)
}

/// Number of currently live traffic cars that must be removed to satisfy a
/// target. Kept pure so deferred ECS despawns never influence the accounting.
fn traffic_surplus(alive: usize, target: usize) -> usize {
    alive.saturating_sub(target)
}

/// Deterministic removal order: farther traffic first, then ascending entity
/// bits as a stable tie-break. Inputs are `(entity_bits, distance_squared)` so
/// the policy is pure and independently testable.
fn traffic_despawn_order(a: (u64, f32), b: (u64, f32)) -> Ordering {
    b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0))
}

/// Translate `Traffic::axis` into `world::is_road_line`'s axis convention.
/// X-moving traffic needs a horizontal (Z-indexed) line; Z-moving traffic
/// needs a vertical (X-indexed) line.
fn road_exists_for_movement(axis: bool, line: i32) -> bool {
    is_road_line(!axis, line)
}

/// Find the nearest deterministic road line to a cross-road world coordinate.
/// Search order is stable, and the bounded fallback is line zero, which
/// `world.rs` guarantees is a road.
fn nearest_road_line(axis: bool, cross: f32) -> i32 {
    let center = (cross / ROAD_GRID).round() as i32;
    if road_exists_for_movement(axis, center) {
        return center;
    }
    for distance in 1..=64_i32 {
        let lower = center.saturating_sub(distance);
        let upper = center.saturating_add(distance);
        let lower_is_road = road_exists_for_movement(axis, lower);
        let upper_is_road = road_exists_for_movement(axis, upper);
        match (lower_is_road, upper_is_road) {
            (true, true) => {
                let lower_distance = (cross - lower as f32 * ROAD_GRID).abs();
                let upper_distance = (cross - upper as f32 * ROAD_GRID).abs();
                return if lower_distance <= upper_distance {
                    lower
                } else {
                    upper
                };
            }
            (true, false) => return lower,
            (false, true) => return upper,
            (false, false) => {}
        }
    }
    0
}

/// Direction-aware centre offset for one of the road's two lanes.
fn lane_offset(dir: f32) -> f32 {
    dir.signum() * LANE_OFFSET
}

/// Pure post-snap safety policy. Only XZ distance and the car heading's
/// forward projection matter; no arbitrary screen-space projection is used.
fn traffic_spawn_is_safe(car_pos: Vec3, forward: Vec3, pos: Vec3) -> bool {
    let heading = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    if heading == Vec3::ZERO {
        return false;
    }
    let delta = Vec3::new(pos.x - car_pos.x, 0.0, pos.z - car_pos.z);
    let forward_projection = delta.dot(heading);
    forward_projection.is_finite()
        && delta.length_squared().is_finite()
        && forward_projection >= SPAWN_AHEAD_MIN - SPAWN_SAFETY_TOLERANCE
        && delta.length_squared() >= SPAWN_SAFE_RADIUS.powi(2)
}

/// Snap a candidate to an existing road's direction-aware lane centre.
fn snap_traffic_candidate_to_road(mut pos: Vec3, axis: bool, dir: f32) -> Vec3 {
    let cross = if axis { pos.z } else { pos.x };
    let line = nearest_road_line(axis, cross);
    let lane = line as f32 * ROAD_GRID + lane_offset(dir);
    if axis {
        pos.z = lane;
    } else {
        pos.x = lane;
    }
    pos
}

/// First real road line at or beyond `start` in the requested direction.
/// The deterministic line network has an unbounded density of road lines;
/// this is used only by the guaranteed fallback after bounded spawn retries.
fn first_road_line_in_direction(axis: bool, start: i32, step: i32) -> i32 {
    debug_assert!(step == -1 || step == 1);
    let mut line = start;
    loop {
        if road_exists_for_movement(axis, line) {
            return line;
        }
        line = line.saturating_add(step);
        // World-space f32 coordinates cannot meaningfully reach this edge in
        // play. Line zero remains a real deterministic road for robustness.
        if line == i32::MIN || line == i32::MAX {
            return 0;
        }
    }
}

/// Guaranteed road-aligned fallback. It uses the road's free coordinate when
/// the heading mostly follows that road; otherwise it selects the first real
/// road line far enough ahead in the heading's cross-road direction.
fn fallback_traffic_spawn_pos(car_pos: Vec3, forward: Vec3, axis: bool, dir: f32) -> Vec3 {
    let heading = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    // A car rotation always supplies a valid heading. Retaining a stable
    // default makes this pure helper safe for malformed standalone callers.
    let heading = if heading == Vec3::ZERO {
        Vec3::NEG_Z
    } else {
        heading
    };
    let movement_forward = if axis { heading.x } else { heading.z };
    let cross_forward = if axis { heading.z } else { heading.x };
    let fallback_projection = SPAWN_FALLBACK_AHEAD + SPAWN_SAFETY_TOLERANCE;

    if movement_forward.abs() >= cross_forward.abs() {
        let mut pos =
            snap_traffic_candidate_to_road(car_pos + heading * SPAWN_FALLBACK_AHEAD, axis, dir);
        let delta = Vec3::new(pos.x - car_pos.x, 0.0, pos.z - car_pos.z);
        let correction = (fallback_projection - delta.dot(heading)) / movement_forward;
        if axis {
            pos.x += correction;
        } else {
            pos.z += correction;
        }
        return pos;
    }

    // Here `cross_forward` cannot be zero because the planar heading is a
    // unit vector and its cross component is the larger component.
    let car_cross = if axis { car_pos.z } else { car_pos.x };
    let required_cross = car_cross + fallback_projection / cross_forward;
    let line_coordinate = (required_cross - lane_offset(dir)) / ROAD_GRID;
    let (start, step) = if cross_forward > 0.0 {
        (line_coordinate.ceil() as i32, 1)
    } else {
        (line_coordinate.floor() as i32, -1)
    };
    let line = first_road_line_in_direction(axis, start, step);
    let lane = line as f32 * ROAD_GRID + lane_offset(dir);
    let mut pos = car_pos;
    if axis {
        pos.z = lane;
    } else {
        pos.x = lane;
    }
    pos
}

/// A spawn position ahead of the player, constrained to a road that actually
/// exists in the deterministic world network. Up to eight deterministic LCG
/// candidates are snapped to their correct lanes, then checked *after* that
/// snap. The first safe candidate wins; otherwise a safe road-line fallback
/// is used rather than returning a position near or on top of the player.
fn traffic_spawn_pos_on_road(
    car_pos: Vec3,
    forward: Vec3,
    axis: bool,
    dir: f32,
    seed: &mut u32,
) -> Vec3 {
    let heading = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    let heading = if heading == Vec3::ZERO {
        Vec3::NEG_Z
    } else {
        heading
    };
    let right = Vec3::new(heading.z, 0.0, -heading.x);

    for _ in 0..SPAWN_RETRY_CANDIDATES {
        let ahead = SPAWN_AHEAD_MIN + rand(seed) * SPAWN_AHEAD_RANGE;
        let lateral = (rand(seed) * 2.0 - 1.0) * SPAWN_LATERAL;
        let candidate =
            snap_traffic_candidate_to_road(car_pos + heading * ahead + right * lateral, axis, dir);
        if traffic_spawn_is_safe(car_pos, heading, candidate) {
            return candidate;
        }
    }

    let fallback = fallback_traffic_spawn_pos(car_pos, heading, axis, dir);
    debug_assert!(traffic_spawn_is_safe(car_pos, heading, fallback));
    fallback
}

/// Deterministic silhouette selection from the shared traffic LCG state.
/// This deliberately does not advance the state: adding visual variety must
/// not change subsequent movement direction, speed, or spawn-position rolls.
fn traffic_kind(seed: u32) -> TrafficKind {
    let visual_hash = seed.wrapping_mul(747796405).wrapping_add(2891336453) ^ seed.rotate_left(13);
    if visual_hash % 20 < 13 {
        TrafficKind::Sedan
    } else {
        TrafficKind::Van
    }
}

/// Spawn one traffic car (top-level) with a deterministic sedan/van shell,
/// lights, wheels, shadow, an axis-correct `Collider`, and the `Traffic` tag.
/// The root `Transform`'s rotation orients the body's front
/// (-Z) toward the movement direction so the headlights lead.
fn spawn_one_traffic(
    commands: &mut Commands,
    assets: &TrafficAssets,
    pos: Vec3,
    axis: bool,
    dir: f32,
    speed: f32,
    speed_roll: f32,
    seed: &mut u32,
) {
    // Movement direction vector in the XZ plane.
    let dir_vec = if axis {
        Vec3::new(dir, 0.0, 0.0)
    } else {
        Vec3::new(0.0, 0.0, dir)
    };
    // Heading so the body's -Z (front, where the headlights are) faces dir.
    // Same convention as `car.rs::move_car` / `chickens.rs::wander_chickens`:
    // forward = (-sin h, 0, -cos h) => h = atan2(-dx, -dz).
    let heading = (-dir_vec.x).atan2(-dir_vec.z);
    let rotation = Quat::from_rotation_y(heading);

    let kind = traffic_kind(*seed);
    let kind_idx = kind.index();
    let color_idx = (rand(seed) * assets.body_mats.len() as f32) as usize % assets.body_mats.len();
    // The root collider remains the original 1×2 footprint for both visual
    // silhouettes, preserving collision behaviour and fairness.
    commands
        .spawn((
            Transform::from_translation(pos).with_rotation(rotation),
            Visibility::default(),
            Traffic {
                speed,
                axis,
                dir,
                speed_roll,
            },
            Collider {
                // Collider is an axis-aligned world box, so swap the visual
                // local extents when the root is rotated onto world X.
                half_x: if axis {
                    TRAFFIC_HALF_LENGTH
                } else {
                    TRAFFIC_HALF_WIDTH
                },
                half_z: if axis {
                    TRAFFIC_HALF_WIDTH
                } else {
                    TRAFFIC_HALF_LENGTH
                },
            },
        ))
        .with_children(|root| {
            let (body_y, cabin_y, cabin_z, glass_y, glass_z) = match kind {
                TrafficKind::Sedan => (0.35, 0.35, 0.2, 0.45, -0.3),
                TrafficKind::Van => (0.41, 0.54, 0.1, 0.62, -0.64),
            };
            root.spawn((
                Mesh3d(assets.body_meshes[kind_idx].clone()),
                MeshMaterial3d(assets.body_mats[color_idx].clone()),
                Transform::from_xyz(0.0, body_y, 0.0),
            ))
            .with_children(|body| {
                body.spawn((
                    Mesh3d(assets.cabin_meshes[kind_idx].clone()),
                    MeshMaterial3d(assets.cabin_mat.clone()),
                    Transform::from_xyz(0.0, cabin_y, cabin_z),
                ));
                body.spawn((
                    Mesh3d(assets.windshield_meshes[kind_idx].clone()),
                    MeshMaterial3d(assets.windshield_mat.clone()),
                    Transform::from_xyz(0.0, glass_y, glass_z),
                ));
                // Warm front lamps and red rear lamps make heading readable.
                for &x in &[-0.3_f32, 0.3] {
                    body.spawn((
                        Mesh3d(assets.light_mesh.clone()),
                        MeshMaterial3d(assets.headlight_mat.clone()),
                        Transform::from_xyz(x, -0.1, -1.02),
                    ));
                    body.spawn((
                        Mesh3d(assets.light_mesh.clone()),
                        MeshMaterial3d(assets.rear_light_mat.clone()),
                        Transform::from_xyz(x, -0.1, 1.02),
                    ));
                }
            });

            // Four cylinder tires with slightly wider metallic hubs. Axles
            // lie along local X; all geometry/material handles are shared.
            for &(x, z) in &[(0.58, 0.7), (-0.58, 0.7), (0.58, -0.7), (-0.58, -0.7)] {
                root.spawn((
                    Mesh3d(assets.wheel_mesh.clone()),
                    MeshMaterial3d(assets.wheel_mat.clone()),
                    Transform::from_xyz(x, 0.15, z).with_rotation(traffic_wheel_rotation(0.0)),
                    TrafficWheel::default(),
                ))
                .with_child((
                    Mesh3d(assets.hub_mesh.clone()),
                    MeshMaterial3d(assets.hub_mat.clone()),
                    Transform::default(),
                ));
            }
            root.spawn((
                Mesh3d(assets.shadow_mesh.clone()),
                MeshMaterial3d(assets.shadow_mat.clone()),
                Transform::from_xyz(0.0, 0.06, 0.0),
            ));
        });
}

// ===========================================================================
// UI — "Lv {level}" top-right, below the minimap
// ===========================================================================

/// Spawn the "Lv {level}" label. Lives only while `Playing` (despawned by
/// [`despawn_marker::<DifficultyUiRoot>`] on exit). Positioned just below the
/// minimap (top-right), aligned with its right edge.
fn spawn_difficulty_ui(mut commands: Commands, touch: Res<TouchControlsActive>) {
    commands
        .spawn((
            difficulty_ui_node(touch.0),
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.35)),
            DifficultyUiRoot,
            Text::new("Lv "),
            difficulty_ui_font(touch.0),
            TextColor(crate::palette::HUD_TEXT.into()),
        ))
        .with_child((
            TextSpan::default(),
            difficulty_ui_font(touch.0),
            TextColor(crate::palette::HUD_ACCENT.into()),
            DifficultyLevelText,
        ));
}

fn difficulty_ui_node(touch_active: bool) -> Node {
    if touch_active {
        Node {
            position_type: PositionType::Absolute,
            top: px(TOUCH_LEVEL_TOP),
            right: px(TOUCH_LEVEL_RIGHT),
            width: px(TOUCH_LEVEL_WIDTH),
            height: px(TOUCH_LEVEL_HEIGHT),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: UiRect::all(px(4.0)),
            ..default()
        }
    } else {
        Node {
            position_type: PositionType::Absolute,
            top: px(UI_TOP),
            right: px(UI_RIGHT),
            padding: UiRect::all(px(6.0)),
            ..default()
        }
    }
}

fn difficulty_ui_font(touch_active: bool) -> TextFont {
    TextFont {
        font_size: FontSize::Px(if touch_active { 14.0 } else { 18.0 }),
        ..default()
    }
}

fn update_difficulty_layout(
    touch: Res<TouchControlsActive>,
    mut roots: Query<
        (&mut Node, &mut TextFont),
        (With<DifficultyUiRoot>, Without<DifficultyLevelText>),
    >,
    mut spans: Query<&mut TextFont, (With<DifficultyLevelText>, Without<DifficultyUiRoot>)>,
) {
    if !touch.0 {
        return;
    }
    for (mut node, mut font) in &mut roots {
        *node = difficulty_ui_node(true);
        *font = difficulty_ui_font(true);
    }
    for mut font in &mut spans {
        *font = difficulty_ui_font(true);
    }
}

/// Refresh the "Lv {level}" number span each frame. Runs in every state; the
/// query is empty when the UI root is absent (e.g. in Menu), so it's a no-op.
fn update_difficulty_ui(
    difficulty: Res<Difficulty>,
    mut spans: Query<&mut TextSpan, With<DifficultyLevelText>>,
) {
    for mut span in &mut spans {
        **span = format!("{}", difficulty.level);
    }
}

// ===========================================================================
// Shared helpers
// ===========================================================================

/// Despawn every entity tagged with marker `M` (mirrors `ui.rs` / `health.rs`
/// / `minimap.rs`).
fn despawn_marker<M: Component>(mut commands: Commands, q: Query<Entity, With<M>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

/// Tiny LCG (matches `world.rs` / `chickens.rs` / `pickups.rs`) — deterministic
/// pseudo-random 0..1 without pulling in the `rand` crate (keeps the web build
/// lean).
fn rand(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
    (*seed as f32) / (u32::MAX as f32)
}

/// Seed a `Local<u32>` RNG on first use so the LCG never starts from 0 (it
/// never produces 0 from a non-zero seed, so this fires exactly once).
fn ensure_seeded(seed: &mut u32, initial: u32) {
    if *seed == 0 {
        *seed = initial;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modifiers::ModifierKind;
    use crate::run_events::EventKind;

    #[test]
    fn standard_traffic_count_preserves_existing_boundaries() {
        let standard = ModifierKind::Standard.traffic_count_multiplier();
        let neutral_event = ActiveEvent(None).traffic_count_multiplier();
        assert_eq!(target_traffic_count(0, standard, neutral_event), 1);
        assert_eq!(target_traffic_count(1, standard, neutral_event), 1);
        assert_eq!(target_traffic_count(2, standard, neutral_event), 2);
        assert_eq!(target_traffic_count(6, standard, neutral_event), 4);
        assert_eq!(
            target_traffic_count(14, standard, neutral_event),
            MAX_TRAFFIC
        );
        assert_eq!(
            target_traffic_count(u32::MAX, standard, neutral_event),
            MAX_TRAFFIC
        );
    }

    #[test]
    fn rush_hour_increases_traffic_count_up_to_the_hard_cap() {
        let rush = ModifierKind::RushHour.traffic_count_multiplier();
        let neutral_event = ActiveEvent(None).traffic_count_multiplier();
        assert_eq!(target_traffic_count(0, rush, neutral_event), 2);
        assert_eq!(target_traffic_count(2, rush, neutral_event), 4);
        assert_eq!(target_traffic_count(6, rush, neutral_event), MAX_TRAFFIC);
        assert_eq!(target_traffic_count(14, rush, neutral_event), MAX_TRAFFIC);
    }

    #[test]
    fn standard_plus_traffic_surge_composes_count_and_speed() {
        let standard_count = ModifierKind::Standard.traffic_count_multiplier();
        let standard_speed = ModifierKind::Standard.traffic_speed_multiplier();
        let surge = ActiveEvent(Some(EventKind::TrafficSurge));

        assert_eq!(
            target_traffic_count(0, standard_count, surge.traffic_count_multiplier()),
            2
        );
        assert_eq!(
            target_traffic_count(2, standard_count, surge.traffic_count_multiplier()),
            4
        );
        assert_eq!(
            target_traffic_count(6, standard_count, surge.traffic_count_multiplier()),
            MAX_TRAFFIC
        );
        assert!(
            (traffic_speed_for_roll(0, 0.5, standard_speed, surge.traffic_speed_multiplier(),)
                - 6.25)
                .abs()
                < 1e-5
        );
    }

    #[test]
    fn neutral_to_surge_to_neutral_restores_speed_and_population() {
        let level = 2;
        let modifier = ModifierKind::Standard;
        let neutral = ActiveEvent(None);
        let surge = ActiveEvent(Some(EventKind::TrafficSurge));
        let roll = 0.37;

        let neutral_speed = traffic_speed_for_roll(
            level,
            roll,
            modifier.traffic_speed_multiplier(),
            neutral.traffic_speed_multiplier(),
        );
        let surge_speed = traffic_speed_for_roll(
            level,
            roll,
            modifier.traffic_speed_multiplier(),
            surge.traffic_speed_multiplier(),
        );
        let restored_speed = traffic_speed_for_roll(
            level,
            roll,
            modifier.traffic_speed_multiplier(),
            neutral.traffic_speed_multiplier(),
        );
        assert!(surge_speed > neutral_speed);
        assert_eq!(restored_speed, neutral_speed);

        let neutral_target = target_traffic_count(
            level,
            modifier.traffic_count_multiplier(),
            neutral.traffic_count_multiplier(),
        );
        let surge_target = target_traffic_count(
            level,
            modifier.traffic_count_multiplier(),
            surge.traffic_count_multiplier(),
        );
        assert_eq!((neutral_target, surge_target), (2, 4));
        assert_eq!(traffic_surplus(surge_target, neutral_target), 2);
        assert_eq!(traffic_surplus(neutral_target, surge_target), 0);

        // A second car with a different immutable roll also returns exactly
        // to its own neutral speed, proving the transition is not based on a
        // shared or newly sampled jitter value.
        let other_roll = 0.81;
        let other_neutral = traffic_speed_for_roll(
            level,
            other_roll,
            modifier.traffic_speed_multiplier(),
            neutral.traffic_speed_multiplier(),
        );
        let other_surge = traffic_speed_for_roll(
            level,
            other_roll,
            modifier.traffic_speed_multiplier(),
            surge.traffic_speed_multiplier(),
        );
        let other_restored = traffic_speed_for_roll(
            level,
            other_roll,
            modifier.traffic_speed_multiplier(),
            neutral.traffic_speed_multiplier(),
        );
        assert!(other_surge > other_neutral);
        assert_eq!(other_restored, other_neutral);
    }

    #[test]
    fn surplus_despawn_order_is_farthest_then_stable_entity_bits() {
        let mut candidates: [(u64, f32); 4] = [(30, 25.0), (20, 100.0), (10, 25.0), (40, 4.0)];
        candidates.sort_by(|a, b| traffic_despawn_order(*a, *b));
        assert_eq!(candidates, [(20, 100.0), (10, 25.0), (30, 25.0), (40, 4.0)]);
    }

    #[test]
    fn rush_hour_plus_traffic_surge_respects_hard_caps() {
        let rush_count = ModifierKind::RushHour.traffic_count_multiplier();
        let rush_speed = ModifierKind::RushHour.traffic_speed_multiplier();
        let surge = ActiveEvent(Some(EventKind::TrafficSurge));

        assert_eq!(
            target_traffic_count(0, rush_count, surge.traffic_count_multiplier()),
            4
        );
        assert_eq!(
            target_traffic_count(2, rush_count, surge.traffic_count_multiplier()),
            MAX_TRAFFIC
        );
        assert_eq!(
            target_traffic_count(u32::MAX, rush_count, surge.traffic_count_multiplier()),
            MAX_TRAFFIC
        );
        assert_eq!(
            traffic_speed_for_roll(MAX_LEVEL, 1.0, rush_speed, surge.traffic_speed_multiplier(),),
            TRAFFIC_MAX_SPEED
        );
    }

    #[test]
    fn standard_speed_boundaries_are_unchanged_and_rush_hour_is_faster_but_fair() {
        let standard = ModifierKind::Standard.traffic_speed_multiplier();
        let rush = ModifierKind::RushHour.traffic_speed_multiplier();
        let neutral_event = ActiveEvent(None).traffic_speed_multiplier();

        // Existing Standard formula at the runtime level/jitter boundaries.
        let standard_slowest = traffic_speed_for_roll(0, 0.0, standard, neutral_event);
        let standard_fastest = traffic_speed_for_roll(MAX_LEVEL, 1.0, standard, neutral_event);
        assert!((standard_slowest - 4.25).abs() < 1e-5);
        assert!((standard_fastest - 10.58).abs() < 1e-5);

        // Representative early/late rolls show Rush Hour's increase.
        let standard_early = traffic_speed_for_roll(0, 0.5, standard, neutral_event);
        let standard_late = traffic_speed_for_roll(MAX_LEVEL, 0.5, standard, neutral_event);
        let rush_early = traffic_speed_for_roll(0, 0.5, rush, neutral_event);
        let rush_late = traffic_speed_for_roll(MAX_LEVEL, 0.5, rush, neutral_event);
        assert!(rush_early > standard_early);
        assert!(rush_late > standard_late);
        assert!(rush_early <= TRAFFIC_MAX_SPEED);
        assert!(rush_late <= TRAFFIC_MAX_SPEED);
        assert_eq!(
            traffic_speed_for_roll(MAX_LEVEL, 1.0, rush, neutral_event),
            TRAFFIC_MAX_SPEED
        );
    }

    #[test]
    fn surge_speed_is_faster_before_saturation_and_never_exceeds_fairness_cap() {
        let surge = ActiveEvent(Some(EventKind::TrafficSurge)).traffic_speed_multiplier();
        let neutral_event = ActiveEvent(None).traffic_speed_multiplier();
        for modifier in [ModifierKind::Standard, ModifierKind::RushHour] {
            let modifier_speed = modifier.traffic_speed_multiplier();
            let neutral = traffic_speed_for_roll(0, 0.0, modifier_speed, neutral_event);
            let surged = traffic_speed_for_roll(0, 0.0, modifier_speed, surge);
            assert!(surged > neutral);

            for level in 0..=MAX_LEVEL {
                for roll in [0.0, 0.5, 1.0] {
                    assert!(
                        traffic_speed_for_roll(level, roll, modifier_speed, surge)
                            <= TRAFFIC_MAX_SPEED
                    );
                }
            }
        }
        assert_eq!(
            traffic_speed_for_roll(
                MAX_LEVEL,
                1.0,
                ModifierKind::Standard.traffic_speed_multiplier(),
                surge,
            ),
            TRAFFIC_MAX_SPEED
        );
    }

    #[test]
    fn wheel_spin_delta_tracks_distance_without_frame_rate_dependence() {
        let speed = 6.0;
        let one_frame = traffic_wheel_spin_delta(speed, 0.5);
        let two_frames =
            traffic_wheel_spin_delta(speed, 0.2) + traffic_wheel_spin_delta(speed, 0.3);
        let expected = speed * 0.5 / TRAFFIC_WHEEL_RADIUS;
        assert!((one_frame - expected).abs() < 1e-5);
        assert!((two_frames - expected).abs() < 1e-5);
        assert_eq!(traffic_wheel_spin_delta(0.0, 1.0), 0.0);
    }

    fn assert_safe_road_spawn(car: Vec3, forward: Vec3, axis: bool, dir: f32, pos: Vec3) {
        let heading = Vec3::new(forward.x, 0.0, forward.z).normalize();
        let delta = Vec3::new(pos.x - car.x, 0.0, pos.z - car.z);
        let cross = if axis { pos.z } else { pos.x };
        let line = ((cross - lane_offset(dir)) / ROAD_GRID).round() as i32;
        let offset = cross - line as f32 * ROAD_GRID;

        assert!(road_exists_for_movement(axis, line));
        assert!((offset - lane_offset(dir)).abs() < 1e-4);
        assert!(offset.abs() + TRAFFIC_HALF_WIDTH < ROAD_HALF);
        assert!(delta.length() >= SPAWN_SAFE_RADIUS);
        assert!(
            delta.dot(heading) >= SPAWN_AHEAD_MIN - SPAWN_SAFETY_TOLERANCE,
            "spawn {pos:?} was not offscreen-ahead of {car:?} along {heading:?}"
        );
        assert!(traffic_spawn_is_safe(car, heading, pos));
    }

    #[test]
    fn many_seeds_and_headings_spawn_safely_on_directional_road_lanes() {
        let cars = [
            Vec3::ZERO,
            Vec3::new(137.0, 0.0, -93.0),
            Vec3::new(-321.25, 0.0, 278.75),
        ];
        let headings = [
            Vec3::NEG_Z,
            Vec3::X,
            Vec3::Z,
            Vec3::NEG_X,
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(0.8, 0.0, -0.6),
            Vec3::new(-0.31, 0.0, -0.95),
        ];

        for (car_index, car) in cars.into_iter().enumerate() {
            for (heading_index, forward) in headings.into_iter().enumerate() {
                for axis in [false, true] {
                    for dir in [-1.0, 1.0] {
                        for seed_index in 0_u32..96 {
                            let mut seed = 0x1234_5678_u32
                                .wrapping_add(seed_index.wrapping_mul(0x9E37_79B9))
                                .wrapping_add((car_index as u32) << 12)
                                .wrapping_add((heading_index as u32) << 20);
                            let pos = traffic_spawn_pos_on_road(car, forward, axis, dir, &mut seed);
                            assert_safe_road_spawn(car, forward, axis, dir, pos);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn fallback_is_safe_and_road_aligned_for_all_cardinal_relationships() {
        let car = Vec3::new(83.25, 0.0, -117.75);
        let headings = [
            Vec3::NEG_Z,
            Vec3::X,
            Vec3::Z,
            Vec3::NEG_X,
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(-1.0, 0.0, 1.0),
        ];
        for forward in headings {
            for axis in [false, true] {
                for dir in [-1.0, 1.0] {
                    let pos = fallback_traffic_spawn_pos(car, forward, axis, dir);
                    assert_safe_road_spawn(car, forward, axis, dir, pos);
                }
            }
        }
    }

    #[test]
    fn spawn_is_deterministic_for_identical_inputs_and_lcg_state() {
        let car = Vec3::new(17.25, 0.0, -81.5);
        let forward = Vec3::new(-0.73, 0.0, 0.41);
        for axis in [false, true] {
            for dir in [-1.0, 1.0] {
                for initial_seed in [1, 2, 0x1234_5678, 0xCAFE_BABE, u32::MAX] {
                    let mut seed_a = initial_seed;
                    let mut seed_b = initial_seed;
                    let a = traffic_spawn_pos_on_road(car, forward, axis, dir, &mut seed_a);
                    let b = traffic_spawn_pos_on_road(car, forward, axis, dir, &mut seed_b);
                    assert_eq!(a, b);
                    assert_eq!(seed_a, seed_b);
                }
            }
        }
    }

    #[test]
    fn opposing_directions_select_separate_safe_lanes() {
        assert_eq!(lane_offset(1.0), LANE_OFFSET);
        assert_eq!(lane_offset(-1.0), -LANE_OFFSET);
        assert!(2.0 * LANE_OFFSET > 2.0 * TRAFFIC_HALF_WIDTH);
        assert!(LANE_OFFSET + TRAFFIC_HALF_WIDTH < ROAD_HALF);

        // Snapping the same road-centre candidate puts opposing directions on
        // opposite lane centres of one real road, never overlapping roots.
        for axis in [false, true] {
            let line = nearest_road_line(axis, 0.0);
            let center = line as f32 * ROAD_GRID;
            let candidate = if axis {
                Vec3::new(12.0, 0.0, center)
            } else {
                Vec3::new(center, 0.0, 12.0)
            };
            let negative = snap_traffic_candidate_to_road(candidate, axis, -1.0);
            let positive = snap_traffic_candidate_to_road(candidate, axis, 1.0);
            let separation = if axis {
                positive.z - negative.z
            } else {
                positive.x - negative.x
            };
            assert!((separation - 2.0 * LANE_OFFSET).abs() < 1e-5);
        }
    }

    #[test]
    fn safety_predicate_rejects_player_circle_and_not_ahead() {
        let car = Vec3::new(4.0, 0.0, -7.0);
        let forward = Vec3::NEG_Z;
        assert!(!traffic_spawn_is_safe(
            car,
            forward,
            car + forward * (SPAWN_SAFE_RADIUS - 0.1),
        ));
        assert!(!traffic_spawn_is_safe(
            car,
            forward,
            car - forward * (SPAWN_AHEAD_MIN + 10.0),
        ));
        assert!(traffic_spawn_is_safe(
            car,
            forward,
            car + forward * SPAWN_AHEAD_MIN,
        ));
    }

    #[test]
    fn kind_selection_is_deterministic_and_varied() {
        let seeds: Vec<_> = (0_u32..64)
            .map(|i| 0xCAFE_BABE_u32.wrapping_add(i.wrapping_mul(0x9E37_79B9)))
            .collect();
        let a: Vec<_> = seeds.iter().copied().map(traffic_kind).collect();
        let b: Vec<_> = seeds.iter().copied().map(traffic_kind).collect();
        assert_eq!(a, b);
        assert!(a.contains(&TrafficKind::Sedan));
        assert!(a.contains(&TrafficKind::Van));
    }
}
