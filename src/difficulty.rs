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

#[cfg(test)]
use bevy::color::LinearRgba;
use bevy::gltf::GltfMaterialName;
use bevy::prelude::*;
use bevy::text::FontSize;
use std::cmp::Ordering;
use std::f32::consts::TAU;

#[cfg(any(target_arch = "wasm32", test))]
use crate::car::ToyShadowCaster;
#[cfg(target_arch = "wasm32")]
use crate::car::counter_rotated_projected_shadow_transform;
use crate::car::{Car, DrivingSet, InputFrozen};
use crate::game::SpawnSet;
use crate::game::TouchStateSet;
use crate::game::resources::{RoundActive, not_drowning};
use crate::game::state::GameState;
use crate::modifiers::ActiveModifier;
use crate::run_events::ActiveEvent;
use crate::textures::PbrDetailAssets;
use crate::touch::{
    TOUCH_LEVEL_HEIGHT, TOUCH_LEVEL_RIGHT, TOUCH_LEVEL_TOP, TOUCH_LEVEL_WIDTH, TouchControlsActive,
};
#[cfg(target_arch = "wasm32")]
use crate::toy_shading::ToyCastShadow;
use crate::toy_shading::{ToyContactShadow, ToyShadingAssets, contact_shadow_transform};
#[cfg(test)]
use crate::toy_shading::{ToyMaterialFamily, toy_material};
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

// Imported toy-car dimensions fit the established conservative collider and
// share one shadow footprint across all five authored silhouettes.
const TRAFFIC_SHADOW_FOOTPRINT: Vec2 = Vec2::new(1.38, 2.30);

// Retained only for the legacy cache regression tests below. Runtime NPCs use
// authored scenes and allocate none of this procedural geometry.
#[cfg(test)]
const BODY_W: f32 = 1.0;
#[cfg(test)]
const BODY_D: f32 = 2.0;
#[cfg(test)]
const WINDSHIELD_D: f32 = 0.03;
/// The established deterministic traffic paint palette. Imported instances
/// replace only an authored `Toy_Paint` material's base color with one entry.
const TRAFFIC_PAINT_COLORS: [Color; 5] = [
    Color::srgb(0.85, 0.12, 0.10),
    Color::srgb(0.15, 0.35, 0.85),
    Color::srgb(0.18, 0.55, 0.22),
    Color::srgb(0.78, 0.78, 0.82),
    Color::srgb(0.95, 0.65, 0.08),
];
/// Authored wheel radius shared by every imported traffic variant.
const TRAFFIC_WHEEL_RADIUS: f32 = 0.19;
#[cfg(target_arch = "wasm32")]
const TRAFFIC_SHADOW_CASTER_HEIGHT: f32 = 1.10;

/// Authored toy-car silhouettes. A fixed 20-bucket table gives these variants
/// exact 6/5/4/3/2 weights without consuming the gameplay LCG.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrafficKind {
    Sedan,
    CityVan,
    Hatchback,
    Pickup,
    Suv,
}

impl TrafficKind {
    /// Exact node-name prefix authored into this variant's glTF hierarchy.
    fn asset_prefix(self) -> &'static str {
        match self {
            Self::Sedan => "npc_toy_sedan",
            Self::CityVan => "npc_toy_city_van",
            Self::Hatchback => "npc_toy_hatchback",
            Self::Pickup => "npc_toy_pickup",
            Self::Suv => "npc_toy_suv",
        }
    }
}

const TRAFFIC_KIND_BUCKETS: [TrafficKind; 20] = [
    TrafficKind::Sedan,
    TrafficKind::Sedan,
    TrafficKind::Sedan,
    TrafficKind::Sedan,
    TrafficKind::Sedan,
    TrafficKind::Sedan,
    TrafficKind::CityVan,
    TrafficKind::CityVan,
    TrafficKind::CityVan,
    TrafficKind::CityVan,
    TrafficKind::CityVan,
    TrafficKind::Hatchback,
    TrafficKind::Hatchback,
    TrafficKind::Hatchback,
    TrafficKind::Hatchback,
    TrafficKind::Pickup,
    TrafficKind::Pickup,
    TrafficKind::Pickup,
    TrafficKind::Suv,
    TrafficKind::Suv,
];

// --- UI placement (top-right, below the minimap) ---
/// Minimap bottom edge = `minimap::PANEL_TOP (54) + MAP_SIZE (120)`; sit 8px
/// below it so the "Lv" label clears the panel.
const UI_TOP: f32 = 54.0 + 120.0 + 8.0; // 182
/// Right offset matches the minimap's `PANEL_RIGHT` (16) for alignment.
const UI_RIGHT: f32 = 16.0;

// ===========================================================================
// Resources
// ===========================================================================

const TRAFFIC_SEDAN_SCENE: &str = "models/traffic/toy/npc_toy_sedan.glb#Scene0";
const TRAFFIC_CITY_VAN_SCENE: &str = "models/traffic/toy/npc_toy_city_van.glb#Scene0";
const TRAFFIC_HATCHBACK_SCENE: &str = "models/traffic/toy/npc_toy_hatchback.glb#Scene0";
const TRAFFIC_PICKUP_SCENE: &str = "models/traffic/toy/npc_toy_pickup.glb#Scene0";
const TRAFFIC_SUV_SCENE: &str = "models/traffic/toy/npc_toy_suv.glb#Scene0";

/// All five authored traffic scenes. They are requested once and every NPC
/// wrapper clones one of these stable handles.
#[derive(Resource)]
struct TrafficVisualAssets {
    sedan: Handle<WorldAsset>,
    city_van: Handle<WorldAsset>,
    hatchback: Handle<WorldAsset>,
    pickup: Handle<WorldAsset>,
    suv: Handle<WorldAsset>,
}

impl TrafficVisualAssets {
    fn scene(&self, kind: TrafficKind) -> Handle<WorldAsset> {
        match kind {
            TrafficKind::Sedan => self.sedan.clone(),
            TrafficKind::CityVan => self.city_van.clone(),
            TrafficKind::Hatchback => self.hatchback.clone(),
            TrafficKind::Pickup => self.pickup.clone(),
            TrafficKind::Suv => self.suv.clone(),
        }
    }

    #[cfg(test)]
    fn all(&self) -> [Handle<WorldAsset>; 5] {
        [
            self.sedan.clone(),
            self.city_van.clone(),
            self.hatchback.clone(),
            self.pickup.clone(),
            self.suv.clone(),
        ]
    }
}

impl FromWorld for TrafficVisualAssets {
    fn from_world(world: &mut World) -> Self {
        let assets = world.resource::<AssetServer>();
        Self {
            sedan: assets.load(TRAFFIC_SEDAN_SCENE),
            city_van: assets.load(TRAFFIC_CITY_VAN_SCENE),
            hatchback: assets.load(TRAFFIC_HATCHBACK_SCENE),
            pickup: assets.load(TRAFFIC_PICKUP_SCENE),
            suv: assets.load(TRAFFIC_SUV_SCENE),
        }
    }
}

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
#[cfg(test)]
#[derive(Resource)]
#[allow(dead_code)]
pub struct TrafficAssets {
    /// Legacy procedural sedan/van geometry retained only for focused cache
    /// regression tests. Runtime traffic never spawns these visible meshes.
    body_meshes: [Handle<Mesh>; 2],
    cabin_meshes: [Handle<Mesh>; 2],
    windshield_meshes: [Handle<Mesh>; 2],
    light_mesh: Handle<Mesh>,
    wheel_mesh: Handle<Mesh>,
    hub_mesh: Handle<Mesh>,
    /// A small shared car-paint palette, selected at spawn.
    body_mats: [Handle<StandardMaterial>; 5],
    cabin_mat: Handle<StandardMaterial>,
    windshield_mat: Handle<StandardMaterial>,
    headlight_mat: Handle<StandardMaterial>,
    rear_light_mat: Handle<StandardMaterial>,
    wheel_mat: Handle<StandardMaterial>,
    hub_mat: Handle<StandardMaterial>,
}

#[cfg(test)]
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

            let body_mats = TRAFFIC_PAINT_COLORS.map(|base_color| {
                materials.add(toy_material(
                    ToyMaterialFamily::CoatedPlastic,
                    StandardMaterial {
                        base_color,
                        ..default()
                    },
                ))
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
            let wheel_mat = materials.add(toy_material(
                ToyMaterialFamily::Rubber,
                StandardMaterial {
                    base_color: Color::srgb(0.025, 0.025, 0.03),
                    ..default()
                },
            ));
            let hub_mat = materials.add(toy_material(
                ToyMaterialFamily::BareMetal,
                StandardMaterial {
                    base_color: Color::srgb(0.5, 0.53, 0.56),
                    ..default()
                },
            ));
            TrafficAssets {
                body_meshes,
                cabin_meshes,
                windshield_meshes,
                light_mesh,
                wheel_mesh,
                hub_mesh,
                body_mats,
                cabin_mat,
                windshield_mat,
                headlight_mat,
                rear_light_mat,
                wheel_mat,
                hub_mat,
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

/// Identity-transform wrapper that owns one asynchronously instantiated NPC
/// scene. The exact authored prefix prevents similarly named nodes from a
/// different selected asset from ever being bound.
#[derive(Component, Clone, Copy, Debug)]
struct ImportedTrafficVisual {
    asset_prefix: &'static str,
    paint_index: usize,
}

/// The sole per-instance clone of the selected owner's authored paint. The
/// source handle is never mutated; every matching primitive beneath this
/// wrapper is redirected to this handle as it arrives asynchronously.
#[derive(Component, Clone, Debug, Default)]
struct ImportedTrafficPaintMaterial(Option<Handle<StandardMaterial>>);

/// Marks a `Toy_Paint` primitive after assignment to its owner's clone.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
struct ImportedTrafficPaintOwner(Entity);

/// Per-wrapper rolling state. Keeping this separate from wheel transforms lets
/// every wheel be rebuilt from its captured authored baseline each frame.
#[derive(Component, Clone, Copy, Debug, Default)]
struct ImportedTrafficWheelAnimation {
    spin_radians: f32,
}

/// Added to the imported wrapper only while all four wheel slots are present
/// exactly once beneath that wrapper.
#[derive(Component, Clone, Copy, Debug)]
struct ImportedTrafficReady;

/// Marks an asynchronously discovered authored wheel pivot.
#[derive(Component, Clone, Copy, Debug)]
struct ImportedTrafficWheel;

/// Stable gameplay owner used to read speed without depending on the glTF's
/// intermediate hierarchy.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
struct ImportedTrafficWheelOwner(Entity);

/// Authored local orientation captured once when the wheel pivot appears.
#[derive(Component, Clone, Copy, Debug)]
struct ImportedTrafficWheelBaseline(Quat);

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
enum ImportedTrafficWheelSlot {
    Fl,
    Fr,
    Rl,
    Rr,
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
            .init_resource::<TrafficVisualAssets>()
            .init_resource::<ToyShadingAssets>()
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
            // Per-frame: ramp the level, plan/move/replenish traffic, then
            // discover and animate wheel pivots that may have arrived from the
            // asynchronous scene spawner this frame.
            .add_systems(
                Update,
                tick_difficulty
                    .run_if(in_state(GameState::Playing))
                    .run_if(not_drowning),
            )
            .add_systems(
                Update,
                manage_traffic
                    .after(tick_difficulty)
                    .before(DrivingSet)
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(
                Update,
                (
                    bind_imported_traffic_wheels,
                    bind_imported_traffic_paint,
                    update_imported_traffic_ready,
                    animate_imported_traffic_wheels,
                )
                    .chain()
                    .after(manage_traffic)
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
    visual_assets: Res<TrafficVisualAssets>,
    toy_shading: Res<ToyShadingAssets>,
    difficulty: Res<Difficulty>,
    modifier: Res<ActiveModifier>,
    event: Res<ActiveEvent>,
    car: Query<&Transform, (With<Car>, Without<Traffic>)>,
    mut traffic_query: Query<
        (Entity, &mut Traffic, &mut Transform, &mut Collider),
        (With<Traffic>, Without<Car>),
    >,
    paint_wrappers: Query<(&ChildOf, &ImportedTrafficPaintMaterial)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    time: Res<Time>,
    mut seed: Local<u32>,
    mut next_traffic_id: Local<u64>,
) {
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
        remove_traffic_paint_material(*entity, &paint_wrappers, &mut materials);
        commands.entity(*entity).despawn();
    }

    // Top-up work is deliberately deferred until survivor/despawn accounting
    // is complete. At target this avoids both rebuilding the deterministic
    // connector catalog and touching the spawn RNG. Below target, the bounded
    // policy invokes exactly one candidate attempt, so even a jump from 0 to
    // the hard cap adds at most one traffic hierarchy this frame.
    let needed = traffic_top_up_needed(alive, target);
    with_bounded_traffic_top_up(
        needed,
        || traffic_spawn_connectors(car_pos),
        |spawn_connectors| {
            ensure_seeded(&mut seed, 0x0BADC0DE);
            let speed_roll = rand(&mut seed);
            let speed =
                traffic_speed_for_roll(difficulty.level, speed_roll, modifier_speed, event_speed);
            let Some(candidate) =
                traffic_spawn_candidate(car_pos, &spawn_connectors, &occupied, &mut seed)
            else {
                return;
            };
            occupied.push((candidate.position, candidate.half_extents));
            if *next_traffic_id == 0 {
                *next_traffic_id = 1;
            }
            let traffic_id = *next_traffic_id;
            *next_traffic_id = next_traffic_id.saturating_add(1);
            spawn_one_traffic(
                &mut commands,
                &visual_assets,
                &toy_shading,
                candidate,
                speed,
                speed_roll,
                traffic_id,
                &mut seed,
            );
        },
    );
}

/// Despawn every traffic car (e.g. on GameOver / Menu). Recursive despawn in
/// 0.19 removes the imported wrapper and shadow children (safe, risk E2).
fn cleanup_traffic(
    mut commands: Commands,
    traffic: Query<Entity, With<Traffic>>,
    paint_wrappers: Query<(&ChildOf, &ImportedTrafficPaintMaterial)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for entity in &traffic {
        remove_traffic_paint_material(entity, &paint_wrappers, &mut materials);
        commands.entity(entity).despawn();
    }
}

fn remove_traffic_paint_material(
    owner: Entity,
    paint_wrappers: &Query<(&ChildOf, &ImportedTrafficPaintMaterial)>,
    materials: &mut Assets<StandardMaterial>,
) {
    for (parent, instance) in paint_wrappers {
        if parent.parent() == owner {
            if let Some(handle) = &instance.0 {
                materials.remove(handle.id());
            }
        }
    }
}

fn traffic_wheel_spin_delta(speed: f32, delta_seconds: f32) -> f32 {
    speed * delta_seconds / TRAFFIC_WHEEL_RADIUS
}

fn classify_imported_traffic_wheel(
    asset_prefix: &str,
    node_name: &str,
) -> Option<ImportedTrafficWheelSlot> {
    use ImportedTrafficWheelSlot::*;
    let suffix = node_name.strip_prefix(asset_prefix)?;
    Some(match suffix {
        "_Wheel_FL" => Fl,
        "_Wheel_FR" => Fr,
        "_Wheel_RL" => Rl,
        "_Wheel_RR" => Rr,
        _ => return None,
    })
}

fn imported_traffic_wheel_binding_to_insert(
    asset_prefix: &str,
    node_name: &str,
    existing: Option<&ImportedTrafficWheel>,
    owner: Entity,
    baseline: Quat,
) -> Option<(
    ImportedTrafficWheelOwner,
    ImportedTrafficWheelBaseline,
    ImportedTrafficWheelSlot,
)> {
    if existing.is_some() {
        return None;
    }
    Some((
        ImportedTrafficWheelOwner(owner),
        ImportedTrafficWheelBaseline(baseline),
        classify_imported_traffic_wheel(asset_prefix, node_name)?,
    ))
}

fn is_traffic_descendant_of(
    mut entity: Entity,
    root: Entity,
    mut parent_of: impl FnMut(Entity) -> Option<Entity>,
) -> bool {
    while let Some(parent) = parent_of(entity) {
        if parent == root {
            return true;
        }
        if parent == entity {
            return false;
        }
        entity = parent;
    }
    false
}

fn imported_traffic_wheels_ready(
    slots: impl IntoIterator<Item = ImportedTrafficWheelSlot>,
) -> bool {
    use ImportedTrafficWheelSlot::*;
    let mut counts = [0_u8; 4];
    for slot in slots {
        let index = match slot {
            Fl => 0,
            Fr => 1,
            Rl => 2,
            Rr => 3,
        };
        counts[index] = counts[index].saturating_add(1);
    }
    counts == [1; 4]
}

fn imported_traffic_visual_ready(
    slots: impl IntoIterator<Item = ImportedTrafficWheelSlot>,
    paint_assigned: bool,
) -> bool {
    paint_assigned && imported_traffic_wheels_ready(slots)
}

fn compose_imported_traffic_wheel_rotation(baseline: Quat, spin_radians: f32) -> Quat {
    baseline * Quat::from_rotation_x(spin_radians)
}

/// Bind named wheel pivots only within their selected scene wrapper. Scene
/// entities arrive asynchronously, so this runs every playing frame until the
/// wrapper is ready. Existing markers are never replaced, preserving the true
/// authored baseline rather than accidentally capturing an animated pose.
fn bind_imported_traffic_wheels(
    mut commands: Commands,
    wrappers: Query<(Entity, &ImportedTrafficVisual, &ChildOf), Without<ImportedTrafficReady>>,
    nodes: Query<(Entity, &Name, &Transform, Option<&ImportedTrafficWheel>)>,
    parents: Query<&ChildOf>,
) {
    for (wrapper, visual, wrapper_parent) in &wrappers {
        let owner = wrapper_parent.parent();
        for (entity, name, transform, existing) in &nodes {
            if existing.is_some()
                || !is_traffic_descendant_of(entity, wrapper, |candidate| {
                    parents.get(candidate).ok().map(ChildOf::parent)
                })
            {
                continue;
            }
            let Some((wheel_owner, baseline, slot)) = imported_traffic_wheel_binding_to_insert(
                visual.asset_prefix,
                name.as_str(),
                existing,
                owner,
                transform.rotation,
            ) else {
                continue;
            };
            commands
                .entity(entity)
                .insert((ImportedTrafficWheel, wheel_owner, baseline, slot));
        }
    }
}

/// Clone an imported `Toy_Paint` material at most once for each traffic
/// wrapper, changing only its base color. All matching primitives owned by the
/// wrapper share that clone, including primitives instantiated in later
/// frames. Unrelated imported player/world primitives are ancestry-excluded.
fn bind_imported_traffic_paint(
    mut commands: Commands,
    mut wrappers: Query<(
        Entity,
        &ImportedTrafficVisual,
        &ChildOf,
        &mut ImportedTrafficPaintMaterial,
    )>,
    mut primitives: Query<(
        Entity,
        &GltfMaterialName,
        &mut MeshMaterial3d<StandardMaterial>,
        Option<&ImportedTrafficPaintOwner>,
    )>,
    parents: Query<&ChildOf>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    details: Option<Res<PbrDetailAssets>>,
) {
    for (wrapper, visual, wrapper_parent, mut instance_material) in &mut wrappers {
        let owner = wrapper_parent.parent();
        for (entity, name, mut primitive_material, assigned_owner) in &mut primitives {
            if name.0 != "Toy_Paint"
                || !is_traffic_descendant_of(entity, wrapper, |candidate| {
                    parents.get(candidate).ok().map(ChildOf::parent)
                })
            {
                continue;
            }

            let instance_handle = if let Some(handle) = &instance_material.0 {
                handle.clone()
            } else {
                let Some(mut cloned) = materials.get(primitive_material.0.id()).cloned() else {
                    // Scene dependencies normally make this available with the
                    // primitive; retry next frame if loading is still pending.
                    continue;
                };
                cloned.base_color = TRAFFIC_PAINT_COLORS[visual.paint_index];
                // Traffic GLBs have UV0 but no tangents, so add only shared
                // plastic roughness/AO detail and preserve authored maps.
                if let Some(details) = details.as_deref() {
                    if cloned.metallic_roughness_texture.is_none() {
                        cloned.metallic_roughness_texture = Some(details.plastic_orm.clone());
                    }
                    if cloned.occlusion_texture.is_none() {
                        cloned.occlusion_texture = Some(details.plastic_orm.clone());
                    }
                }
                let handle = materials.add(cloned);
                instance_material.0 = Some(handle.clone());
                handle
            };

            if primitive_material.0.id() != instance_handle.id() {
                primitive_material.0 = instance_handle;
            }
            if assigned_owner.map(|assigned| assigned.0) != Some(owner) {
                commands
                    .entity(entity)
                    .insert(ImportedTrafficPaintOwner(owner));
            }
        }
    }
}

fn update_imported_traffic_ready(
    mut commands: Commands,
    wrappers: Query<(Entity, &ChildOf, Option<&ImportedTrafficReady>), With<ImportedTrafficVisual>>,
    wheels: Query<
        (
            Entity,
            &ImportedTrafficWheelOwner,
            &ImportedTrafficWheelSlot,
        ),
        With<ImportedTrafficWheel>,
    >,
    paints: Query<(Entity, &ImportedTrafficPaintOwner)>,
    parents: Query<&ChildOf>,
) {
    for (wrapper, wrapper_parent, ready) in &wrappers {
        let owner = wrapper_parent.parent();
        let slots = wheels.iter().filter_map(|(entity, wheel_owner, slot)| {
            (wheel_owner.0 == owner
                && is_traffic_descendant_of(entity, wrapper, |candidate| {
                    parents.get(candidate).ok().map(ChildOf::parent)
                }))
            .then_some(*slot)
        });
        let paint_assigned = paints.iter().any(|(entity, paint_owner)| {
            paint_owner.0 == owner
                && is_traffic_descendant_of(entity, wrapper, |candidate| {
                    parents.get(candidate).ok().map(ChildOf::parent)
                })
        });
        let complete = imported_traffic_visual_ready(slots, paint_assigned);
        match (complete, ready.is_some()) {
            (true, false) => {
                commands.entity(wrapper).insert(ImportedTrafficReady);
            }
            (false, true) => {
                commands.entity(wrapper).remove::<ImportedTrafficReady>();
            }
            _ => {}
        }
    }
}

/// Roll all four imported wheel pivots after traffic movement. Rotation is
/// always `authored_baseline * rotation_x(spin)`, never the prior frame's
/// transform, so floating-point error cannot accumulate into axle drift.
fn animate_imported_traffic_wheels(
    time: Res<Time>,
    owners: Query<&Traffic>,
    mut wrappers: Query<
        (Entity, &ChildOf, &mut ImportedTrafficWheelAnimation),
        (With<ImportedTrafficVisual>, With<ImportedTrafficReady>),
    >,
    mut wheels: Query<
        (
            Entity,
            &ImportedTrafficWheelOwner,
            &ImportedTrafficWheelBaseline,
            &mut Transform,
        ),
        With<ImportedTrafficWheel>,
    >,
    parents: Query<&ChildOf>,
) {
    let delta_seconds = time.delta_secs();
    for (wrapper, wrapper_parent, mut animation) in &mut wrappers {
        let owner = wrapper_parent.parent();
        let Ok(traffic) = owners.get(owner) else {
            continue;
        };
        animation.spin_radians = (animation.spin_radians
            + traffic_wheel_spin_delta(traffic.speed, delta_seconds))
        .rem_euclid(TAU);
        for (entity, wheel_owner, baseline, mut transform) in &mut wheels {
            if wheel_owner.0 == owner
                && is_traffic_descendant_of(entity, wrapper, |candidate| {
                    parents.get(candidate).ok().map(ChildOf::parent)
                })
            {
                transform.rotation =
                    compose_imported_traffic_wheel_rotation(baseline.0, animation.spin_radians);
            }
        }
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

/// Number of successful spawns still needed after all survivor and deferred
/// despawn accounting. Saturation makes a reduced target a zero-work top-up.
fn traffic_top_up_needed(alive: usize, target: usize) -> usize {
    target.saturating_sub(alive)
}

/// Run the expensive half of traffic top-up at most once per update. Keeping
/// the zero-needed guard around both closures makes catalog construction and
/// spawn-seed work observable and independently testable without an ECS world.
fn with_bounded_traffic_top_up<C, R>(
    needed: usize,
    build_catalog: impl FnOnce() -> C,
    attempt_spawn: impl FnOnce(C) -> R,
) -> Option<R> {
    if needed == 0 {
        return None;
    }
    Some(attempt_spawn(build_catalog()))
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

/// Pure integer avalanche used only for presentation selection. This is not an
/// LCG step and cannot mutate or otherwise perturb gameplay RNG state.
fn traffic_visual_hash(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

fn traffic_kind(seed: u32) -> TrafficKind {
    TRAFFIC_KIND_BUCKETS[(traffic_visual_hash(seed) % 20) as usize]
}

/// Spawn one top-level traffic car. The root front (-Z), stored velocity, and
/// conservative collider all derive from the same connector tangent.
fn spawn_one_traffic(
    commands: &mut Commands,
    visual_assets: &TrafficVisualAssets,
    toy_shading: &ToyShadingAssets,
    candidate: TrafficSpawnCandidate,
    speed: f32,
    speed_roll: f32,
    traffic_id: u64,
    seed: &mut u32,
) {
    let heading = (-candidate.tangent.x).atan2(-candidate.tangent.y);
    let kind = traffic_kind(*seed);
    let scene = visual_assets.scene(kind);
    // Preserve the established spawn RNG contract and palette selection.
    // Presentation hashing consumes no LCG state; paint still consumes the one
    // post-kind roll used by the former procedural traffic.
    let paint_roll = rand(seed);
    let paint_index =
        (paint_roll * TRAFFIC_PAINT_COLORS.len() as f32) as usize % TRAFFIC_PAINT_COLORS.len();
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
            root.spawn((
                Mesh3d(toy_shading.contact_plane.clone()),
                MeshMaterial3d(toy_shading.contact_material.clone()),
                contact_shadow_transform(TRAFFIC_SHADOW_FOOTPRINT, 0.0),
                ToyContactShadow,
            ));
            #[cfg(target_arch = "wasm32")]
            {
                let caster = ToyShadowCaster::new(
                    TRAFFIC_SHADOW_FOOTPRINT,
                    TRAFFIC_SHADOW_CASTER_HEIGHT,
                    0.0,
                );
                root.spawn((
                    Mesh3d(toy_shading.cast_plane.clone()),
                    MeshMaterial3d(toy_shading.cast_material.clone()),
                    counter_rotated_projected_shadow_transform(
                        Quat::from_rotation_y(heading),
                        caster,
                    ),
                    ToyCastShadow,
                    caster,
                ));
            }

            // The authored glTF is already Bevy-oriented (front -Z, ground at
            // Y=0), so this wrapper must remain exactly identity transformed.
            root.spawn((
                WorldAssetRoot(scene),
                Transform::IDENTITY,
                ImportedTrafficVisual {
                    asset_prefix: kind.asset_prefix(),
                    paint_index,
                },
                ImportedTrafficPaintMaterial::default(),
                ImportedTrafficWheelAnimation::default(),
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
    use crate::toy_shading::ToyShadingAssets;
    use bevy::ecs::world::CommandQueue;

    const EXPECTED_BODY_COLORS: [Color; 5] = TRAFFIC_PAINT_COLORS;

    fn traffic_asset_test_world() -> World {
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Assets<StandardMaterial>>();
        world.init_resource::<ToyShadingAssets>();
        let assets = TrafficAssets::from_world(&mut world);
        world.insert_resource(assets);
        world
    }

    fn traffic_runtime_test_world() -> World {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        app.init_asset::<WorldAsset>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<ToyShadingAssets>();
        app.finish();
        app.cleanup();
        app.init_resource::<TrafficVisualAssets>();
        std::mem::replace(app.world_mut(), World::new())
    }

    fn traffic_paint_test_app() -> App {
        let mut app = App::new();
        app.init_resource::<Assets<StandardMaterial>>()
            .add_systems(Update, bind_imported_traffic_paint);
        app
    }

    fn spawn_paint_wrapper(app: &mut App, paint_index: usize) -> (Entity, Entity) {
        let connector = road_plan(0, 0)
            .connectors
            .into_iter()
            .flatten()
            .next()
            .unwrap();
        let owner = app
            .world_mut()
            .spawn((Traffic {
                id: 1,
                speed: 0.0,
                speed_roll: 0.0,
                velocity: Vec2::ZERO,
                connector,
                distance: 0.0,
                route_rng: 1,
            },))
            .id();
        let wrapper = app
            .world_mut()
            .spawn((
                ImportedTrafficVisual {
                    asset_prefix: TrafficKind::Sedan.asset_prefix(),
                    paint_index,
                },
                ImportedTrafficPaintMaterial::default(),
            ))
            .id();
        app.world_mut().entity_mut(owner).add_child(wrapper);
        (owner, wrapper)
    }

    fn spawn_paint_primitive(
        app: &mut App,
        parent: Entity,
        material: Handle<StandardMaterial>,
    ) -> Entity {
        let primitive = app
            .world_mut()
            .spawn((
                GltfMaterialName("Toy_Paint".to_owned()),
                MeshMaterial3d(material),
            ))
            .id();
        app.world_mut().entity_mut(parent).add_child(primitive);
        primitive
    }

    #[test]
    fn traffic_body_palette_has_exact_cached_coated_plastic_materials() {
        let world = traffic_asset_test_world();
        let assets = world.resource::<TrafficAssets>();
        let materials = world.resource::<Assets<StandardMaterial>>();

        assert_eq!(assets.body_mats.len(), EXPECTED_BODY_COLORS.len());
        // Five body paints, six unchanged shared part materials, and the two
        // globally cached toy-shadow materials.
        assert_eq!(materials.len(), 13);
        for (index, (handle, expected_color)) in assets
            .body_mats
            .iter()
            .zip(EXPECTED_BODY_COLORS)
            .enumerate()
        {
            let actual = materials.get(handle).expect("cached body material");
            let expected = StandardMaterial {
                base_color: expected_color,
                metallic: 0.0,
                perceptual_roughness: 0.30,
                reflectance: 0.5,
                clearcoat: 0.85,
                clearcoat_perceptual_roughness: 0.20,
                ..default()
            };
            assert_eq!(
                format!("{actual:?}"),
                format!("{expected:?}"),
                "body palette material {index} differs"
            );
        }

        for left in 0..EXPECTED_BODY_COLORS.len() {
            for right in left + 1..EXPECTED_BODY_COLORS.len() {
                assert_ne!(
                    EXPECTED_BODY_COLORS[left], EXPECTED_BODY_COLORS[right],
                    "body palette entries {left} and {right} must be distinct"
                );
            }
        }
    }

    #[test]
    fn traffic_non_body_materials_keep_their_existing_finishes() {
        let world = traffic_asset_test_world();
        let assets = world.resource::<TrafficAssets>();
        let materials = world.resource::<Assets<StandardMaterial>>();

        let cabin = materials.get(&assets.cabin_mat).unwrap();
        assert_eq!(cabin.base_color, Color::srgb(0.10, 0.10, 0.18));
        assert_eq!(cabin.metallic, 0.2);
        assert_eq!(cabin.perceptual_roughness, 0.4);
        assert_eq!(cabin.clearcoat, 0.0);

        // Preserve the existing dark dielectric-style glazing rather than
        // applying the body clearcoat to it.
        let windshield = materials.get(&assets.windshield_mat).unwrap();
        assert_eq!(windshield.base_color, Color::srgb(0.05, 0.08, 0.12));
        assert_eq!(windshield.metallic, 0.6);
        assert_eq!(windshield.perceptual_roughness, 0.08);
        assert_eq!(windshield.clearcoat, 0.0);

        let tire = materials.get(&assets.wheel_mat).unwrap();
        assert_eq!(tire.base_color, Color::srgb(0.025, 0.025, 0.03));
        assert_eq!(tire.metallic, 0.0);
        assert_eq!(tire.perceptual_roughness, 0.88);
        assert_eq!(tire.reflectance, 0.35);
        assert_eq!(tire.clearcoat, 0.0);

        let hub = materials.get(&assets.hub_mat).unwrap();
        assert_eq!(hub.base_color, Color::srgb(0.5, 0.53, 0.56));
        assert_eq!(hub.metallic, 0.92);
        assert_eq!(hub.perceptual_roughness, 0.24);
        assert_eq!(hub.reflectance, 0.5);
        assert_eq!(hub.clearcoat, 0.0);

        let headlight = materials.get(&assets.headlight_mat).unwrap();
        assert_eq!(headlight.base_color, Color::srgb(1.0, 0.9, 0.6));
        assert_eq!(headlight.emissive, LinearRgba::new(1.0, 0.9, 0.6, 1.0));
        assert_eq!(headlight.perceptual_roughness, 0.18);
        assert_eq!(headlight.clearcoat, 0.0);

        let rear_light = materials.get(&assets.rear_light_mat).unwrap();
        assert_eq!(rear_light.base_color, Color::srgb(0.45, 0.015, 0.01));
        assert_eq!(rear_light.emissive, LinearRgba::new(0.8, 0.025, 0.015, 1.0));
        assert_eq!(rear_light.perceptual_roughness, 0.22);
        assert_eq!(rear_light.clearcoat, 0.0);
    }

    #[test]
    fn traffic_visual_cache_has_exact_scene0_paths() {
        let world = traffic_runtime_test_world();
        let assets = world.resource::<TrafficVisualAssets>();
        let expected = [
            TRAFFIC_SEDAN_SCENE,
            TRAFFIC_CITY_VAN_SCENE,
            TRAFFIC_HATCHBACK_SCENE,
            TRAFFIC_PICKUP_SCENE,
            TRAFFIC_SUV_SCENE,
        ];
        let actual = assets.all().map(|handle| {
            handle
                .path()
                .expect("cached traffic handle has a path")
                .to_string()
        });
        assert_eq!(actual, expected);
    }

    #[test]
    fn spawned_traffic_has_one_imported_visual_collider_and_cached_contact_card() {
        let mut world = traffic_runtime_test_world();
        let connector = road_plan(0, 0)
            .connectors
            .into_iter()
            .flatten()
            .next()
            .unwrap();
        let candidate = TrafficSpawnCandidate {
            position: Vec2::ZERO,
            half_extents: Vec2::new(TRAFFIC_HALF_WIDTH, TRAFFIC_HALF_LENGTH),
            tangent: Vec2::Y,
            connector,
            distance: 0.0,
            route_rng: 1,
        };
        let initial_seed = 7;
        let expected_scene = world
            .resource::<TrafficVisualAssets>()
            .scene(traffic_kind(initial_seed))
            .id();
        let mut expected_seed = initial_seed;
        let _ = rand(&mut expected_seed);
        let mut actual_seed = initial_seed;
        let mut queue = CommandQueue::default();
        {
            let assets = world.resource::<TrafficVisualAssets>();
            let toy = world.resource::<ToyShadingAssets>();
            let mut commands = Commands::new(&mut queue, &world);
            spawn_one_traffic(
                &mut commands,
                assets,
                toy,
                candidate,
                5.0,
                0.5,
                1,
                &mut actual_seed,
            );
        }
        queue.apply(&mut world);
        assert_eq!(
            actual_seed, expected_seed,
            "visual hashing must not add an LCG step"
        );

        let root = world
            .query_filtered::<Entity, With<Traffic>>()
            .single(&world)
            .unwrap();
        let contact = world
            .query_filtered::<Entity, With<ToyContactShadow>>()
            .single(&world)
            .unwrap();
        let visual = world
            .query_filtered::<Entity, With<ImportedTrafficVisual>>()
            .single(&world)
            .unwrap();
        assert_eq!(
            world
                .get::<ImportedTrafficVisual>(visual)
                .unwrap()
                .asset_prefix,
            traffic_kind(initial_seed).asset_prefix()
        );
        assert!(world.get::<ImportedTrafficWheelAnimation>(visual).is_some());
        assert!(world.get::<ImportedTrafficPaintMaterial>(visual).is_some());
        assert!(world.get::<ImportedTrafficReady>(visual).is_none());
        assert_eq!(world.get::<ChildOf>(contact).unwrap().parent(), root);
        assert_eq!(world.get::<ChildOf>(visual).unwrap().parent(), root);
        assert_eq!(
            *world.get::<Transform>(visual).unwrap(),
            Transform::IDENTITY
        );
        assert_eq!(
            world.get::<WorldAssetRoot>(visual).unwrap().0.id(),
            expected_scene
        );
        assert!(world.get::<Collider>(root).is_some());
        assert!(world.get::<Collider>(visual).is_none());
        assert_eq!(world.query::<&Traffic>().iter(&world).count(), 1);
        assert_eq!(world.query::<&Collider>().iter(&world).count(), 1);
        assert_eq!(world.query::<&WorldAssetRoot>().iter(&world).count(), 1);
        assert_eq!(
            world.query::<&Mesh3d>().iter(&world).count(),
            1 + usize::from(cfg!(target_arch = "wasm32"))
        );
        let toy = world.resource::<ToyShadingAssets>();
        assert_eq!(
            world.get::<Mesh3d>(contact).unwrap().0.id(),
            toy.contact_plane.id()
        );
        assert_eq!(
            world
                .get::<MeshMaterial3d<StandardMaterial>>(contact)
                .unwrap()
                .0
                .id(),
            toy.contact_material.id()
        );
        assert_eq!(
            world.query::<&ToyShadowCaster>().iter(&world).count(),
            usize::from(cfg!(target_arch = "wasm32"))
        );

        // Recursive traffic cleanup owns the imported wrapper and both cards.
        // A subsequent round spawn produces exactly one fresh hierarchy rather
        // than leaking or duplicating a visual/shadow child.
        let mut cleanup = CommandQueue::default();
        Commands::new(&mut cleanup, &world).entity(root).despawn();
        cleanup.apply(&mut world);
        assert_eq!(
            world
                .query_filtered::<Entity, With<ToyContactShadow>>()
                .iter(&world)
                .count(),
            0
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<ImportedTrafficVisual>>()
                .iter(&world)
                .count(),
            0,
            "recursive traffic cleanup must own the async scene wrapper"
        );

        let mut restart = CommandQueue::default();
        {
            let assets = world.resource::<TrafficVisualAssets>();
            let toy = world.resource::<ToyShadingAssets>();
            let mut commands = Commands::new(&mut restart, &world);
            let mut seed = 9;
            spawn_one_traffic(
                &mut commands,
                assets,
                toy,
                candidate,
                5.0,
                0.5,
                2,
                &mut seed,
            );
        }
        restart.apply(&mut world);
        assert_eq!(world.query::<&Traffic>().iter(&world).count(), 1);
        assert_eq!(
            world
                .query_filtered::<Entity, With<ToyContactShadow>>()
                .iter(&world)
                .count(),
            1
        );
        assert_eq!(
            world
                .query_filtered::<Entity, With<ImportedTrafficVisual>>()
                .iter(&world)
                .count(),
            1,
            "a new lifecycle gets exactly one fresh imported wrapper"
        );
    }

    #[test]
    fn imported_traffic_paint_clones_once_shares_and_preserves_authored_fields() {
        let mut app = traffic_paint_test_app();
        let base_texture = Handle::<Image>::default();
        let emissive_texture = Handle::<Image>::default();
        let source = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: Color::WHITE,
                base_color_texture: Some(base_texture.clone()),
                metallic: 0.63,
                perceptual_roughness: 0.17,
                reflectance: 0.41,
                clearcoat: 0.72,
                clearcoat_perceptual_roughness: 0.26,
                emissive: LinearRgba::new(0.1, 0.2, 0.3, 1.0),
                emissive_texture: Some(emissive_texture.clone()),
                alpha_mode: AlphaMode::Blend,
                ..default()
            });
        let source_before = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&source)
            .unwrap()
            .clone();
        let (owner, wrapper) = spawn_paint_wrapper(&mut app, 3);
        let first = spawn_paint_primitive(&mut app, wrapper, source.clone());
        let branch = app.world_mut().spawn_empty().id();
        app.world_mut().entity_mut(wrapper).add_child(branch);
        let second = spawn_paint_primitive(&mut app, branch, source.clone());
        // A similarly named player/world primitive outside the wrapper must be
        // untouched and must not receive traffic ownership.
        let outside = spawn_paint_primitive(&mut app, owner, source.clone());

        app.update();

        let first_handle = app
            .world()
            .get::<MeshMaterial3d<StandardMaterial>>(first)
            .unwrap();
        let second_handle = app
            .world()
            .get::<MeshMaterial3d<StandardMaterial>>(second)
            .unwrap();
        assert_eq!(first_handle.0.id(), second_handle.0.id());
        assert_ne!(first_handle.0.id(), source.id());
        assert_eq!(
            app.world()
                .get::<ImportedTrafficPaintMaterial>(wrapper)
                .unwrap()
                .0
                .as_ref()
                .unwrap()
                .id(),
            first_handle.0.id()
        );
        assert_eq!(
            app.world()
                .get::<ImportedTrafficPaintOwner>(first)
                .unwrap()
                .0,
            owner
        );
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(outside)
                .unwrap()
                .0
                .id(),
            source.id()
        );
        assert!(
            app.world()
                .get::<ImportedTrafficPaintOwner>(outside)
                .is_none()
        );
        assert_eq!(
            app.world().resource::<Assets<StandardMaterial>>().len(),
            2,
            "two owner primitives must create exactly one instance clone"
        );

        let materials = app.world().resource::<Assets<StandardMaterial>>();
        assert_eq!(
            format!("{:?}", materials.get(&source).unwrap()),
            format!("{source_before:?}"),
            "the shared authored source must remain untouched"
        );
        let mut expected = source_before;
        expected.base_color = TRAFFIC_PAINT_COLORS[3];
        let cloned = materials.get(&first_handle.0).unwrap();
        assert_eq!(
            format!("{cloned:?}"),
            format!("{expected:?}"),
            "only base_color may differ from the authored source"
        );
        assert_eq!(cloned.base_color_texture.as_ref(), Some(&base_texture));
        assert_eq!(cloned.emissive_texture.as_ref(), Some(&emissive_texture));
        assert_eq!(cloned.alpha_mode, AlphaMode::Blend);
    }

    #[test]
    fn imported_traffic_paint_clone_is_removed_with_its_owner_lifecycle() {
        let mut app = traffic_paint_test_app();
        let source = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let (owner, wrapper) = spawn_paint_wrapper(&mut app, 0);
        spawn_paint_primitive(&mut app, wrapper, source);
        app.update();
        assert_eq!(app.world().resource::<Assets<StandardMaterial>>().len(), 2);

        app.add_systems(Update, cleanup_traffic);
        app.update();
        app.update();
        assert!(app.world().get_entity(owner).is_err());
        assert_eq!(
            app.world().resource::<Assets<StandardMaterial>>().len(),
            1,
            "recycled traffic must remove its unique paint clone"
        );
    }

    #[test]
    fn imported_traffic_paint_clones_are_independent_across_owners() {
        let mut app = traffic_paint_test_app();
        let source = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let (_, first_wrapper) = spawn_paint_wrapper(&mut app, 0);
        let (_, second_wrapper) = spawn_paint_wrapper(&mut app, 1);
        let first = spawn_paint_primitive(&mut app, first_wrapper, source.clone());
        let second = spawn_paint_primitive(&mut app, second_wrapper, source.clone());

        app.update();

        let first_handle = app
            .world()
            .get::<MeshMaterial3d<StandardMaterial>>(first)
            .unwrap();
        let second_handle = app
            .world()
            .get::<MeshMaterial3d<StandardMaterial>>(second)
            .unwrap();
        assert_ne!(first_handle.0.id(), second_handle.0.id());
        assert_eq!(
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(&first_handle.0)
                .unwrap()
                .base_color,
            TRAFFIC_PAINT_COLORS[0]
        );
        assert_eq!(
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(&second_handle.0)
                .unwrap()
                .base_color,
            TRAFFIC_PAINT_COLORS[1]
        );
        assert_eq!(
            app.world().resource::<Assets<StandardMaterial>>().len(),
            3,
            "one shared source plus exactly one clone per owner"
        );
    }

    #[test]
    fn imported_traffic_wheel_classifier_uses_exact_selected_prefix_and_suffixes() {
        use ImportedTrafficWheelSlot::*;
        for kind in [
            TrafficKind::Sedan,
            TrafficKind::CityVan,
            TrafficKind::Hatchback,
            TrafficKind::Pickup,
            TrafficKind::Suv,
        ] {
            let prefix = kind.asset_prefix();
            assert_eq!(
                classify_imported_traffic_wheel(prefix, &format!("{prefix}_Wheel_FL")),
                Some(Fl)
            );
            assert_eq!(
                classify_imported_traffic_wheel(prefix, &format!("{prefix}_Wheel_FR")),
                Some(Fr)
            );
            assert_eq!(
                classify_imported_traffic_wheel(prefix, &format!("{prefix}_Wheel_RL")),
                Some(Rl)
            );
            assert_eq!(
                classify_imported_traffic_wheel(prefix, &format!("{prefix}_Wheel_RR")),
                Some(Rr)
            );
        }
        assert_eq!(
            classify_imported_traffic_wheel(
                TrafficKind::Sedan.asset_prefix(),
                "npc_toy_suv_Wheel_FL"
            ),
            None,
            "a node from a non-selected asset must not bind"
        );
        assert_eq!(
            classify_imported_traffic_wheel(
                TrafficKind::Sedan.asset_prefix(),
                "npc_toy_sedan_Wheel_FL_Tire"
            ),
            None
        );
        assert_eq!(
            classify_imported_traffic_wheel(
                TrafficKind::Sedan.asset_prefix(),
                "NPC_TOY_SEDAN_Wheel_FL"
            ),
            None
        );
    }

    #[test]
    fn imported_traffic_binding_is_ancestry_scoped_and_idempotent() {
        let wrapper = Entity::from_raw_u32(1).unwrap();
        let branch = Entity::from_raw_u32(2).unwrap();
        let wheel = Entity::from_raw_u32(3).unwrap();
        let outside = Entity::from_raw_u32(4).unwrap();
        let parent = |entity| match entity {
            e if e == wheel => Some(branch),
            e if e == branch => Some(wrapper),
            _ => None,
        };
        assert!(is_traffic_descendant_of(wheel, wrapper, parent));
        assert!(!is_traffic_descendant_of(outside, wrapper, parent));
        assert!(!is_traffic_descendant_of(wrapper, wrapper, parent));

        // A second async-discovery pass must not replace the true authored
        // baseline with an already animated transform.
        let baseline = Quat::from_euler(EulerRot::XYZ, 0.2, -0.3, 0.4);
        let name = "npc_toy_sedan_Wheel_FL";
        let (_, captured, slot) = imported_traffic_wheel_binding_to_insert(
            "npc_toy_sedan",
            name,
            None,
            wrapper,
            baseline,
        )
        .unwrap();
        assert_eq!(slot, ImportedTrafficWheelSlot::Fl);
        assert!(captured.0.abs_diff_eq(baseline, 1e-6));
        assert!(
            imported_traffic_wheel_binding_to_insert(
                "npc_toy_sedan",
                name,
                Some(&ImportedTrafficWheel),
                wrapper,
                Quat::IDENTITY,
            )
            .is_none()
        );
    }

    #[test]
    fn imported_traffic_readiness_requires_paint_and_four_unique_slots() {
        use ImportedTrafficWheelSlot::*;
        assert!(imported_traffic_visual_ready([Fl, Fr, Rl, Rr], true));
        assert!(!imported_traffic_visual_ready([Fl, Fr, Rl, Rr], false));
        assert!(!imported_traffic_visual_ready([Fl, Fr, Rl], true));
        assert!(!imported_traffic_visual_ready([Fl, Fr, Rl, Rr, Fl], true));
        assert!(!imported_traffic_visual_ready([Fl, Fr, Rl, Rl], true));
    }

    #[test]
    fn imported_traffic_rotation_rebuilds_from_baseline_without_accumulation() {
        let baseline = Quat::from_euler(EulerRot::XYZ, 0.2, -0.3, 0.4);
        let spin = 1.27;
        let expected = baseline * Quat::from_rotation_x(spin);
        let first = compose_imported_traffic_wheel_rotation(baseline, spin);
        let second = compose_imported_traffic_wheel_rotation(baseline, spin);
        assert!(first.abs_diff_eq(expected, 1e-6));
        assert!(second.abs_diff_eq(first, 1e-6));
        assert_eq!(TRAFFIC_WHEEL_RADIUS, 0.19);
    }

    #[test]
    fn imported_visual_roots_respect_the_traffic_hard_cap() {
        let mut world = traffic_runtime_test_world();
        let connector = road_plan(0, 0)
            .connectors
            .into_iter()
            .flatten()
            .next()
            .unwrap();
        let candidate = TrafficSpawnCandidate {
            position: Vec2::ZERO,
            half_extents: Vec2::new(TRAFFIC_HALF_WIDTH, TRAFFIC_HALF_LENGTH),
            tangent: Vec2::Y,
            connector,
            distance: 0.0,
            route_rng: 1,
        };
        let mut queue = CommandQueue::default();
        {
            let assets = world.resource::<TrafficVisualAssets>();
            let toy = world.resource::<ToyShadingAssets>();
            let mut commands = Commands::new(&mut queue, &world);
            let mut seed = 1;
            for id in 1..=MAX_TRAFFIC as u64 {
                spawn_one_traffic(
                    &mut commands,
                    assets,
                    toy,
                    candidate,
                    5.0,
                    0.5,
                    id,
                    &mut seed,
                );
            }
        }
        queue.apply(&mut world);
        assert_eq!(world.query::<&Traffic>().iter(&world).count(), MAX_TRAFFIC);
        assert_eq!(
            world
                .query_filtered::<Entity, With<ImportedTrafficVisual>>()
                .iter(&world)
                .count(),
            MAX_TRAFFIC,
            "each bounded gameplay root owns exactly one imported wrapper"
        );
        assert_eq!(
            world.query::<&WorldAssetRoot>().iter(&world).count(),
            MAX_TRAFFIC
        );
        assert_eq!(
            world
                .query::<&ImportedTrafficPaintMaterial>()
                .iter(&world)
                .count(),
            MAX_TRAFFIC,
            "the traffic cap bounds per-owner paint state to eight wrappers"
        );
    }

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
    fn bounded_top_up_does_no_catalog_or_seed_work_at_or_above_target() {
        for alive in [MAX_TRAFFIC, MAX_TRAFFIC + 1] {
            let mut catalog_builds = 0;
            let mut seed_work = 0;
            let result = with_bounded_traffic_top_up(
                traffic_top_up_needed(alive, MAX_TRAFFIC),
                || catalog_builds += 1,
                |_| seed_work += 1,
            );
            assert_eq!(result, None);
            assert_eq!(catalog_builds, 0);
            assert_eq!(seed_work, 0);
        }
    }

    #[test]
    fn bounded_top_up_reaches_zero_to_eight_one_per_update_without_overshoot() {
        let mut alive = 0;
        let mut catalog_builds = 0;
        let mut seed_attempts = 0;

        for expected_alive in 1..=MAX_TRAFFIC {
            let spawned = with_bounded_traffic_top_up(
                traffic_top_up_needed(alive, MAX_TRAFFIC),
                || {
                    catalog_builds += 1;
                },
                |_| {
                    seed_attempts += 1;
                    true
                },
            )
            .unwrap_or(false);
            alive += usize::from(spawned);
            assert_eq!(alive, expected_alive);
            assert_eq!(catalog_builds, expected_alive);
            assert_eq!(seed_attempts, expected_alive);
        }

        // Further updates at target perform no work and cannot overshoot.
        for _ in 0..3 {
            let spawned = with_bounded_traffic_top_up(
                traffic_top_up_needed(alive, MAX_TRAFFIC),
                || {
                    catalog_builds += 1;
                },
                |_| {
                    seed_attempts += 1;
                    true
                },
            )
            .unwrap_or(false);
            alive += usize::from(spawned);
        }
        assert_eq!(alive, MAX_TRAFFIC);
        assert_eq!(catalog_builds, MAX_TRAFFIC);
        assert_eq!(seed_attempts, MAX_TRAFFIC);
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
    fn kind_bucket_distribution_is_exact_and_every_variant_is_reachable() {
        let mut counts = [0; 5];
        for kind in TRAFFIC_KIND_BUCKETS {
            counts[match kind {
                TrafficKind::Sedan => 0,
                TrafficKind::CityVan => 1,
                TrafficKind::Hatchback => 2,
                TrafficKind::Pickup => 3,
                TrafficKind::Suv => 4,
            }] += 1;
        }
        assert_eq!(counts, [6, 5, 4, 3, 2]);

        let reached: Vec<_> = (0..10_000_u32).map(traffic_kind).collect();
        for kind in [
            TrafficKind::Sedan,
            TrafficKind::CityVan,
            TrafficKind::Hatchback,
            TrafficKind::Pickup,
            TrafficKind::Suv,
        ] {
            assert!(reached.contains(&kind), "{kind:?} must be reachable");
        }
    }

    #[test]
    fn kind_selection_is_deterministic_and_does_not_advance_gameplay_rng() {
        let seeds: Vec<_> = (0_u32..64)
            .map(|i| 0xCAFE_BABE_u32.wrapping_add(i.wrapping_mul(0x9E37_79B9)))
            .collect();
        assert_eq!(
            seeds.iter().copied().map(traffic_kind).collect::<Vec<_>>(),
            seeds.iter().copied().map(traffic_kind).collect::<Vec<_>>()
        );
        for initial in seeds {
            let mut selected_seed = initial;
            let _ = traffic_kind(selected_seed);
            let selected_next = next_random_u32(&mut selected_seed);
            let mut control_seed = initial;
            let control_next = next_random_u32(&mut control_seed);
            assert_eq!((selected_seed, selected_next), (control_seed, control_next));
        }
    }
}
