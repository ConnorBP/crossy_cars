//! Difficulty ramp + oncoming traffic (T18).
//!
//! This module is the sole owner of the difficulty / traffic logic. It
//! provides:
//!
//! - `Difficulty { elapsed, level }` — a resource tracking how long the
//!   current round has been running (only ticks while input is NOT frozen,
//!   mirroring `tick_timeleft`) and the derived difficulty level
//!   (`level = (elapsed / 10) as u32`, capped at 6).
//! - `Traffic` — a moving car the player must avoid. Traffic follows the pure
//!   world lane graph across streamed-cell boundaries, retaining deterministic
//!   route state, distance progress, and curve-tangent velocity. Top-level
//!   conservative AABB colliders make traffic solid to the player. The baseline
//!   count scales with `level` (`1 + level/2`); active modifier and event
//!   multipliers compose under the existing cap of 8.
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
use crate::world::{
    Collider, LaneConnector, LaneCurve, LaneEdge, LaneTurn, ROAD_BLOCK_SIZE, road_plan,
    world_to_road_cell,
};

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

/// A width-first conservative gameplay ground envelope. The production camera
/// is fixed-horizontal (10..12u), but its isometric projection, lead, shake,
/// and portrait aspect enlarge the ground footprint. This deliberately broad
/// square avoids depending on camera/window queries and is safe for portrait.
const GAMEPLAY_GROUND_HALF_EXTENT: f32 = 32.0;
/// Traffic is retained well beyond the conservative visible envelope. This
/// preserves visible/surplus cars and gives curves time to continue naturally.
const TRAFFIC_KEEP_HALF_EXTENT: f32 = 90.0;
/// No traffic root may be created inside this XZ circle around the player.
const SPAWN_SAFE_RADIUS: f32 = 26.0;
/// Fixed bounded search around the player's road cell.
const SPAWN_CELL_RADIUS: i32 = 3;
const SPAWN_RETRY_CANDIDATES: usize = 24;
/// Arc-length lookup resolution. It intentionally matches the committed lane
/// graph's canonical sampled-length resolution.
const CURVE_LENGTH_SAMPLES: usize = 32;
/// Prevent an adversarial delta or malformed route from causing an unbounded
/// number of cross-cell transitions in one update.
const MAX_CONNECTOR_TRANSITIONS_PER_FRAME: usize = 8;
const TRAFFIC_HALF_WIDTH: f32 = 0.5;
const TRAFFIC_HALF_LENGTH: f32 = 1.0;
/// Empty body-to-body space maintained along a route.
const MIN_BUMPER_GAP: f32 = 1.5;
/// Normal speed changes are deliberately bounded. The distance safety clamp
/// below remains authoritative for an already-unsafe imported state.
const TRAFFIC_ACCELERATION: f32 = 2.5;
const TRAFFIC_DECELERATION: f32 = 6.0;
/// Half-width of the sampled central junction area. Parallel directional
/// lanes remain nonconflicting while crossing/merging paths are detected.
const JUNCTION_HALF_EXTENT: f32 = 7.0;
const JUNCTION_APPROACH_DISTANCE: f32 = 18.0;
/// Keep an ungranted front bumper visibly before the conflict boundary. Exact
/// contact with the stop line is not treated as already owning the junction.
const JUNCTION_STOP_MARGIN: f32 = 0.05;

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

/// A moving traffic car following one directed connector of the world lane
/// graph. Distance progress is measured in world units along the connector's
/// canonical sampled arc. `route_rng` is per-car, so route choices do not
/// depend on query order or unrelated spawns. `velocity` is the current world
/// XZ curve-tangent velocity consumed directly by collision impact handling.
#[derive(Component)]
pub struct Traffic {
    /// Monotonic spawn identity. Unlike `Entity`, this is assigned by this
    /// module's deterministic spawn stream and is never query-order derived.
    id: u64,
    pub(crate) speed: f32,
    pub(crate) speed_roll: f32,
    pub(crate) velocity: Vec2,
    connector: LaneConnector,
    distance: f32,
    route_rng: u32,
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

#[derive(Clone, Copy, Debug)]
struct TrafficSnapshot {
    id: u64,
    connector: LaneConnector,
    distance: f32,
    speed: f32,
    cruise_speed: f32,
    route_rng: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TrafficPlan {
    id: u64,
    speed: f32,
}

#[derive(Clone, Copy, Debug)]
struct JunctionCandidate {
    snapshot_index: usize,
    inside: bool,
    distance_to_entry: f32,
    eta: f32,
}

fn same_connector(a: LaneConnector, b: LaneConnector) -> bool {
    a.cell == b.cell && a.slot == b.slot
}

fn junction_center(connector: LaneConnector) -> Vec2 {
    connector.cell.as_vec2() * ROAD_BLOCK_SIZE
}

fn in_junction(connector: LaneConnector, position: Vec2) -> bool {
    (position - junction_center(connector)).abs().max_element() <= JUNCTION_HALF_EXTENT
}

/// Conservative sampled entry/exit distances for the central conflict area.
/// U-turns at road stubs normally never enter it and therefore remain free to
/// reverse rather than waiting forever for a reservation they do not need.
fn junction_interval(connector: LaneConnector) -> Option<(f32, f32)> {
    let mut previous = connector.curve.eval(0.0);
    let mut traversed = 0.0;
    let mut entry = None;
    let mut exit = 0.0;
    for step in 1..=CURVE_LENGTH_SAMPLES {
        let point = connector
            .curve
            .eval(step as f32 / CURVE_LENGTH_SAMPLES as f32);
        let next_distance = traversed + previous.distance(point);
        if in_junction(connector, previous) || in_junction(connector, point) {
            entry.get_or_insert(traversed);
            exit = next_distance;
        }
        traversed = next_distance;
        previous = point;
    }
    entry.map(|entry| (entry, exit))
}

/// Constant-time conflict policy. Connector masks encode all movement geometry;
/// connectors in different cells never contend.
fn connectors_conflict(a: LaneConnector, b: LaneConnector) -> bool {
    a.cell == b.cell && a.conflict_mask & (1_u16 << b.slot) != 0
}

fn approach(value: f32, target: f32, max_delta: f32) -> f32 {
    if value < target {
        (value + max_delta).min(target)
    } else {
        (value - max_delta).max(target)
    }
}

/// Immutable snapshot -> deterministic plans. The returned vector is sorted
/// by stable traffic ID, so both planning and application ignore ECS order.
fn plan_traffic(snapshots: &[TrafficSnapshot], delta_seconds: f32) -> Vec<TrafficPlan> {
    let dt = if delta_seconds.is_finite() {
        delta_seconds.max(0.0)
    } else {
        0.0
    };
    let mut candidates = Vec::with_capacity(MAX_TRAFFIC);
    for (index, snapshot) in snapshots.iter().enumerate() {
        let Some((entry, exit)) = junction_interval(snapshot.connector) else {
            continue;
        };
        // Occupancy is body-aware: acquire when the front bumper reaches
        // the central area and retain until the rear bumper has cleared it.
        let stop_line = (entry - TRAFFIC_HALF_LENGTH).max(0.0);
        if snapshot.distance > exit + TRAFFIC_HALF_LENGTH {
            continue;
        }
        let inside = snapshot.distance > stop_line + JUNCTION_STOP_MARGIN;
        let distance_to_entry = (stop_line - snapshot.distance).max(0.0);
        if inside || distance_to_entry <= JUNCTION_APPROACH_DISTANCE {
            candidates.push(JunctionCandidate {
                snapshot_index: index,
                inside,
                distance_to_entry,
                eta: distance_to_entry / snapshot.speed.max(0.1),
            });
        }
    }
    candidates.sort_by(|a, b| {
        b.inside
            .cmp(&a.inside)
            .then_with(|| a.eta.total_cmp(&b.eta))
            .then_with(|| a.distance_to_entry.total_cmp(&b.distance_to_entry))
            .then_with(|| {
                snapshots[a.snapshot_index]
                    .id
                    .cmp(&snapshots[b.snapshot_index].id)
            })
    });

    let mut granted = Vec::with_capacity(MAX_TRAFFIC);
    for candidate in &candidates {
        let connector = snapshots[candidate.snapshot_index].connector;
        if granted
            .iter()
            .all(|&other: &usize| !connectors_conflict(connector, snapshots[other].connector))
        {
            granted.push(candidate.snapshot_index);
        }
    }

    let mut plans = Vec::with_capacity(snapshots.len());
    for (index, snapshot) in snapshots.iter().enumerate() {
        let mut target_speed = snapshot.cruise_speed.max(0.0);

        // Find the nearest leader either on this connector or on the exact
        // connector this car will choose next. Copying route_rng is important:
        // planning never consumes route state.
        let mut route_rng = snapshot.route_rng;
        let next = next_connector(snapshot.connector, &mut route_rng);
        let mut nearest: Option<(f32, f32)> = None;
        for leader in snapshots {
            if leader.id == snapshot.id {
                continue;
            }
            let center_distance = if same_connector(snapshot.connector, leader.connector)
                && leader.distance > snapshot.distance
            {
                Some(leader.distance - snapshot.distance)
            } else if next.is_some_and(|next| same_connector(next, leader.connector)) {
                Some(snapshot.connector.curve.length() - snapshot.distance + leader.distance)
            } else {
                None
            };
            if let Some(center_distance) = center_distance {
                let clearance = center_distance - 2.0 * TRAFFIC_HALF_LENGTH - MIN_BUMPER_GAP;
                if nearest.is_none_or(|current| clearance < current.0) {
                    nearest = Some((clearance, leader.speed.max(0.0)));
                }
            }
        }
        if let Some((clearance, leader_speed)) = nearest {
            // Kinematic following target: enough room to match the leader at
            // bounded deceleration, plus a hard one-step guard below.
            target_speed = target_speed.min(
                (leader_speed * leader_speed + 2.0 * TRAFFIC_DECELERATION * clearance.max(0.0))
                    .sqrt(),
            );
        }

        if let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate.snapshot_index == index)
        {
            if !candidate.inside && !granted.contains(&index) {
                target_speed = target_speed
                    .min((2.0 * TRAFFIC_DECELERATION * candidate.distance_to_entry).sqrt());
            }
        }

        let max_delta = if target_speed < snapshot.speed {
            TRAFFIC_DECELERATION * dt
        } else {
            TRAFFIC_ACCELERATION * dt
        };
        let mut speed = approach(snapshot.speed.max(0.0), target_speed, max_delta);

        // Numerical/sudden-state safety guards. Normal approaches decelerate
        // smoothly; these only prevent this frame's travel consuming the last
        // legal bumper clearance or crossing a denied stop line.
        if dt > f32::EPSILON {
            if let Some((clearance, _)) = nearest {
                // Assume the leader could stop this frame. This conservative
                // synchronous clamp makes the minimum gap invariant even when
                // the leader's own plan changes sharply at a junction.
                speed = speed.min((clearance.max(0.0) / dt).max(0.0));
            }
            if let Some(candidate) = candidates
                .iter()
                .find(|candidate| candidate.snapshot_index == index)
            {
                if !candidate.inside && !granted.contains(&index) {
                    speed = speed.min(
                        ((candidate.distance_to_entry - JUNCTION_STOP_MARGIN).max(0.0) / dt)
                            .max(0.0),
                    );
                }
            }
        }
        plans.push(TrafficPlan {
            id: snapshot.id,
            speed: speed.max(0.0),
        });
    }
    plans.sort_by_key(|plan| plan.id);
    plans
}

/// Snapshot, plan, then apply moving traffic. Stable IDs, immutable planning,
/// longitudinal headway, and deterministic geometric junction grants make the
/// result independent of ECS query iteration order.
fn manage_traffic(
    mut commands: Commands,
    assets: Res<TrafficAssets>,
    difficulty: Res<Difficulty>,
    modifier: Res<ActiveModifier>,
    event: Res<ActiveEvent>,
    car: Query<&Transform, (With<Car>, Without<Traffic>)>,
    mut traffic_query: Query<
        (Entity, &mut Traffic, &mut Transform, &mut Collider),
        (With<Traffic>, Without<TrafficWheel>, Without<Car>),
    >,
    time: Res<Time>,
    mut seed: Local<u32>,
    mut next_traffic_id: Local<u64>,
) {
    ensure_seeded(&mut seed, 0x0BADC0DE);
    let Ok(car_t) = car.single() else {
        return;
    };
    let car_pos = car_t.translation.xz();
    let dt = time.delta_secs();
    let modifier_speed = modifier.traffic_speed_multiplier();
    let event_speed = event.traffic_speed_multiplier();
    let target = target_traffic_count(
        difficulty.level,
        modifier.traffic_count_multiplier(),
        event.traffic_count_multiplier(),
    );

    let mut snapshots = Vec::with_capacity(MAX_TRAFFIC);
    for (_, traffic, _, _) in &mut traffic_query {
        snapshots.push(TrafficSnapshot {
            id: traffic.id,
            connector: traffic.connector,
            distance: traffic.distance,
            speed: traffic.speed,
            cruise_speed: traffic_speed_for_roll(
                difficulty.level,
                traffic.speed_roll,
                modifier_speed,
                event_speed,
            ),
            route_rng: traffic.route_rng,
        });
    }
    snapshots.sort_by_key(|snapshot| snapshot.id);
    let plans = plan_traffic(&snapshots, dt);

    let mut to_despawn = Vec::with_capacity(MAX_TRAFFIC);
    let mut survivors = Vec::with_capacity(MAX_TRAFFIC);
    let mut occupied = Vec::with_capacity(MAX_TRAFFIC);
    for (entity, mut traffic, mut transform, mut collider) in &mut traffic_query {
        let Ok(plan_index) = plans.binary_search_by_key(&traffic.id, |plan| plan.id) else {
            continue;
        };
        traffic.speed = plans[plan_index].speed;
        if !advance_traffic(&mut traffic, &mut transform, &mut collider, dt) {
            to_despawn.push(entity);
            continue;
        }

        let position = transform.translation.xz();
        let delta = position - car_pos;
        let distance_squared = delta.length_squared();
        if !distance_squared.is_finite() || delta.abs().max_element() > TRAFFIC_KEEP_HALF_EXTENT {
            to_despawn.push(entity);
            continue;
        }
        let half_extents = Vec2::new(collider.half_x, collider.half_z);
        survivors.push((
            entity,
            distance_squared,
            aabb_off_gameplay_view(car_pos, position, half_extents),
        ));
        occupied.push((position, half_extents));
    }

    // A reduced target never pops a visible car. Only the farthest currently
    // offscreen survivors are eligible; any remaining surplus naturally
    // persists until it leaves the conservative gameplay envelope.
    let surplus = traffic_surplus(survivors.len(), target);
    if surplus > 0 {
        let mut eligible: Vec<_> = survivors
            .iter()
            .filter(|(_, _, offscreen)| *offscreen)
            .map(|(entity, distance, _)| (*entity, *distance))
            .collect();
        eligible.sort_by(|(entity_a, distance_a), (entity_b, distance_b)| {
            traffic_despawn_order(
                (entity_a.to_bits(), *distance_a),
                (entity_b.to_bits(), *distance_b),
            )
        });
        to_despawn.extend(eligible.into_iter().take(surplus).map(|(entity, _)| entity));
    }
    to_despawn.sort_by_key(|entity| entity.to_bits());
    to_despawn.dedup();
    let alive = survivors
        .iter()
        .filter(|(entity, _, _)| !to_despawn.contains(entity))
        .count();
    for entity in &to_despawn {
        commands.entity(*entity).despawn();
    }

    let spawn_connectors = traffic_spawn_connectors(car_pos);
    for _ in 0..target.saturating_sub(alive) {
        let speed_roll = rand(&mut seed);
        let speed =
            traffic_speed_for_roll(difficulty.level, speed_roll, modifier_speed, event_speed);
        let Some(candidate) =
            traffic_spawn_candidate(car_pos, &spawn_connectors, &occupied, &mut seed)
        else {
            continue;
        };
        occupied.push((candidate.position, candidate.half_extents));
        if *next_traffic_id == 0 {
            *next_traffic_id = 1;
        }
        let traffic_id = *next_traffic_id;
        *next_traffic_id = next_traffic_id.saturating_add(1);
        spawn_one_traffic(
            &mut commands,
            &assets,
            candidate,
            speed,
            speed_roll,
            traffic_id,
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

fn opposite_edge(edge: LaneEdge) -> LaneEdge {
    match edge {
        LaneEdge::W => LaneEdge::E,
        LaneEdge::E => LaneEdge::W,
        LaneEdge::S => LaneEdge::N,
        LaneEdge::N => LaneEdge::S,
    }
}

fn adjacent_cell(cell: IVec2, exit: LaneEdge) -> Option<IVec2> {
    match exit {
        LaneEdge::W => cell.x.checked_sub(1).map(|x| IVec2::new(x, cell.y)),
        LaneEdge::E => cell.x.checked_add(1).map(|x| IVec2::new(x, cell.y)),
        LaneEdge::S => cell.y.checked_sub(1).map(|y| IVec2::new(cell.x, y)),
        LaneEdge::N => cell.y.checked_add(1).map(|y| IVec2::new(cell.x, y)),
    }
}

/// Select an outbound movement from the cell across an exit edge. Straight is
/// weighted strongly over left/right. U-turn connectors are excluded whenever
/// any non-U-turn route exists, and therefore occur only at a graph stub/dead
/// end. Stable connector slots plus per-car RNG make this query-order invariant.
fn next_connector(connector: LaneConnector, route_rng: &mut u32) -> Option<LaneConnector> {
    let cell = adjacent_cell(connector.cell, connector.to)?;
    let inbound = opposite_edge(connector.to);
    let plan = road_plan(cell.x, cell.y);
    let mut regular = Vec::new();
    let mut u_turn = None;
    for candidate in plan.connectors.into_iter().flatten() {
        if candidate.from != inbound {
            continue;
        }
        if candidate.turn == LaneTurn::UTurn {
            u_turn = Some(candidate);
        } else {
            regular.push(candidate);
        }
    }
    if regular.is_empty() {
        return u_turn;
    }
    regular.sort_by_key(|candidate| candidate.slot);
    let total_weight: u32 = regular
        .iter()
        .map(|candidate| match candidate.turn {
            LaneTurn::Straight => 8,
            LaneTurn::Left | LaneTurn::Right => 2,
            LaneTurn::UTurn => 0,
        })
        .sum();
    let mut roll = next_random_u32(route_rng) % total_weight;
    for candidate in regular {
        let weight = if candidate.turn == LaneTurn::Straight {
            8
        } else {
            2
        };
        if roll < weight {
            return Some(candidate);
        }
        roll -= weight;
    }
    None
}

/// Arc-length lookup returning a point and tangent at a distance. Fixed linear
/// samples give deterministic distance motion and make partitioned frame deltas
/// agree up to the graph's sampling tolerance.
fn curve_at_distance(curve: LaneCurve, distance: f32) -> Option<(Vec2, Vec2)> {
    if !distance.is_finite() {
        return None;
    }
    let total = curve.sampled_length_with_steps(CURVE_LENGTH_SAMPLES);
    if !total.is_finite() || total <= f32::EPSILON {
        return None;
    }
    let target = distance.clamp(0.0, total);
    let mut previous = curve.eval(0.0);
    let mut traversed = 0.0;
    for step in 1..=CURVE_LENGTH_SAMPLES {
        let t = step as f32 / CURVE_LENGTH_SAMPLES as f32;
        let point = curve.eval(t);
        let segment_length = previous.distance(point);
        if !segment_length.is_finite() {
            return None;
        }
        if traversed + segment_length >= target {
            let local = if segment_length > f32::EPSILON {
                (target - traversed) / segment_length
            } else {
                0.0
            };
            let sample_t = (step - 1) as f32 / CURVE_LENGTH_SAMPLES as f32
                + local / CURVE_LENGTH_SAMPLES as f32;
            let tangent = curve.tangent(sample_t);
            return (tangent.length_squared() > 0.5 && tangent.is_finite())
                .then_some((previous.lerp(point, local), tangent));
        }
        traversed += segment_length;
        previous = point;
    }
    let tangent = curve.tangent(1.0);
    (tangent.length_squared() > 0.5 && tangent.is_finite())
        .then_some((curve.control_points[3], tangent))
}

fn traffic_half_extents(tangent: Vec2) -> Vec2 {
    let tangent = tangent.normalize_or_zero().abs();
    Vec2::new(
        tangent.y * TRAFFIC_HALF_WIDTH + tangent.x * TRAFFIC_HALF_LENGTH,
        tangent.x * TRAFFIC_HALF_WIDTH + tangent.y * TRAFFIC_HALF_LENGTH,
    )
}

fn apply_traffic_pose(
    traffic: &mut Traffic,
    transform: &mut Transform,
    collider: &mut Collider,
) -> bool {
    let Some((position, tangent)) = curve_at_distance(traffic.connector.curve, traffic.distance)
    else {
        traffic.velocity = Vec2::ZERO;
        return false;
    };
    if !traffic.speed.is_finite() || traffic.speed < 0.0 {
        traffic.velocity = Vec2::ZERO;
        return false;
    }
    traffic.velocity = tangent * traffic.speed;
    transform.translation.x = position.x;
    transform.translation.z = position.y;
    transform.rotation = Quat::from_rotation_y((-tangent.x).atan2(-tangent.y));
    let half = traffic_half_extents(tangent);
    collider.half_x = half.x;
    collider.half_z = half.y;
    transform.translation.is_finite() && traffic.velocity.is_finite() && half.is_finite()
}

fn advance_traffic(
    traffic: &mut Traffic,
    transform: &mut Transform,
    collider: &mut Collider,
    delta_seconds: f32,
) -> bool {
    if !delta_seconds.is_finite() || delta_seconds < 0.0 || !traffic.distance.is_finite() {
        traffic.velocity = Vec2::ZERO;
        return false;
    }
    let mut remaining = traffic.speed * delta_seconds;
    if !remaining.is_finite() {
        traffic.velocity = Vec2::ZERO;
        return false;
    }
    for transition in 0..=MAX_CONNECTOR_TRANSITIONS_PER_FRAME {
        let length = traffic.connector.curve.length();
        if !length.is_finite()
            || length <= f32::EPSILON
            || traffic.distance < 0.0
            || traffic.distance > length + 1e-3
        {
            traffic.velocity = Vec2::ZERO;
            return false;
        }
        traffic.distance = traffic.distance.min(length);
        let available = length - traffic.distance;
        if remaining <= available {
            traffic.distance += remaining;
            return apply_traffic_pose(traffic, transform, collider);
        }
        if transition == MAX_CONNECTOR_TRANSITIONS_PER_FRAME {
            traffic.velocity = Vec2::ZERO;
            return false;
        }
        remaining -= available;
        let Some(next) = next_connector(traffic.connector, &mut traffic.route_rng) else {
            traffic.velocity = Vec2::ZERO;
            return false;
        };
        traffic.connector = next;
        traffic.distance = 0.0;
    }
    traffic.velocity = Vec2::ZERO;
    false
}

fn aabb_off_gameplay_view(car_pos: Vec2, position: Vec2, half: Vec2) -> bool {
    let delta = (position - car_pos).abs();
    delta.x - half.x > GAMEPLAY_GROUND_HALF_EXTENT || delta.y - half.y > GAMEPLAY_GROUND_HALF_EXTENT
}

fn aabb_overlaps(a_position: Vec2, a_half: Vec2, b_position: Vec2, b_half: Vec2) -> bool {
    let delta = (a_position - b_position).abs();
    delta.x < a_half.x + b_half.x && delta.y < a_half.y + b_half.y
}

#[derive(Clone, Copy, Debug)]
struct TrafficSpawnCandidate {
    position: Vec2,
    half_extents: Vec2,
    tangent: Vec2,
    connector: LaneConnector,
    distance: f32,
    route_rng: u32,
}

/// Bounded deterministic connector candidates around the player. A candidate
/// must be wholly beyond the portrait-safe gameplay bound, outside player
/// clearance, and separated from every existing traffic conservative AABB.
/// Failure is expected and simply defers target top-up to a later frame.
fn traffic_spawn_connectors(car_pos: Vec2) -> Vec<LaneConnector> {
    let center_x = world_to_road_cell(car_pos.x);
    let center_z = world_to_road_cell(car_pos.y);
    let mut connectors = Vec::with_capacity(((SPAWN_CELL_RADIUS * 2 + 1).pow(2) * 12) as usize);
    for gx in
        center_x.saturating_sub(SPAWN_CELL_RADIUS)..=center_x.saturating_add(SPAWN_CELL_RADIUS)
    {
        for gz in
            center_z.saturating_sub(SPAWN_CELL_RADIUS)..=center_z.saturating_add(SPAWN_CELL_RADIUS)
        {
            let plan = road_plan(gx, gz);
            let has_non_u_turn = plan
                .connectors
                .iter()
                .flatten()
                .any(|connector| connector.turn != LaneTurn::UTurn);
            connectors.extend(
                plan.connectors
                    .into_iter()
                    .flatten()
                    .filter(|connector| !has_non_u_turn || connector.turn != LaneTurn::UTurn),
            );
        }
    }
    connectors
}

fn traffic_spawn_candidate(
    car_pos: Vec2,
    connectors: &[LaneConnector],
    occupied: &[(Vec2, Vec2)],
    seed: &mut u32,
) -> Option<TrafficSpawnCandidate> {
    if !car_pos.is_finite() || connectors.is_empty() {
        return None;
    }

    for _ in 0..SPAWN_RETRY_CANDIDATES {
        let index = (next_random_u32(seed) as usize) % connectors.len();
        let connector = connectors[index];
        let length = connector.curve.length();
        if !length.is_finite() || length <= 2.0 * TRAFFIC_HALF_LENGTH {
            continue;
        }
        let distance = TRAFFIC_HALF_LENGTH + rand(seed) * (length - 2.0 * TRAFFIC_HALF_LENGTH);
        let Some((position, tangent)) = curve_at_distance(connector.curve, distance) else {
            continue;
        };
        let half_extents = traffic_half_extents(tangent);
        if !aabb_off_gameplay_view(car_pos, position, half_extents)
            || in_junction(connector, position)
            || (position - car_pos).abs().max_element() > TRAFFIC_KEEP_HALF_EXTENT
            || position.distance_squared(car_pos) < SPAWN_SAFE_RADIUS.powi(2)
            || occupied.iter().any(|&(other_position, other_half)| {
                aabb_overlaps(position, half_extents, other_position, other_half)
            })
        {
            continue;
        }
        return Some(TrafficSpawnCandidate {
            position,
            half_extents,
            tangent,
            connector,
            distance,
            route_rng: next_random_u32(seed).max(1),
        });
    }
    None
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

/// Spawn one top-level traffic car. The root front (-Z), stored velocity, and
/// conservative collider all derive from the same connector tangent.
fn spawn_one_traffic(
    commands: &mut Commands,
    assets: &TrafficAssets,
    candidate: TrafficSpawnCandidate,
    speed: f32,
    speed_roll: f32,
    traffic_id: u64,
    seed: &mut u32,
) {
    let heading = (-candidate.tangent.x).atan2(-candidate.tangent.y);
    let kind = traffic_kind(*seed);
    let kind_idx = kind.index();
    let color_idx = (rand(seed) * assets.body_mats.len() as f32) as usize % assets.body_mats.len();
    commands
        .spawn((
            Transform::from_xyz(candidate.position.x, 0.0, candidate.position.y)
                .with_rotation(Quat::from_rotation_y(heading)),
            Visibility::default(),
            Traffic {
                id: traffic_id,
                speed,
                speed_roll,
                velocity: candidate.tangent * speed,
                connector: candidate.connector,
                distance: candidate.distance,
                route_rng: candidate.route_rng,
            },
            Collider {
                half_x: candidate.half_extents.x,
                half_z: candidate.half_extents.y,
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
fn next_random_u32(seed: &mut u32) -> u32 {
    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
    *seed
}

fn rand(seed: &mut u32) -> f32 {
    next_random_u32(seed) as f32 / u32::MAX as f32
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

    fn first_crossing_with_turn(turn: LaneTurn) -> LaneConnector {
        for gx in -20..=20 {
            for gz in -20..=20 {
                let plan = road_plan(gx, gz);
                if let Some(connector) = plan
                    .connectors
                    .into_iter()
                    .flatten()
                    .find(|connector| connector.turn == turn)
                {
                    return connector;
                }
            }
        }
        panic!("no connector with requested turn")
    }

    fn source_with_next_turns(required: &[LaneTurn]) -> LaneConnector {
        for gx in -40..=40 {
            for gz in -40..=40 {
                for source in road_plan(gx, gz).connectors.into_iter().flatten() {
                    let cell = adjacent_cell(source.cell, source.to).unwrap();
                    let inbound = opposite_edge(source.to);
                    let choices: Vec<_> = road_plan(cell.x, cell.y)
                        .connectors
                        .into_iter()
                        .flatten()
                        .filter(|candidate| {
                            candidate.from == inbound && candidate.turn != LaneTurn::UTurn
                        })
                        .collect();
                    if choices.len() == required.len()
                        && required
                            .iter()
                            .all(|turn| choices.iter().any(|choice| choice.turn == *turn))
                    {
                        return source;
                    }
                }
            }
        }
        panic!("no transition with requested outbound turns")
    }

    #[test]
    fn route_selection_is_deterministic_prefers_regular_and_uses_stub_uturn() {
        let source = source_with_next_turns(&[LaneTurn::Straight, LaneTurn::Left, LaneTurn::Right]);
        let mut seed_a = 0x1234_5678;
        let mut seed_b = seed_a;
        assert_eq!(
            next_connector(source, &mut seed_a),
            next_connector(source, &mut seed_b)
        );
        assert_eq!(seed_a, seed_b);

        // Across many real crossings, U-turn is never selected when another
        // movement exists. Straight's 8:2:2 weighting also dominates turns.
        let mut counts = [0_usize; 4];
        for seed in 1..=512_u32 {
            let mut route_seed = seed;
            if let Some(next) = next_connector(source, &mut route_seed) {
                counts[match next.turn {
                    LaneTurn::Straight => 0,
                    LaneTurn::Left => 1,
                    LaneTurn::Right => 2,
                    LaneTurn::UTurn => 3,
                }] += 1;
            }
        }
        assert_eq!(counts[3], 0);
        assert!(counts[0] > counts[1] && counts[0] > counts[2]);

        // Find a real transition whose adjacent tile is a stub. Its only legal
        // continuation is an explicit U-turn.
        let mut found_stub = false;
        'cells: for gx in -40..=40 {
            for gz in -40..=40 {
                for connector in road_plan(gx, gz).connectors.into_iter().flatten() {
                    let cell = adjacent_cell(connector.cell, connector.to).unwrap();
                    let inbound = opposite_edge(connector.to);
                    let choices: Vec<_> = road_plan(cell.x, cell.y)
                        .connectors
                        .into_iter()
                        .flatten()
                        .filter(|candidate| candidate.from == inbound)
                        .collect();
                    if choices.len() == 1 && choices[0].turn == LaneTurn::UTurn {
                        assert_eq!(next_connector(connector, &mut 7), Some(choices[0]));
                        found_stub = true;
                        break 'cells;
                    }
                }
            }
        }
        assert!(found_stub);
    }

    fn traffic_on(connector: LaneConnector, speed: f32) -> (Traffic, Transform, Collider) {
        let mut traffic = Traffic {
            id: 1,
            speed,
            speed_roll: 0.5,
            velocity: Vec2::ZERO,
            connector,
            distance: 0.0,
            route_rng: 0xCAFE_BABE,
        };
        let mut transform = Transform::default();
        let mut collider = Collider {
            half_x: 0.0,
            half_z: 0.0,
        };
        assert!(apply_traffic_pose(
            &mut traffic,
            &mut transform,
            &mut collider
        ));
        (traffic, transform, collider)
    }

    #[test]
    fn route_extends_multiple_cells_beyond_streamed_five_by_five() {
        let connector = source_with_next_turns(&[LaneTurn::Straight]);
        let mut extended = None;
        for route_rng in 1..=256 {
            let (mut traffic, mut transform, mut collider) = traffic_on(connector, 10.0);
            traffic.route_rng = route_rng;
            let start_cell = traffic.connector.cell;
            let mut furthest = 0;
            for _ in 0..400 {
                assert!(advance_traffic(
                    &mut traffic,
                    &mut transform,
                    &mut collider,
                    0.25
                ));
                furthest = furthest.max((traffic.connector.cell - start_cell).abs().max_element());
            }
            if furthest > 2 {
                extended = Some((transform, traffic.velocity));
                break;
            }
        }
        let (transform, velocity) = extended.expect("a deterministic route leaves the 5x5 stream");
        assert!(transform.translation.is_finite() && velocity.is_finite());
    }

    #[test]
    fn connector_transitions_cover_straight_corner_t_and_cross() {
        for required in [
            vec![LaneTurn::Straight],
            vec![LaneTurn::Left],
            vec![LaneTurn::Straight, LaneTurn::Left],
            vec![LaneTurn::Straight, LaneTurn::Left, LaneTurn::Right],
        ] {
            let source = source_with_next_turns(&required);
            let mut selected = Vec::new();
            for seed in 1..=256_u32 {
                let mut rng = seed.wrapping_mul(0x9E37_79B9);
                if let Some(connector) = next_connector(source, &mut rng) {
                    if !selected.contains(&connector.turn) {
                        selected.push(connector.turn);
                    }
                }
            }
            assert!(
                required.iter().all(|turn| selected.contains(turn)),
                "required {required:?}, selected {selected:?}"
            );
            assert!(!selected.contains(&LaneTurn::UTurn));
        }
    }

    #[test]
    fn distance_partition_yaw_velocity_and_collider_agree() {
        let connector = first_crossing_with_turn(LaneTurn::Left);
        let (mut one, mut one_tf, mut one_box) = traffic_on(connector, 7.0);
        let (mut split, mut split_tf, mut split_box) = traffic_on(connector, 7.0);
        assert!(advance_traffic(&mut one, &mut one_tf, &mut one_box, 1.0));
        for _ in 0..10 {
            assert!(advance_traffic(
                &mut split,
                &mut split_tf,
                &mut split_box,
                0.1
            ));
        }
        assert!(one_tf.translation.distance(split_tf.translation) < 2e-3);
        assert!(one.velocity.distance(split.velocity) < 2e-3);
        let root_forward = (one_tf.rotation * Vec3::NEG_Z).xz();
        assert!(root_forward.dot(one.velocity.normalize()) > 0.999);
        let expected = traffic_half_extents(one.velocity.normalize());
        assert!((one_box.half_x - expected.x).abs() < 1e-5);
        assert!((one_box.half_z - expected.y).abs() < 1e-5);
    }

    #[test]
    fn spawn_is_deterministic_offscreen_and_rejects_player_or_traffic_overlap() {
        let car = Vec2::new(17.25, -81.5);
        for initial_seed in [1, 2, 0x1234_5678, 0xCAFE_BABE, u32::MAX] {
            let mut seed_a = initial_seed;
            let mut seed_b = initial_seed;
            let connectors = traffic_spawn_connectors(car);
            let a = traffic_spawn_candidate(car, &connectors, &[], &mut seed_a);
            let b = traffic_spawn_candidate(car, &connectors, &[], &mut seed_b);
            assert_eq!(
                a.map(|candidate| (candidate.connector, candidate.distance)),
                b.map(|candidate| (candidate.connector, candidate.distance))
            );
            assert_eq!(seed_a, seed_b);
            if let Some(candidate) = a {
                assert!(aabb_off_gameplay_view(
                    car,
                    candidate.position,
                    candidate.half_extents
                ));
                assert!(candidate.position.distance(car) >= SPAWN_SAFE_RADIUS);
                let occupied = [(candidate.position, candidate.half_extents)];
                let mut blocked_seed = initial_seed;
                let blocked =
                    traffic_spawn_candidate(car, &connectors, &occupied, &mut blocked_seed);
                if let Some(other) = blocked {
                    assert!(!aabb_overlaps(
                        other.position,
                        other.half_extents,
                        candidate.position,
                        candidate.half_extents
                    ));
                }
            }
        }
        assert!(!aabb_off_gameplay_view(Vec2::ZERO, Vec2::ZERO, Vec2::ONE));
    }

    #[test]
    fn visible_route_and_surplus_remain_inside_retention_bound() {
        let car = Vec2::new(12.0, -8.0);
        let half = Vec2::new(TRAFFIC_HALF_LENGTH, TRAFFIC_HALF_WIDTH);
        let visible = car + Vec2::new(GAMEPLAY_GROUND_HALF_EXTENT + half.x - 0.1, 0.0);
        assert!(!aabb_off_gameplay_view(car, visible, half));
        assert!((visible - car).abs().max_element() < TRAFFIC_KEEP_HALF_EXTENT);
        assert_eq!(traffic_surplus(4, 1), 3);
        // Runtime surplus eligibility uses this exact predicate, so all three
        // visible surplus entities remain until they clear the view envelope.
        let eligible = [visible; 3]
            .into_iter()
            .filter(|position| aabb_off_gameplay_view(car, *position, half))
            .count();
        assert_eq!(eligible, 0);
    }

    #[test]
    fn coordinate_edges_retire_without_integer_overflow() {
        assert_eq!(adjacent_cell(IVec2::new(i32::MAX, 0), LaneEdge::E), None);
        assert_eq!(adjacent_cell(IVec2::new(i32::MIN, 0), LaneEdge::W), None);
        assert_eq!(adjacent_cell(IVec2::new(0, i32::MAX), LaneEdge::N), None);
        assert_eq!(adjacent_cell(IVec2::new(0, i32::MIN), LaneEdge::S), None);
    }

    #[test]
    fn malformed_routes_fail_finitely() {
        let valid = first_crossing_with_turn(LaneTurn::Straight);
        let malformed = LaneConnector {
            curve: LaneCurve::new(Vec2::NAN, Vec2::ZERO, Vec2::ZERO, Vec2::ZERO),
            ..valid
        };
        let (mut traffic, mut transform, mut collider) = traffic_on(valid, 5.0);
        traffic.connector = malformed;
        assert!(!advance_traffic(
            &mut traffic,
            &mut transform,
            &mut collider,
            0.1
        ));
        assert_eq!(traffic.velocity, Vec2::ZERO);
        traffic.connector = valid;
        traffic.distance = valid.curve.length() + 1.0;
        assert!(!advance_traffic(
            &mut traffic,
            &mut transform,
            &mut collider,
            0.0
        ));
        traffic.distance = 0.0;
        assert!(!advance_traffic(
            &mut traffic,
            &mut transform,
            &mut collider,
            f32::NAN
        ));
    }

    fn planning_snapshot(
        id: u64,
        connector: LaneConnector,
        distance: f32,
        speed: f32,
        cruise_speed: f32,
    ) -> TrafficSnapshot {
        TrafficSnapshot {
            id,
            connector,
            distance,
            speed,
            cruise_speed,
            route_rng: 0x1234_5678,
        }
    }

    const REFERENCE_CONFLICT_PATH_CLEARANCE: f32 = 1.35;
    const REFERENCE_CONFLICT_SAMPLES: usize = 32;

    fn reference_orientation(a: Vec2, b: Vec2, c: Vec2) -> f32 {
        (b - a).perp_dot(c - a)
    }

    fn reference_segments_cross(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> bool {
        const EPSILON: f32 = 1e-4;
        let ab0 = reference_orientation(a0, a1, b0);
        let ab1 = reference_orientation(a0, a1, b1);
        let ba0 = reference_orientation(b0, b1, a0);
        let ba1 = reference_orientation(b0, b1, a1);
        ((ab0 > EPSILON && ab1 < -EPSILON) || (ab0 < -EPSILON && ab1 > EPSILON))
            && ((ba0 > EPSILON && ba1 < -EPSILON) || (ba0 < -EPSILON && ba1 > EPSILON))
    }

    /// Test-only copy of the sampled geometric policy from before conflict
    /// masks. This deliberately remains independent of the runtime lookup.
    fn sampled_connectors_conflict(a: LaneConnector, b: LaneConnector) -> bool {
        if a.cell != b.cell {
            return false;
        }
        if same_connector(a, b) {
            return true;
        }
        let center = junction_center(a);
        let mut a0 = a.curve.eval(0.0);
        for ai in 1..=REFERENCE_CONFLICT_SAMPLES {
            let a1 = a.curve.eval(ai as f32 / REFERENCE_CONFLICT_SAMPLES as f32);
            let a_central = ((a0 + a1) * 0.5 - center).abs().max_element()
                <= JUNCTION_HALF_EXTENT + REFERENCE_CONFLICT_PATH_CLEARANCE;
            if a_central {
                let mut b0 = b.curve.eval(0.0);
                for bi in 1..=REFERENCE_CONFLICT_SAMPLES {
                    let b1 = b.curve.eval(bi as f32 / REFERENCE_CONFLICT_SAMPLES as f32);
                    let b_central = ((b0 + b1) * 0.5 - center).abs().max_element()
                        <= JUNCTION_HALF_EXTENT + REFERENCE_CONFLICT_PATH_CLEARANCE;
                    if b_central
                        && (reference_segments_cross(a0, a1, b0, b1)
                            || a0.distance(b0) <= REFERENCE_CONFLICT_PATH_CLEARANCE
                            || a0.distance(b1) <= REFERENCE_CONFLICT_PATH_CLEARANCE
                            || a1.distance(b0) <= REFERENCE_CONFLICT_PATH_CLEARANCE
                            || a1.distance(b1) <= REFERENCE_CONFLICT_PATH_CLEARANCE)
                    {
                        return true;
                    }
                    b0 = b1;
                }
            }
            a0 = a1;
        }
        false
    }

    fn representative_cross_connectors() -> [LaneConnector; 16] {
        let plan = road_plan(0, 0);
        assert_eq!(plan.kind, crate::world::TileKind::Cross);
        plan.connectors.map(Option::unwrap)
    }

    fn translated_connector(mut connector: LaneConnector, cell: IVec2) -> LaneConnector {
        let delta = (cell - connector.cell).as_vec2() * ROAD_BLOCK_SIZE;
        connector.cell = cell;
        connector.curve.control_points = connector.curve.control_points.map(|point| point + delta);
        connector
    }

    fn crossing_pair(conflicting: bool) -> (LaneConnector, LaneConnector) {
        for gx in -30..=30 {
            for gz in -30..=30 {
                let connectors: Vec<_> =
                    road_plan(gx, gz).connectors.into_iter().flatten().collect();
                for (index, &a) in connectors.iter().enumerate() {
                    for &b in connectors.iter().skip(index + 1) {
                        if junction_interval(a).is_some()
                            && junction_interval(b).is_some()
                            && connectors_conflict(a, b) == conflicting
                            && a.from != b.from
                        {
                            return (a, b);
                        }
                    }
                }
            }
        }
        panic!("no requested connector pair")
    }

    #[test]
    fn follower_gap_is_preserved_at_common_frame_rates_with_faster_follower() {
        let connector = first_crossing_with_turn(LaneTurn::Straight);
        for hz in [30, 60, 120] {
            let dt = 1.0 / hz as f32;
            let mut leader_distance = 14.0;
            let mut follower_distance = 14.0 - 2.0 * TRAFFIC_HALF_LENGTH - MIN_BUMPER_GAP;
            let mut leader_speed = 4.0;
            let mut follower_speed = 9.0;
            for _ in 0..(hz * 6) {
                let snapshots = [
                    planning_snapshot(2, connector, follower_distance, follower_speed, 9.0),
                    planning_snapshot(1, connector, leader_distance, leader_speed, 4.0),
                ];
                let plans = plan_traffic(&snapshots, dt);
                leader_speed = plans.iter().find(|plan| plan.id == 1).unwrap().speed;
                follower_speed = plans.iter().find(|plan| plan.id == 2).unwrap().speed;
                leader_distance += leader_speed * dt;
                follower_distance += follower_speed * dt;
                assert!(
                    leader_distance - follower_distance
                        >= 2.0 * TRAFFIC_HALF_LENGTH + MIN_BUMPER_GAP - 1e-4
                );
            }
        }
    }

    #[test]
    fn immediate_next_connector_headway_prevents_overlap() {
        let connector = source_with_next_turns(&[LaneTurn::Straight]);
        let mut rng = 0x1234_5678;
        let next = next_connector(connector, &mut rng).unwrap();
        let follower_distance = connector.curve.length() - 0.25;
        let leader_distance = 2.0 * TRAFFIC_HALF_LENGTH + MIN_BUMPER_GAP + 0.25;
        let plans = plan_traffic(
            &[
                planning_snapshot(9, connector, follower_distance, 8.0, 8.0),
                planning_snapshot(3, next, leader_distance, 0.0, 0.0),
            ],
            1.0 / 30.0,
        );
        let follower_speed = plans.iter().find(|plan| plan.id == 9).unwrap().speed;
        let clearance = connector.curve.length() - follower_distance + leader_distance
            - 2.0 * TRAFFIC_HALF_LENGTH
            - MIN_BUMPER_GAP;
        assert!(follower_speed / 30.0 <= clearance + 1e-5);
    }

    #[test]
    fn conflict_masks_equal_sampled_reference_for_all_256_pairs() {
        let connectors = representative_cross_connectors();
        for (a_slot, &a) in connectors.iter().enumerate() {
            for (b_slot, &b) in connectors.iter().enumerate() {
                let expected = sampled_connectors_conflict(a, b);
                assert_eq!(
                    connectors_conflict(a, b),
                    expected,
                    "slots {a_slot} and {b_slot}"
                );
                assert_eq!(
                    a.conflict_mask & (1_u16 << b_slot) != 0,
                    expected,
                    "literal mask {a_slot}, bit {b_slot}"
                );
            }
        }
    }

    #[test]
    fn conflict_masks_are_symmetric_and_include_every_self_conflict() {
        let connectors = representative_cross_connectors();
        for a in connectors {
            assert!(connectors_conflict(a, a), "slot {}", a.slot);
            for b in connectors {
                assert_eq!(
                    connectors_conflict(a, b),
                    connectors_conflict(b, a),
                    "slots {} and {}",
                    a.slot,
                    b.slot
                );
            }
        }
    }

    #[test]
    fn conflict_masks_are_translation_invariant() {
        let origin = representative_cross_connectors();
        let translated = origin.map(|connector| translated_connector(connector, IVec2::new(3, -2)));
        for a_slot in 0..16 {
            for b_slot in 0..16 {
                assert_eq!(
                    connectors_conflict(origin[a_slot], origin[b_slot]),
                    connectors_conflict(translated[a_slot], translated[b_slot])
                );
                assert_eq!(
                    sampled_connectors_conflict(origin[a_slot], origin[b_slot]),
                    sampled_connectors_conflict(translated[a_slot], translated[b_slot])
                );
            }
        }
    }

    #[test]
    fn conflict_masks_cover_known_crossing_and_parallel_nonconflict() {
        let connectors = representative_cross_connectors();
        // W->E and S->N cross in the central pad.
        assert!(connectors_conflict(connectors[1], connectors[11]));
        // W->E and E->W occupy separated directional lanes.
        assert!(!connectors_conflict(connectors[1], connectors[4]));
    }

    #[test]
    fn conflict_lookup_rejects_different_cells_and_ignores_inactive_slots() {
        let cross = representative_cross_connectors();
        let elsewhere = translated_connector(cross[11], IVec2::new(1, 0));
        assert!(!connectors_conflict(cross[1], elsewhere));

        // A one-road stub only activates slot 0. Its mask is nevertheless the
        // canonical slot mask; inactive socket filtering is not baked into it.
        let stub = (-50..=50)
            .flat_map(|gx| (-50..=50).map(move |gz| road_plan(gx, gz)))
            .find(|plan| {
                matches!(
                    plan.kind,
                    crate::world::TileKind::StubW
                        | crate::world::TileKind::StubE
                        | crate::world::TileKind::StubS
                        | crate::world::TileKind::StubN
                )
            })
            .expect("generated region contains a stub");
        let active = stub.connectors.into_iter().flatten().next().unwrap();
        assert_eq!(active.conflict_mask, cross[active.slot].conflict_mask);
        assert!(active.conflict_mask.count_ones() > 1);
    }

    #[test]
    fn junction_conflicts_nonconflicts_inside_priority_and_stable_tie() {
        let (a, conflicting) = crossing_pair(true);
        let (non_a, non_b) = crossing_pair(false);
        let a_entry = junction_interval(a).unwrap().0;
        let conflict_entry = junction_interval(conflicting).unwrap().0;
        let non_a_entry = junction_interval(non_a).unwrap().0;
        let non_b_entry = junction_interval(non_b).unwrap().0;

        let plans = plan_traffic(
            &[
                planning_snapshot(
                    20,
                    a,
                    (a_entry - TRAFFIC_HALF_LENGTH - 0.2).max(0.0),
                    5.0,
                    5.0,
                ),
                planning_snapshot(
                    10,
                    conflicting,
                    (conflict_entry - TRAFFIC_HALF_LENGTH - 0.2).max(0.0),
                    5.0,
                    5.0,
                ),
            ],
            0.1,
        );
        assert!(plans.iter().find(|plan| plan.id == 10).unwrap().speed > 0.0);
        assert!(plans.iter().find(|plan| plan.id == 20).unwrap().speed < 5.0);

        let plans = plan_traffic(
            &[
                planning_snapshot(1, a, a_entry + 0.1, 2.0, 2.0),
                planning_snapshot(
                    0,
                    conflicting,
                    (conflict_entry - TRAFFIC_HALF_LENGTH - 0.1).max(0.0),
                    5.0,
                    5.0,
                ),
            ],
            0.1,
        );
        assert!(plans.iter().find(|plan| plan.id == 1).unwrap().speed > 0.0);
        assert!(plans.iter().find(|plan| plan.id == 0).unwrap().speed < 5.0);

        // A geometrically independent movement can hold a simultaneous grant.
        let plans = plan_traffic(
            &[
                planning_snapshot(1, non_a, non_a_entry - 0.1, 4.0, 4.0),
                planning_snapshot(2, non_b, non_b_entry - 0.1, 4.0, 4.0),
            ],
            0.1,
        );
        assert!(plans.iter().all(|plan| plan.speed > 0.0));
    }

    #[test]
    fn planning_is_query_order_independent_and_grants_release_after_exit() {
        let (a, b) = crossing_pair(true);
        let a_entry = junction_interval(a).unwrap().0;
        let (b_entry, b_exit) = junction_interval(b).unwrap();
        let ordered = [
            planning_snapshot(5, a, a_entry - 0.5, 5.0, 5.0),
            planning_snapshot(2, b, b_entry - 0.5, 5.0, 5.0),
        ];
        let shuffled = [ordered[1], ordered[0]];
        assert_eq!(plan_traffic(&ordered, 0.1), plan_traffic(&shuffled, 0.1));

        let released = [
            planning_snapshot(5, a, a_entry - 0.1, 5.0, 5.0),
            planning_snapshot(2, b, b_exit + 0.1, 5.0, 5.0),
        ];
        let plans = plan_traffic(&released, 0.1);
        assert!(plans.iter().find(|plan| plan.id == 5).unwrap().speed > 0.0);
    }

    #[test]
    fn stub_uturn_keeps_progressing_when_uncontended() {
        let connector = first_crossing_with_turn(LaneTurn::UTurn);
        let plan = plan_traffic(&[planning_snapshot(1, connector, 0.0, 0.0, 5.0)], 0.1);
        assert!(plan[0].speed > 0.0);
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
