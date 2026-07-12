//! Difficulty ramp + oncoming traffic (T18).
//!
//! This module is the sole owner of the difficulty / traffic logic. It
//! provides:
//!
//! - `Difficulty { elapsed, level }` — a resource tracking how long the
//!   current round has been running (only ticks while input is NOT frozen,
//!   mirroring `tick_timeleft`) and the derived difficulty level
//!   (`level = (elapsed / 10) as u32`, capped at 6).
//! - `Traffic { speed, axis, dir }` — a moving car the player must avoid.
//!   Traffic entities are top-level (world `Transform`) and carry a
//!   `world::Collider { half_x: 0.5, half_z: 1.0 }` so `car.rs::
//!   physics_collisions` treats them as solid obstacles — crashing into one
//!   emits `ObstacleHit` → damage. The count scales with `level`
//!   (`1 + level/2`, capped at 8); they drive straight along a world axis
//!   and are recycled (despawned + respawned near/ahead) once they drift
//!   ~60u from the car.
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

use crate::car::{Car, InputFrozen};
use crate::game::resources::RoundActive;
use crate::game::state::GameState;
use crate::game::SpawnSet;
use crate::world::Collider;

// ===========================================================================
// Tuning constants
// ===========================================================================

/// Seconds of round elapsed per difficulty level. `level = (elapsed / 10)`.
const LEVEL_SECONDS: f32 = 10.0;
/// Maximum difficulty level (caps the ramp over the 60s round: 0..=6).
const MAX_LEVEL: u32 = 6;

/// Target traffic population = `(1 + level / 2).min(MAX_TRAFFIC)`.
const MAX_TRAFFIC: usize = 8;

/// Distance from the car (XZ) beyond which a traffic car is recycled
/// (despawned + replaced). Keeps the traffic near the endless driver.
const TRAFFIC_KEEP_RADIUS: f32 = 60.0;

/// Traffic spawn forward bias range (ahead of the car, in its forward
/// direction): `SPAWN_AHEAD_MIN .. + SPAWN_AHEAD_RANGE`.
const SPAWN_AHEAD_MIN: f32 = 18.0;
const SPAWN_AHEAD_RANGE: f32 = 32.0;
/// Lateral offset (perpendicular to the car's forward, in the XZ plane) so
/// traffic doesn't spawn exactly on the car's path line. ±`SPAWN_LATERAL`.
const SPAWN_LATERAL: f32 = 3.0;

/// Base traffic speed at level 0 (u/s). The player's `max_speed` is 12.0, so
/// traffic is always slower and must be dodged, not outrun-forward forever.
const TRAFFIC_BASE_SPEED: f32 = 5.0;
/// Per-level speed gain (so later traffic is a bit quicker). At level 6 →
/// `5 + 6*0.7 = 9.2`, still under the player's cap.
const TRAFFIC_SPEED_PER_LEVEL: f32 = 0.7;
/// Per-car speed jitter band: `speed *= 0.85 + rand * 0.3` (0.85..1.15).
const TRAFFIC_SPEED_JITTER: f32 = 0.3;
const TRAFFIC_SPEED_JITTER_BASE: f32 = 0.85;

// --- Traffic car mesh proportions (mirrors `car.rs` styling, no wheels) ---
const BODY_W: f32 = 1.0;
const BODY_H: f32 = 0.5;
const BODY_D: f32 = 2.0;
const CABIN_W: f32 = 0.8;
const CABIN_H: f32 = 0.4;
const CABIN_D: f32 = 1.0;
const WINDSHIELD_W: f32 = 0.7;
const WINDSHIELD_H: f32 = 0.2;
const WINDSHIELD_D: f32 = 0.03;

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
    body_mesh: Handle<Mesh>,
    cabin_mesh: Handle<Mesh>,
    windshield_mesh: Handle<Mesh>,
    headlight_mesh: Handle<Mesh>,
    /// A small palette of car-paint body materials (one per color) for
    /// variety. Indexed by `weighted`-ish uniform pick at spawn.
    body_mats: [Handle<StandardMaterial>; 5],
    cabin_mat: Handle<StandardMaterial>,
    windshield_mat: Handle<StandardMaterial>,
    headlight_mat: Handle<StandardMaterial>,
}

impl FromWorld for TrafficAssets {
    fn from_world(world: &mut World) -> Self {
        // Scope meshes first, then materials inside the closure so we never
        // hold `&mut Assets<Mesh>` + `&mut Assets<StandardMaterial>` without
        // scoping (risk E3 — mirrors `chickens.rs` / `pickups.rs`).
        world.resource_scope::<Assets<Mesh>, _>(|world, mut meshes| {
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();

            let body_mesh = meshes.add(Cuboid::new(BODY_W, BODY_H, BODY_D));
            let cabin_mesh = meshes.add(Cuboid::new(CABIN_W, CABIN_H, CABIN_D));
            let windshield_mesh = meshes.add(Cuboid::new(WINDSHIELD_W, WINDSHIELD_H, WINDSHIELD_D));
            let headlight_mesh = meshes.add(Cuboid::new(0.18, 0.12, 0.04));

            // Car-paint palette: slightly metallic, low-ish roughness so the
            // IBL + bloom read them as glossy cars (T15-style PBR).
            let body_colors = [
                Color::srgb(0.85, 0.12, 0.10), // red
                Color::srgb(0.15, 0.35, 0.85), // blue
                Color::srgb(0.18, 0.55, 0.22), // green
                Color::srgb(0.78, 0.78, 0.82), // silver
                Color::srgb(0.95, 0.65, 0.08), // orange
            ];
            let body_mats = body_colors.map(|c| {
                materials.add(StandardMaterial {
                    base_color: c,
                    metallic: 0.6,
                    perceptual_roughness: 0.35,
                    ..default()
                })
            });

            // Cabin: dark glass-like box (matches `car.rs::CAR_CABIN`).
            let cabin_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.10, 0.10, 0.18),
                perceptual_roughness: 0.4,
                metallic: 0.2,
                ..default()
            });

            // Windshield: dark, glossy, metallic so it catches reflections.
            let windshield_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.05, 0.08, 0.12),
                perceptual_roughness: 0.08,
                metallic: 0.6,
                ..default()
            });

            // Headlights: warm emissive cubes (front = -Z). `LinearRgba`
            // emissive so they glow under bloom (T9 rendering).
            let headlight_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(1.0, 0.9, 0.6),
                emissive: LinearRgba::new(1.0, 0.9, 0.6, 1.0),
                ..default()
            });

            TrafficAssets {
                body_mesh,
                cabin_mesh,
                windshield_mesh,
                headlight_mesh,
                body_mats,
                cabin_mat,
                windshield_mat,
                headlight_mat,
            }
        })
    }
}

// ===========================================================================
// Components
// ===========================================================================

/// A moving traffic car the player must avoid.
///
/// - `speed` — units per second along the movement axis.
/// - `axis`  — `true` => drives along world X; `false` => along world Z.
/// - `dir`   — `+1.0` or `-1.0` (direction along the axis).
///
/// The entity is **top-level** (world `Transform`) and also carries a
/// `Collider { half_x: 0.5, half_z: 1.0 }` so `physics_collisions` crashes
/// the car into it. The root `Transform`'s rotation is set at spawn so the
/// body's front (-Z, where the headlights are) faces the movement direction;
/// `manage_traffic` only advances `translation` each frame.
#[derive(Component)]
pub struct Traffic {
    pub speed: f32,
    pub axis: bool,
    pub dir: f32,
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
            // Per-frame: ramp the level + manage the traffic population.
            .add_systems(
                Update,
                (tick_difficulty, manage_traffic)
                    .run_if(in_state(GameState::Playing)),
            )
            // UI refresh runs in every state so the label recovers even while
            // paused; the query is trivial when the UI root is absent.
            .add_systems(Update, update_difficulty_ui)
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
/// 1. advance each traffic car along its axis/direction;
/// 2. despawn any that drifted beyond `TRAFFIC_KEEP_RADIUS` from the car;
/// 3. top up the population up to the level-derived target count.
///
/// The car query excludes `Traffic` (and the traffic query fetches `&Traffic`,
/// implying `With<Traffic>`), so the two `Transform` accesses are disjoint →
/// no B0001.
fn manage_traffic(
    mut commands: Commands,
    assets: Res<TrafficAssets>,
    difficulty: Res<Difficulty>,
    car: Query<&Transform, (With<Car>, Without<Traffic>)>,
    mut traffic: Query<(Entity, &Traffic, &mut Transform)>,
    time: Res<Time>,
    mut seed: Local<u32>,
) {
    ensure_seeded(&mut seed, 0x0BADC0DE);
    let Ok(car_t) = car.single() else {
        return;
    };
    let car_pos = car_t.translation;
    let dt = time.delta_secs();

    // --- Move + recycle far-away traffic ---
    // Collect entities to recycle first (we can't despawn+spawn inside the
    // mutable iteration without borrow issues; collecting the ids then
    // despawning after is clean and avoids double-mutation).
    let mut to_despawn: Vec<Entity> = Vec::new();
    let mut alive = 0usize;
    for (e, traffic, mut tf) in &mut traffic {
        let axis_vec = if traffic.axis {
            Vec3::new(traffic.dir, 0.0, 0.0)
        } else {
            Vec3::new(0.0, 0.0, traffic.dir)
        };
        tf.translation += axis_vec * traffic.speed * dt;

        let dx = tf.translation.x - car_pos.x;
        let dz = tf.translation.z - car_pos.z;
        if dx * dx + dz * dz > TRAFFIC_KEEP_RADIUS * TRAFFIC_KEEP_RADIUS {
            to_despawn.push(e);
        } else {
            alive += 1;
        }
    }
    for e in &to_despawn {
        commands.entity(*e).despawn();
    }

    // --- Top up the population to the level-derived target ---
    let target = target_traffic_count(difficulty.level);
    let mut needed = target.saturating_sub(alive);

    // Car forward (heading 0 => -Z); bias spawns ahead of the driver so
    // traffic appears in front (fair — visible + avoidable).
    let forward = car_t.rotation * Vec3::NEG_Z;
    let forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
    // Perpendicular (right) in the XZ plane.
    let right = Vec3::new(forward.z, 0.0, -forward.x);

    while needed > 0 {
        needed -= 1;
        let axis = rand(&mut seed) < 0.5; // true = X, false = Z
        let dir = if rand(&mut seed) < 0.5 { 1.0 } else { -1.0 };
        let speed = traffic_speed(difficulty.level, &mut seed);
        // Choose axis BEFORE position so the spawn can snap the cross-axis
        // coordinate onto a road grid line (traffic drives on roads, not grass).
        let pos = traffic_spawn_pos_on_road(car_pos, forward, right, axis, &mut seed);
        spawn_one_traffic(&mut commands, &assets, pos, axis, dir, speed, &mut seed);
    }
}

/// Despawn every traffic car (e.g. on GameOver / Menu). Recursive despawn in
/// 0.19 nukes the body/cabin/headlight children (safe, risk E2).
fn cleanup_traffic(mut commands: Commands, traffic: Query<Entity, With<Traffic>>) {
    for e in &traffic {
        commands.entity(e).despawn();
    }
}

// ===========================================================================
// Traffic spawn helpers
// ===========================================================================

/// Target traffic population for a given difficulty level:
/// `(1 + level / 2).min(MAX_TRAFFIC)`.
fn target_traffic_count(level: u32) -> usize {
    (1 + level / 2).min(MAX_TRAFFIC as u32) as usize
}

/// Traffic speed for a given level with per-car jitter:
/// `(BASE + level * PER_LEVEL) * (JITTER_BASE + rand * JITTER)`.
fn traffic_speed(level: u32, seed: &mut u32) -> f32 {
    let base = TRAFFIC_BASE_SPEED + level as f32 * TRAFFIC_SPEED_PER_LEVEL;
    base * (TRAFFIC_SPEED_JITTER_BASE + rand(seed) * TRAFFIC_SPEED_JITTER)
}

/// A spawn position ahead of the car (along its forward direction) with a
/// small lateral offset so traffic doesn't spawn on the car's exact path.
fn traffic_spawn_pos_on_road(
    car_pos: Vec3,
    forward: Vec3,
    right: Vec3,
    axis: bool,
    seed: &mut u32,
) -> Vec3 {
    let ahead = SPAWN_AHEAD_MIN + rand(seed) * SPAWN_AHEAD_RANGE;
    let lat = (rand(seed) * 2.0 - 1.0) * SPAWN_LATERAL;
    let mut pos = car_pos + forward * ahead + right * lat;
    // Snap the CROSS-axis coordinate to the nearest road grid line (a multiple
    // of the block size 40) so traffic drives ON a road instead of on the
    // grass. axis=true => moves along X => snap Z; axis=false => snap X.
    const BLOCK: f32 = 40.0;
    if axis {
        pos.z = (pos.z / BLOCK).round() * BLOCK;
    } else {
        pos.x = (pos.x / BLOCK).round() * BLOCK;
    }
    pos
}

/// Spawn one traffic car (top-level) with a body + cabin + windshield +
/// emissive headlights, a `Collider { half_x: 0.5, half_z: 1.0 }`, and the
/// `Traffic` tag. The root `Transform`'s rotation orients the body's front
/// (-Z) toward the movement direction so the headlights lead.
fn spawn_one_traffic(
    commands: &mut Commands,
    assets: &TrafficAssets,
    pos: Vec3,
    axis: bool,
    dir: f32,
    speed: f32,
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

    // Pick a body color uniformly from the palette.
    let color_idx = (rand(seed) * assets.body_mats.len() as f32) as usize % assets.body_mats.len();

    commands
        .spawn((
            Transform::from_translation(pos).with_rotation(rotation),
            Visibility::default(),
            Traffic { speed, axis, dir },
            Collider {
                half_x: 0.5,
                half_z: 1.0,
            },
        ))
        .with_children(|root| {
            // Painted body shell (car paint), sitting at y = BODY_H/2 + a
            // tiny lift so it doesn't z-fight the road.
            root.spawn((
                Mesh3d(assets.body_mesh.clone()),
                MeshMaterial3d(assets.body_mats[color_idx].clone()),
                Transform::from_xyz(0.0, 0.35, 0.0),
            ))
            .with_children(|body| {
                // Cabin on top of the body (slightly toward the rear so the
                // windshield faces forward).
                body.spawn((
                    Mesh3d(assets.cabin_mesh.clone()),
                    MeshMaterial3d(assets.cabin_mat.clone()),
                    Transform::from_xyz(0.0, 0.35, 0.2),
                ));
                // Windshield: dark glass slab on the front of the cabin
                // (front = -Z, where the headlights are).
                body.spawn((
                    Mesh3d(assets.windshield_mesh.clone()),
                    MeshMaterial3d(assets.windshield_mat.clone()),
                    Transform::from_xyz(0.0, 0.45, -0.3),
                ));
                // Headlights at the front bumper (-Z).
                for &x in &[-0.3_f32, 0.3] {
                    body.spawn((
                        Mesh3d(assets.headlight_mesh.clone()),
                        MeshMaterial3d(assets.headlight_mat.clone()),
                        Transform::from_xyz(x, -0.1, -1.0),
                    ));
                }
            });
        });
}

// ===========================================================================
// UI — "Lv {level}" top-right, below the minimap
// ===========================================================================

/// Spawn the "Lv {level}" label. Lives only while `Playing` (despawned by
/// [`despawn_marker::<DifficultyUiRoot>`] on exit). Positioned just below the
/// minimap (top-right), aligned with its right edge.
fn spawn_difficulty_ui(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(UI_TOP),
                right: px(UI_RIGHT),
                padding: UiRect::all(px(6.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.35)),
            DifficultyUiRoot,
            Text::new("Lv "),
            TextFont {
                font_size: FontSize::Px(18.0),
                ..default()
            },
            TextColor(crate::palette::HUD_TEXT.into()),
        ))
        .with_child((
            TextSpan::default(),
            TextFont {
                font_size: FontSize::Px(18.0),
                ..default()
            },
            TextColor(crate::palette::HUD_ACCENT.into()),
            DifficultyLevelText,
        ));
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
