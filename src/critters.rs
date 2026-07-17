//! AI critters (pedestrian / cow / moose) that wander near the roads and
//! that you must **NOT** hit.
//!
//! Distinct from chickens (which **award** score): hitting a critter is a
//! **PENALTY** — at most once per short contact window the car loses 25 base
//! health (scaled by the active modifier) and 2 chicken-score; every hit still
//! fires SFX and `CritterHit`, while reduced motion may suppress particles.
//!
//! The module mirrors `chickens.rs` (wander + recycle-ahead + bob + particle
//! burst + cleanup) but:
//! - has three distinct primitive models (pedestrian / cow / moose) instead of
//!   one chicken model;
//! - penalises on hit instead of rewarding;
//! - does **not** add a `Collider` to critters, so `physics_collisions` won't
//!   push the car off them — the `hit_critters` XZ-distance check handles
//!   contact and lets the car "run them over" for a penalty.
//!
//! Contracts honoured:
//! - `CritterHit` is defined here and registered via
//!   `app.add_message::<CritterHit>()` in `CrittersPlugin` (the orchestrator
//!   does **not** register it in `game/mod.rs`).
//! - `spawn_critters` runs `.in_set(SpawnSet)` and consumes a cleanup-driven
//!   fresh-round latch, so pause/resume skips spawning regardless of reset order.
//! - Critter entities have **no** `Collider` component.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::f32::consts::TAU;

use crate::car::{Car, DrivingSet};
use crate::game::SpawnSet;
use crate::game::resources::{Drowning, GameConfig, GameOverReason, Score};
use crate::game::state::GameState;
use crate::game_modes::{ActiveRunRules, Conduct};
use crate::health::Health;
use crate::modifiers::ActiveModifier;
use crate::run_events::{EventKind, RoundEventStarted};
use crate::settings::Settings;
#[cfg(any(target_arch = "wasm32", test))]
use crate::toy_shading::{ToyCastShadow, projected_shadow_transform};
use crate::toy_shading::{ToyContactShadow, ToyShadingAssets, contact_shadow_transform};
use crate::toy_shading::{ToyMaterialFamily, toy_material};

// ---------------------------------------------------------------------------
// Tuning constants
// ---------------------------------------------------------------------------

/// Initial critters scattered around the car at the start of a fresh round.
/// Kept small (web-friendly + the penalty makes them stressful, not numerous).
const CRITTER_COUNT: usize = 5;

/// Waddle phase advance rate (radians / second). `bob.sin()` produces the
/// oscillating vertical + sway offset on the body group.
const WADDLE_SPEED: f32 = 6.0;

/// Car-to-critter hit distance (XZ plane). Slightly larger than the chicken
/// hit radius so the bigger critter bodies register contact reliably.
const HIT_RADIUS: f32 = 1.2;

/// Critters farther than this from the car are recycled (despawned + respawned
/// ahead) so the menagerie stays near the endless driver.
const KEEP_RADIUS: f32 = 65.0;

/// Critters this far behind the car along its current heading are recycled
/// even if they remain within `KEEP_RADIUS`.
const BEHIND_THRESHOLD: f32 = 15.0;

/// Recycled critters respawn this many units along the car's current forward
/// axis, at a random offset within `[RESPAWN_AHEAD_MIN, ... + RANGE]`.
const RESPAWN_AHEAD_MIN: f32 = 34.0;
const RESPAWN_AHEAD_RANGE: f32 = 22.0;

/// Approximate far edge of the camera footprint. Recycled spawns must clear
/// this in both radial distance and forward projection before appearing.
const VISIBLE_VIEW_RADIUS: f32 = 12.0;

/// Initial scatter radius around the car (fresh round). Inner radius keeps the
/// first critter from spawning on top of the car (which would be an instant
/// penalty on round start).
const SCATTER_RADIUS: f32 = 40.0;
const SCATTER_INNER: f32 = 8.0;

/// Maximum lateral spread from the car's current position and heading for
/// scattered / respawned critters.
const LATERAL_SPREAD: f32 = 22.0;

/// Base health lost per critter hit, before the active damage multiplier.
const HIT_HEALTH_PENALTY: f32 = 25.0;
/// Minimum interval between health/score penalties. Contacts during this
/// bounded window still receive all visual, audio, message and respawn work.
const HIT_PENALTY_COOLDOWN: f32 = 0.4;
/// Chicken-score lost per critter hit (the player is punished for bad driving).
const HIT_SCORE_PENALTY: u32 = 2;

/// Particle burst tuning (web-friendly: small, capped by natural despawn).
const GIB_COUNT: usize = 8;
const PUFF_COUNT: usize = 4;
const GIB_LIFE: f32 = 0.5;
const PUFF_LIFE: f32 = 0.4;
const GIB_GRAVITY: f32 = 6.0;

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// A wandering critter (pedestrian / cow / moose).
///
/// - `dir`   — current heading as a unit vector in the XZ plane.
/// - `speed` — wander speed in units/second (varies by `CritterKind`).
/// - `timer` — seconds until the next random direction pick.
/// - `bob`   — advancing waddle phase; `bob.sin()` drives the body group's
///             vertical bob + z-rotation sway.
///
/// The entity also carries a `Transform` (world position + Y-rotation to face
/// `dir`), a `CritterKind`, and a `Children`-based mesh hierarchy (see
/// `spawn_one_critter`). Critters deliberately have **no** `Collider`.
#[derive(Component)]
pub struct Critter {
    pub dir: Vec3,
    pub speed: f32,
    pub timer: f32,
    pub bob: f32,
}

/// Which kind of critter this is — drives the model built in
/// `spawn_one_critter` and the wander speed.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CritterKind {
    Pedestrian,
    Cow,
    Moose,
}

/// Marks the additional critters created by a mid-run `CritterBurst`. Hits
/// preserve the marker on their replacement, while ordinary distance/behind
/// recycling despawns them without replacement so the burst drains naturally.
#[derive(Component)]
struct CritterBurstExtra;

/// The bob-animated body group of a critter (parent of the body mesh, head,
/// spots, antlers, eyes). `base_y` is the resting local Y offset;
/// `wander_critters` offsets it by `bob.sin() * amplitude` each frame for the
/// waddle. The legs are siblings of this group (children of the critter root)
/// so they stay grounded while the body bobs.
#[derive(Component)]
struct CritterBody {
    base_y: f32,
}

/// Cast-card marker used to counter-rotate WebGL projections against their
/// moving owner's heading.
#[derive(Component)]
struct CritterCastShadow;

/// A red gib particle (small sphere) ejected on critter hit. Affected by
/// gravity + spin; despawns when `life` reaches 0.
#[derive(Component)]
struct Gib {
    vel: Vec3,
    life: f32,
    spin: f32,
}

/// A red smoke puff (flat expanding quad) for the burst on critter hit.
/// Expands + decelerates; despawns when `life` reaches 0.
#[derive(Component)]
struct BloodPuff {
    vel: Vec3,
    life: f32,
    max_life: f32,
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

/// Emitted when the car runs over a wandering critter (penalty hit).
#[derive(Message)]
pub struct CritterHit;

/// Remaining contact-penalty lockout. A resource (rather than a system local)
/// lets fresh-round spawning explicitly clear it independent of reset order.
#[derive(Resource, Default)]
struct CritterPenaltyCooldown(f32);

#[derive(SystemParam)]
struct CritterScoring<'w> {
    score: ResMut<'w, Score>,
    rules: Option<Res<'w, ActiveRunRules>>,
}

/// Fresh-round spawn latch set by cleanup and unaffected by reset ordering.
#[derive(Resource)]
struct CritterSpawnPending(bool);

impl Default for CritterSpawnPending {
    fn default() -> Self {
        Self(true)
    }
}

// ---------------------------------------------------------------------------
// Asset resource (FromWorld — meshes + materials built via scope)
// ---------------------------------------------------------------------------

/// Pre-built mesh + material handles for the three critter models and the
/// hit particle burst. Built once via `FromWorld` so the handles exist
/// before any `OnEnter(Playing)` / `Update` system spawns a critter. The
/// penalty-hit thud SFX is owned and played by the audio core, not here.
#[derive(Resource)]
pub struct CritterAssets {
    // --- Pedestrian parts ---
    ped_body_mesh: Handle<Mesh>,
    ped_head_mesh: Handle<Mesh>,
    // --- Cow parts ---
    cow_body_mesh: Handle<Mesh>,
    cow_head_mesh: Handle<Mesh>,
    cow_spot_mesh: Handle<Mesh>,
    // --- Moose parts ---
    moose_body_mesh: Handle<Mesh>,
    moose_head_mesh: Handle<Mesh>,
    antler_mesh: Handle<Mesh>,
    // --- Shared parts ---
    leg_mesh: Handle<Mesh>,
    eye_mesh: Handle<Mesh>,
    // Cached globally by ToyShadingAssets, then cloned here for spawn access.
    contact_shadow_mesh: Handle<Mesh>,
    #[cfg(any(target_arch = "wasm32", test))]
    cast_shadow_mesh: Handle<Mesh>,
    // --- Particle burst ---
    gib_mesh: Handle<Mesh>,
    puff_mesh: Handle<Mesh>,
    // --- Materials ---
    ped_body_mat: Handle<StandardMaterial>,
    ped_head_mat: Handle<StandardMaterial>,
    ped_leg_mat: Handle<StandardMaterial>,
    cow_body_mat: Handle<StandardMaterial>,
    cow_spot_mat: Handle<StandardMaterial>,
    cow_head_mat: Handle<StandardMaterial>,
    animal_leg_mat: Handle<StandardMaterial>,
    moose_body_mat: Handle<StandardMaterial>,
    moose_head_mat: Handle<StandardMaterial>,
    antler_mat: Handle<StandardMaterial>,
    eye_mat: Handle<StandardMaterial>,
    contact_shadow_mat: Handle<StandardMaterial>,
    #[cfg(any(target_arch = "wasm32", test))]
    cast_shadow_mat: Handle<StandardMaterial>,
    gib_mat: Handle<StandardMaterial>,
    puff_mat: Handle<StandardMaterial>,
}

impl FromWorld for CritterAssets {
    fn from_world(world: &mut World) -> Self {
        world.init_resource::<Assets<Image>>();
        world.init_resource::<ToyShadingAssets>();
        let (contact_shadow_mesh, contact_shadow_mat) = {
            let toy = world.resource::<ToyShadingAssets>();
            (toy.contact_plane.clone(), toy.contact_material.clone())
        };
        #[cfg(any(target_arch = "wasm32", test))]
        let (cast_shadow_mesh, cast_shadow_mat) = {
            let toy = world.resource::<ToyShadingAssets>();
            (toy.cast_plane.clone(), toy.cast_material.clone())
        };

        // Build meshes + materials together inside a `resource_scope` so we
        // never hold `&mut Assets<Mesh>` and `&mut Assets<StandardMaterial>`
        // without scoping (mirrors `chickens.rs::ChickenAssets`).
        world.resource_scope::<Assets<Mesh>, _>(|world, mut meshes| {
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();

            // --- Pedestrian meshes ---
            // Body: capsule (radius 0.18, cylinder length 0.4) — reads as a
            // standing torso. Head: sphere.
            let ped_body_mesh = meshes.add(Capsule3d::new(0.18, 0.4));
            let ped_head_mesh = meshes.add(Sphere::new(0.14).mesh().uv(12, 8));

            // --- Cow meshes ---
            // Boxy body + boxy head + small sphere spots.
            let cow_body_mesh = meshes.add(Cuboid::new(0.70, 0.55, 1.10));
            let cow_head_mesh = meshes.add(Cuboid::new(0.28, 0.35, 0.32));
            let cow_spot_mesh = meshes.add(Sphere::new(0.12).mesh().uv(8, 6));

            // --- Moose meshes ---
            // Taller boxy body + head + thin cuboid antlers.
            let moose_body_mesh = meshes.add(Cuboid::new(0.55, 0.75, 1.20));
            let moose_head_mesh = meshes.add(Cuboid::new(0.32, 0.40, 0.38));
            let antler_mesh = meshes.add(Cuboid::new(0.04, 0.28, 0.04));

            // --- Shared meshes ---
            // Legs: cylinder (radius 0.06, height 0.4) — scaled per kind at
            // spawn. Eyes: tiny black spheres.
            let leg_mesh = meshes.add(Cylinder::new(0.06, 0.40));
            let eye_mesh = meshes.add(Sphere::new(0.035).mesh().uv(6, 4));

            // --- Particle meshes ---
            let gib_mesh = meshes.add(Sphere::new(0.09).mesh().uv(6, 4));
            let puff_mesh = meshes.add(Plane3d::default().mesh().size(0.3, 0.3));

            // --- Materials: pedestrian (neutral clothing + skin) ---
            let ped_body_mat = materials.add(toy_material(
                ToyMaterialFamily::CoatedPlastic,
                StandardMaterial {
                    base_color: Color::srgb(0.30, 0.40, 0.70), // blue shirt
                    ..default()
                },
            ));
            let ped_head_mat = materials.add(toy_material(
                ToyMaterialFamily::Clay,
                StandardMaterial {
                    base_color: Color::srgb(0.85, 0.70, 0.55), // skin
                    ..default()
                },
            ));
            let ped_leg_mat = materials.add(toy_material(
                ToyMaterialFamily::CoatedPlastic,
                StandardMaterial {
                    base_color: Color::srgb(0.20, 0.20, 0.25), // dark pants
                    ..default()
                },
            ));

            // --- Materials: cow (white body + black spots + pinkish head) ---
            let cow_body_mat = materials.add(toy_material(
                ToyMaterialFamily::CoatedPlastic,
                StandardMaterial {
                    base_color: Color::srgb(0.92, 0.92, 0.90),
                    ..default()
                },
            ));
            let cow_spot_mat = materials.add(toy_material(
                ToyMaterialFamily::CoatedPlastic,
                StandardMaterial {
                    base_color: Color::srgb(0.05, 0.05, 0.05),
                    ..default()
                },
            ));
            let cow_head_mat = materials.add(toy_material(
                ToyMaterialFamily::Clay,
                StandardMaterial {
                    base_color: Color::srgb(0.82, 0.68, 0.62), // pinkish
                    ..default()
                },
            ));

            // --- Materials: shared animal legs (dark brown, cow + moose) ---
            let animal_leg_mat = materials.add(toy_material(
                ToyMaterialFamily::RawWood,
                StandardMaterial {
                    base_color: Color::srgb(0.15, 0.12, 0.10),
                    ..default()
                },
            ));

            // --- Materials: moose (brown body + darker head + tan antlers) ---
            let moose_body_mat = materials.add(toy_material(
                ToyMaterialFamily::RawWood,
                StandardMaterial {
                    base_color: Color::srgb(0.35, 0.25, 0.15), // brown
                    ..default()
                },
            ));
            let moose_head_mat = materials.add(toy_material(
                ToyMaterialFamily::RawWood,
                StandardMaterial {
                    base_color: Color::srgb(0.28, 0.20, 0.12), // dark brown
                    ..default()
                },
            ));
            let antler_mat = materials.add(toy_material(
                ToyMaterialFamily::RawWood,
                StandardMaterial {
                    base_color: Color::srgb(0.65, 0.55, 0.40), // tan
                    ..default()
                },
            ));

            // --- Materials: shared eyes + shadow + particles ---
            let eye_mat = materials.add(toy_material(
                ToyMaterialFamily::CoatedPlastic,
                StandardMaterial {
                    base_color: Color::srgb(0.02, 0.02, 0.02),
                    ..default()
                },
            ));
            // Red gib + red smoke for the penalty burst.
            let gib_mat = materials.add(toy_material(
                ToyMaterialFamily::Clay,
                StandardMaterial {
                    base_color: Color::srgb(0.80, 0.10, 0.08),
                    ..default()
                },
            ));
            let puff_mat = materials.add(toy_material(
                ToyMaterialFamily::Clay,
                StandardMaterial {
                    base_color: Color::srgba(0.70, 0.10, 0.08, 0.50),
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                },
            ));

            CritterAssets {
                ped_body_mesh,
                ped_head_mesh,
                cow_body_mesh,
                cow_head_mesh,
                cow_spot_mesh,
                moose_body_mesh,
                moose_head_mesh,
                antler_mesh,
                leg_mesh,
                eye_mesh,
                contact_shadow_mesh,
                #[cfg(any(target_arch = "wasm32", test))]
                cast_shadow_mesh,
                gib_mesh,
                puff_mesh,
                ped_body_mat,
                ped_head_mat,
                ped_leg_mat,
                cow_body_mat,
                cow_spot_mat,
                cow_head_mat,
                animal_leg_mat,
                moose_body_mat,
                moose_head_mat,
                antler_mat,
                eye_mat,
                contact_shadow_mat,
                #[cfg(any(target_arch = "wasm32", test))]
                cast_shadow_mat,
                gib_mat,
                puff_mat,
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct CrittersPlugin;

impl Plugin for CrittersPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<CritterHit>()
            .init_resource::<CritterAssets>()
            .init_resource::<CritterPenaltyCooldown>()
            .init_resource::<CritterSpawnPending>()
            // Fresh-round spawn: inside SpawnSet and guarded by a cleanup-
            // driven latch, so reset ordering and pause/resume are both safe.
            .add_systems(OnEnter(GameState::Playing), spawn_critters.in_set(SpawnSet))
            // Hit detection runs before wandering (chained — they share
            // Transform access on Critter entities; ordering resolves the
            // borrow). update_particles is disjoint (Gib/BloodPuff comps) so
            // it runs concurrently.
            .add_systems(
                Update,
                (hit_critters, wander_critters)
                    .chain()
                    .after(DrivingSet)
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(
                Update,
                (spawn_critter_burst, update_particles).run_if(in_state(GameState::Playing)),
            )
            // Recursive despawn of all critters + particles on round end /
            // return to menu.
            .add_systems(
                OnEnter(GameState::GameOver),
                (cleanup_critters, cleanup_particles),
            )
            .add_systems(
                OnEnter(GameState::Menu),
                (cleanup_critters, cleanup_particles),
            );
    }
}

// ---------------------------------------------------------------------------
// Spawn systems
// ---------------------------------------------------------------------------

/// Fresh-round spawn: scatter the modifier-adjusted critter target (random
/// kinds) within radius `SCATTER_RADIUS` of the car. Runs in `SpawnSet` and
/// consumes the cleanup-driven fresh-round latch, independent of reset order.
fn spawn_critters(
    mut commands: Commands,
    assets: Res<CritterAssets>,
    cfg: Res<GameConfig>,
    modifier: Res<ActiveModifier>,
    car: Query<&Transform, (With<Car>, Without<Critter>)>,
    mut spawn_pending: ResMut<CritterSpawnPending>,
    mut penalty_cooldown: ResMut<CritterPenaltyCooldown>,
    mut seed: Local<u32>,
) {
    if !spawn_pending.0 {
        return;
    }
    penalty_cooldown.0 = 0.0;
    ensure_seeded(&mut seed, 0x2468_ACE0);
    let Ok(car_t) = car.single() else {
        return;
    };
    spawn_pending.0 = false;
    let car_pos = car_t.translation;
    let forward = horizontal_forward(car_t.rotation);

    for _ in 0..effective_critter_target(&modifier) {
        let angle = rand(&mut seed) * TAU;
        let radius = SCATTER_INNER + rand(&mut seed) * (SCATTER_RADIUS - SCATTER_INNER);
        let lateral = (angle.cos() * radius).clamp(-LATERAL_SPREAD, LATERAL_SPREAD);
        let longitudinal = angle.sin() * radius;
        let pos = car_relative_ground_pos(car_pos, forward, longitudinal, lateral);
        let kind = match (rand(&mut seed) * 3.0) as u32 {
            0 => CritterKind::Pedestrian,
            1 => CritterKind::Cow,
            _ => CritterKind::Moose,
        };
        spawn_one_critter(
            &mut commands,
            &assets,
            &cfg,
            pos,
            kind,
            rand_dir_xz(&mut seed),
            1.5 + rand(&mut seed) * 2.0,
            rand(&mut seed) * TAU,
        );
    }
}

/// Consume this system's independent event cursor and add one deterministic,
/// mixed group ahead of the car for each `CritterBurst` start. The read-only
/// car query and deferred spawns keep this system disjoint from existing
/// critters.
fn spawn_critter_burst(
    mut commands: Commands,
    assets: Res<CritterAssets>,
    cfg: Res<GameConfig>,
    car: Query<&Transform, (With<Car>, Without<Critter>)>,
    mut starts: MessageReader<RoundEventStarted>,
    mut seed: Local<u32>,
) {
    let additional_count: usize = starts
        .read()
        .map(|started| critter_burst_spawn_count(started.0))
        .sum();
    if additional_count == 0 {
        return;
    }

    let Ok(car_t) = car.single() else {
        return;
    };
    ensure_seeded(&mut seed, 0xC817_7E25);
    let car_pos = car_t.translation;
    let forward = horizontal_forward(car_t.rotation);

    for index in 0..additional_count {
        let entity = spawn_one_critter(
            &mut commands,
            &assets,
            &cfg,
            respawn_ahead_pos(car_pos, forward, &mut seed),
            burst_critter_kind(index),
            rand_dir_xz(&mut seed),
            1.5 + rand(&mut seed) * 2.0,
            rand(&mut seed) * TAU,
        );
        commands.entity(entity).insert(CritterBurstExtra);
    }
}

/// Build one critter as a parent + children hierarchy. The body group
/// (`CritterBody`) is animated by `wander_critters`: its `translation.y` bobs
/// and its `rotation.z` sways with the waddle phase.
///
/// Hierarchy:
/// ```text
/// critter_root (Transform: world pos + heading, Critter, CritterKind, Visibility)
///   ├── CritterBody { base_y } (bob group — animated each frame)
///   │     └── kind-specific parts (body mesh, head, spots/antlers, eyes)
///   ├── leg × N (Cylinders, children of root — stay grounded)
///   ├── one shared soft contact card
///   └── one world-fixed projected card for every critter on WebGL2
/// ```
fn spawn_one_critter(
    commands: &mut Commands,
    assets: &CritterAssets,
    cfg: &GameConfig,
    pos: Vec3,
    kind: CritterKind,
    dir: Vec3,
    timer: f32,
    bob: f32,
) -> Entity {
    let speed = critter_speed(kind, cfg);
    let base_y = critter_body_base_y(kind);
    #[cfg(any(target_arch = "wasm32", test))]
    let owner_transform = Transform::from_translation(pos);
    #[cfg(any(target_arch = "wasm32", test))]
    let cast_transform = critter_projected_shadow_local_transform(kind, &owner_transform);

    commands
        .spawn((
            Transform::from_translation(pos),
            Visibility::default(),
            Critter {
                dir,
                speed,
                timer,
                bob,
            },
            kind,
        ))
        .with_children(|root| {
            // --- Bob-animated body group (kind-specific parts nest under it) ---
            root.spawn((
                Transform::from_xyz(0.0, base_y, 0.0),
                Visibility::default(),
                CritterBody { base_y },
            ))
            .with_children(|body| match kind {
                CritterKind::Pedestrian => build_pedestrian(body, assets),
                CritterKind::Cow => build_cow(body, assets),
                CritterKind::Moose => build_moose(body, assets),
            });

            // --- Legs (children of root, not the bob group — stay grounded) ---
            build_legs(root, assets, kind);

            // Every kind retains exactly one contact card at the old
            // footprint. The shared alpha texture supplies the soft edge.
            let (sw, sl) = critter_shadow_size(kind);
            root.spawn((
                Mesh3d(assets.contact_shadow_mesh.clone()),
                MeshMaterial3d(assets.contact_shadow_mat.clone()),
                contact_shadow_transform(Vec2::new(sw, sl), 0.0),
                ToyContactShadow,
            ));

            // WebGL2 has no real-time shadow maps, so every critter gets a
            // classical projection in addition to its contact card.
            #[cfg(any(target_arch = "wasm32", test))]
            if critter_has_projected_shadow(kind) {
                root.spawn((
                    Mesh3d(assets.cast_shadow_mesh.clone()),
                    MeshMaterial3d(assets.cast_shadow_mat.clone()),
                    cast_transform,
                    ToyCastShadow,
                    CritterCastShadow,
                ));
            }
        })
        .id()
}

/// Pedestrian model: capsule torso + sphere head + dark pants legs. Front
/// (face direction) is -Z.
fn build_pedestrian(body: &mut ChildSpawnerCommands, assets: &CritterAssets) {
    // Torso (capsule).
    body.spawn((
        Mesh3d(assets.ped_body_mesh.clone()),
        MeshMaterial3d(assets.ped_body_mat.clone()),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));
    // Head (sphere) on top, front-facing.
    body.spawn((
        Mesh3d(assets.ped_head_mesh.clone()),
        MeshMaterial3d(assets.ped_head_mat.clone()),
        Transform::from_xyz(0.0, 0.42, 0.0),
    ))
    .with_children(|head| {
        // Eyes on the front of the head (front = -Z).
        for &x in &[-0.06_f32, 0.06] {
            head.spawn((
                Mesh3d(assets.eye_mesh.clone()),
                MeshMaterial3d(assets.eye_mat.clone()),
                Transform::from_xyz(x, 0.03, -0.12),
            ));
        }
    });
}

/// Cow model: boxy white body + pinkish head + black sphere spots + 4 dark
/// legs. Front (face direction) is -Z.
fn build_cow(body: &mut ChildSpawnerCommands, assets: &CritterAssets) {
    // Body (boxy).
    body.spawn((
        Mesh3d(assets.cow_body_mesh.clone()),
        MeshMaterial3d(assets.cow_body_mat.clone()),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));
    // Head at the front (-Z).
    body.spawn((
        Mesh3d(assets.cow_head_mesh.clone()),
        MeshMaterial3d(assets.cow_head_mat.clone()),
        Transform::from_xyz(0.0, 0.08, -0.65),
    ))
    .with_children(|head| {
        // Eyes on the front of the head.
        for &x in &[-0.12_f32, 0.12] {
            head.spawn((
                Mesh3d(assets.eye_mesh.clone()),
                MeshMaterial3d(assets.eye_mat.clone()),
                Transform::from_xyz(x, 0.08, -0.14),
            ));
        }
    });
    // Black spots scattered on the body surface.
    for &(x, y, z) in &[
        (0.22_f32, 0.22_f32, 0.10_f32),
        (-0.18_f32, 0.25_f32, 0.30_f32),
        (0.12_f32, -0.24_f32, -0.15_f32),
        (-0.28_f32, 0.05_f32, 0.05_f32),
        (0.05_f32, 0.27_f32, -0.35_f32),
    ] {
        body.spawn((
            Mesh3d(assets.cow_spot_mesh.clone()),
            MeshMaterial3d(assets.cow_spot_mat.clone()),
            Transform::from_xyz(x, y, z),
        ));
    }
}

/// Moose model: tall brown body + darker head + tan antlers + 4 dark legs.
/// Front (face direction) is -Z.
fn build_moose(body: &mut ChildSpawnerCommands, assets: &CritterAssets) {
    // Body (tall boxy).
    body.spawn((
        Mesh3d(assets.moose_body_mesh.clone()),
        MeshMaterial3d(assets.moose_body_mat.clone()),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));
    // Head at the front (-Z), lowered slightly.
    body.spawn((
        Mesh3d(assets.moose_head_mesh.clone()),
        MeshMaterial3d(assets.moose_head_mat.clone()),
        Transform::from_xyz(0.0, 0.10, -0.68),
    ))
    .with_children(|head| {
        // Eyes on the front of the head.
        for &x in &[-0.13_f32, 0.13] {
            head.spawn((
                Mesh3d(assets.eye_mesh.clone()),
                MeshMaterial3d(assets.eye_mat.clone()),
                Transform::from_xyz(x, 0.08, -0.16),
            ));
        }
        // Antlers: two thin cuboids angling up + outward from the head top.
        for &x in &[-0.20_f32, 0.20] {
            // Main antler beam (vertical).
            head.spawn((
                Mesh3d(assets.antler_mesh.clone()),
                MeshMaterial3d(assets.antler_mat.clone()),
                Transform::from_xyz(x, 0.28, -0.05),
            ));
            // Forward branch (angled).
            head.spawn((
                Mesh3d(assets.antler_mesh.clone()),
                MeshMaterial3d(assets.antler_mat.clone()),
                Transform::from_xyz(x, 0.34, -0.12).with_rotation(Quat::from_rotation_x(0.5)),
            ));
        }
    });
}

/// Spawn the legs for a critter as children of the root (so they stay
/// grounded while the body group bobs). Leg count + position + scale vary by
/// kind; all reuse the shared cylinder `leg_mesh`.
fn build_legs(root: &mut ChildSpawnerCommands, assets: &CritterAssets, kind: CritterKind) {
    // Use slices (not fixed arrays) so the 2-leg + 4-leg variants share a
    // common type in the match.
    let (mat, positions, scale): (Handle<StandardMaterial>, &[(f32, f32)], Vec3) = match kind {
        // Pedestrian: 2 legs, pants material, thin + short.
        CritterKind::Pedestrian => (
            assets.ped_leg_mat.clone(),
            &[(-0.08_f32, 0.0_f32), (0.08_f32, 0.0_f32)],
            Vec3::new(0.8, 0.75, 0.8), // radius 0.06*0.8=0.048, height 0.4*0.75=0.30
        ),
        // Cow: 4 legs, dark animal material, stocky.
        CritterKind::Cow => (
            assets.animal_leg_mat.clone(),
            &[
                (-0.25_f32, -0.40_f32),
                (0.25_f32, -0.40_f32),
                (-0.25_f32, 0.40_f32),
                (0.25_f32, 0.40_f32),
            ],
            Vec3::new(1.2, 1.0, 1.2), // radius 0.072, height 0.40
        ),
        // Moose: 4 legs, dark animal material, longer.
        CritterKind::Moose => (
            assets.animal_leg_mat.clone(),
            &[
                (-0.18_f32, -0.45_f32),
                (0.18_f32, -0.45_f32),
                (-0.18_f32, 0.45_f32),
                (0.18_f32, 0.45_f32),
            ],
            Vec3::new(1.2, 1.35, 1.2), // radius 0.072, height 0.54
        ),
    };
    // Leg Y center = half of the (scaled) leg height, so the cylinder sits
    // on the ground (y=0). Scaled height = 0.40 * scale.y.
    let leg_y = 0.40 * scale.y * 0.5;
    for &(x, z) in positions {
        root.spawn((
            Mesh3d(assets.leg_mesh.clone()),
            MeshMaterial3d(mat.clone()),
            Transform::from_xyz(x, leg_y, z).with_scale(scale),
        ));
    }
}

// ---------------------------------------------------------------------------
// Wander system
// ---------------------------------------------------------------------------

/// Move critters by `dir` at their per-critter `speed`, periodically pick a
/// new random heading, face it, recycle critters that fall behind / drift
/// beyond `KEEP_RADIUS`, and animate the waddle bob on the body child.
fn wander_critters(
    mut commands: Commands,
    assets: Res<CritterAssets>,
    cfg: Res<GameConfig>,
    car: Query<&Transform, (With<Car>, Without<Critter>)>,
    mut critters: Query<
        (
            Entity,
            &mut Critter,
            &CritterKind,
            &mut Transform,
            &Children,
            Has<CritterBurstExtra>,
        ),
        Without<Car>,
    >,
    mut bodies: Query<
        (&mut Transform, &CritterBody),
        (Without<Critter>, Without<Car>, Without<CritterCastShadow>),
    >,
    #[cfg(any(target_arch = "wasm32", test))] mut cast_shadows: Query<
        &mut Transform,
        (
            With<CritterCastShadow>,
            Without<CritterBody>,
            Without<Critter>,
            Without<Car>,
        ),
    >,
    time: Res<Time>,
    settings: Res<Settings>,
    mut seed: Local<u32>,
) {
    ensure_seeded(&mut seed, 0x1357_9BDF);
    let Ok(car_t) = car.single() else {
        return;
    };
    let car_pos = car_t.translation;
    let car_forward = horizontal_forward(car_t.rotation);
    let dt = time.delta_secs();

    for (e, mut critter, _kind, mut tf, children, burst_extra) in &mut critters {
        // --- Periodically pick a new random heading ---
        critter.timer -= dt;
        if critter.timer <= 0.0 {
            critter.dir = rand_dir_xz(&mut seed);
            critter.timer = 1.5 + rand(&mut seed) * 2.0;
        }

        // --- Move (XZ plane only; y stays 0) ---
        tf.translation += critter.dir * critter.speed * dt;

        // --- Face the heading (rotate Y so the face points along dir) ---
        let heading = (-critter.dir.x).atan2(-critter.dir.z);
        tf.rotation = Quat::from_rotation_y(heading);

        #[cfg(any(target_arch = "wasm32", test))]
        for child_e in children.iter() {
            if let Ok(mut shadow_tf) = cast_shadows.get_mut(child_e) {
                *shadow_tf = critter_projected_shadow_local_transform(*_kind, &tf);
            }
        }

        // --- Recycle critters that fell behind or drifted too far away ---
        if should_recycle(tf.translation, car_pos, car_forward) {
            commands.entity(e).despawn();
            if !burst_extra {
                let new_pos = respawn_ahead_pos(car_pos, car_forward, &mut seed);
                let kind = match (rand(&mut seed) * 3.0) as u32 {
                    0 => CritterKind::Pedestrian,
                    1 => CritterKind::Cow,
                    _ => CritterKind::Moose,
                };
                spawn_one_critter(
                    &mut commands,
                    &assets,
                    &cfg,
                    new_pos,
                    kind,
                    rand_dir_xz(&mut seed),
                    1.5 + rand(&mut seed) * 2.0,
                    rand(&mut seed) * TAU,
                );
            }
            continue;
        }

        // --- Waddle: accessibility can freeze the visual body motion only. ---
        if !settings.reduced_motion {
            critter.bob += dt * WADDLE_SPEED;
        }
        for child_e in children.iter() {
            if let Ok((mut body_tf, body)) = bodies.get_mut(child_e) {
                let (y, rotation) =
                    creature_visual_pose(body.base_y, critter.bob, settings.reduced_motion);
                body_tf.translation.y = y;
                body_tf.rotation = rotation;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hit system (PENALTY)
// ---------------------------------------------------------------------------

/// On car-to-critter contact (XZ distance < `HIT_RADIUS`): despawn the
/// critter, apply a cooldown-gated **PENALTY** (25 base health scaled by the
/// active modifier, score -2),
/// write a `CritterHit` message, spawn a **red** particle burst, and respawn
/// ahead. The penalty-hit thud SFX is owned and played by the audio core in
/// reaction to the `CritterHit` message. Visual/message handling is retained
/// for every clustered contact; reduced motion suppresses only the visual
/// particles. Lethal admitted damage ends the round as Wrecked.
fn hit_critters(
    mut commands: Commands,
    assets: Res<CritterAssets>,
    cfg: Res<GameConfig>,
    modifier: Res<ActiveModifier>,
    mut car: Query<(&mut Car, &Transform), Without<Critter>>,
    critters: Query<(Entity, &Transform, &CritterKind, Has<CritterBurstExtra>), With<Critter>>,
    mut health: ResMut<Health>,
    mut scoring: CritterScoring,
    mut critter_hits: MessageWriter<CritterHit>,
    mut next: ResMut<NextState<GameState>>,
    mut reason: ResMut<GameOverReason>,
    time: Res<Time>,
    settings: Res<Settings>,
    mut penalty_cooldown: ResMut<CritterPenaltyCooldown>,
    drowning: Res<Drowning>,
    mut seed: Local<u32>,
) {
    if drowning.active {
        return;
    }
    ensure_seeded(&mut seed, 0xACE0_2468);
    penalty_cooldown.0 = (penalty_cooldown.0 - time.delta_secs()).max(0.0);
    let Ok((mut car, car_t)) = car.single_mut() else {
        return;
    };
    let car_pos = car_t.translation;
    let car_forward = horizontal_forward(car_t.rotation);
    let hit_r2 = HIT_RADIUS * HIT_RADIUS;

    for (e, critter_t, &kind, burst_extra) in &critters {
        let dx = car_pos.x - critter_t.translation.x;
        let dz = car_pos.z - critter_t.translation.z;
        if dx * dx + dz * dz < hit_r2 {
            // --- Despawn the hit critter ---
            commands.entity(e).despawn();

            // --- PENALTY: one health/score loss per short contact window ---
            // Clustered critters are still visibly hit below, but cannot stack
            // several penalties in the same frame or immediate aftermath.
            let decision =
                critter_damage_decision(health.0, penalty_cooldown.0, modifier.damage_multiplier());
            if decision.apply {
                health.0 = decision.health;
                if !scoring
                    .rules
                    .as_ref()
                    .is_some_and(|rules| rules.conduct == Conduct::RightOfWay)
                {
                    scoring.score.chickens =
                        scoring.score.chickens.saturating_sub(HIT_SCORE_PENALTY);
                }
                penalty_cooldown.0 = HIT_PENALTY_COOLDOWN;

                if decision.lethal {
                    car.speed = 0.0;
                    *reason = GameOverReason::Wrecked;
                    next.set(GameState::GameOver);
                }
            }

            // --- Write the message (audio.rs / UI can react) ---
            critter_hits.write(CritterHit);

            // Consume the same random sequence either way so this visual
            // preference cannot alter replacement gameplay placement.
            spawn_particle_burst(
                &mut commands,
                &assets,
                critter_t.translation,
                &mut seed,
                hit_particles_enabled(settings.reduced_motion),
            );

            // --- Respawn a critter ahead so there's always something to avoid ---
            let new_pos = respawn_ahead_pos(car_pos, car_forward, &mut seed);
            let replacement = spawn_one_critter(
                &mut commands,
                &assets,
                &cfg,
                new_pos,
                kind,
                rand_dir_xz(&mut seed),
                1.5 + rand(&mut seed) * 2.0,
                rand(&mut seed) * TAU,
            );
            if burst_extra {
                commands.entity(replacement).insert(CritterBurstExtra);
            }
        }
    }
}

/// Consume one penalty-burst random sequence and, when enabled, spawn ~8 red
/// gib spheres plus a few red smoke puffs. This preserves gameplay RNG when
/// reduced motion suppresses the visual entities.
fn spawn_particle_burst(
    commands: &mut Commands,
    assets: &CritterAssets,
    pos: Vec3,
    seed: &mut u32,
    enabled: bool,
) {
    let body_pos = pos + Vec3::new(0.0, 0.30, 0.0);
    let ground_pos = pos + Vec3::new(0.0, 0.10, 0.0);

    for _ in 0..GIB_COUNT {
        let angle = rand(seed) * TAU;
        let horiz_speed = 1.5 + rand(seed) * 2.5;
        let vel = Vec3::new(
            angle.cos() * horiz_speed,
            2.0 + rand(seed) * 2.5, // upward pop
            angle.sin() * horiz_speed,
        );
        let spin = (rand(seed) * 2.0 - 1.0) * 10.0;
        if enabled {
            commands.spawn((
                Mesh3d(assets.gib_mesh.clone()),
                MeshMaterial3d(assets.gib_mat.clone()),
                Transform::from_translation(body_pos),
                Gib {
                    vel,
                    life: GIB_LIFE,
                    spin,
                },
            ));
        }
    }

    for _ in 0..PUFF_COUNT {
        let angle = rand(seed) * TAU;
        let horiz_speed = 0.5 + rand(seed) * 1.0;
        let vel = Vec3::new(
            angle.cos() * horiz_speed,
            0.5 + rand(seed) * 0.5,
            angle.sin() * horiz_speed,
        );
        if enabled {
            commands.spawn((
                Mesh3d(assets.puff_mesh.clone()),
                MeshMaterial3d(assets.puff_mat.clone()),
                Transform::from_translation(ground_pos),
                BloodPuff {
                    vel,
                    life: PUFF_LIFE,
                    max_life: PUFF_LIFE,
                },
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Particle update
// ---------------------------------------------------------------------------

/// Advance gib + puff particles: gravity, motion, spin, expansion, and
/// despawn when `life` reaches 0. Runs only during `Playing`.
fn update_particles(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<Settings>,
    mut gibs: Query<(Entity, &mut Transform, &mut Gib)>,
    mut puffs: Query<(Entity, &mut Transform, &mut BloodPuff), Without<Gib>>,
) {
    let dt = time.delta_secs();
    let t = time.elapsed_secs();

    if !hit_particles_enabled(settings.reduced_motion) {
        for (e, _, _) in &mut gibs {
            commands.entity(e).despawn();
        }
        for (e, _, _) in &mut puffs {
            commands.entity(e).despawn();
        }
        return;
    }

    for (e, mut tf, mut gib) in &mut gibs {
        gib.life -= dt;
        if gib.life <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        gib.vel.y -= GIB_GRAVITY * dt;
        tf.translation += gib.vel * dt;
        // Tumble for visual interest.
        tf.rotation =
            Quat::from_rotation_y(t * gib.spin) * Quat::from_rotation_x(t * gib.spin * 0.7);
        // Don't sink through the ground.
        if tf.translation.y < 0.05 {
            tf.translation.y = 0.05;
            gib.vel.y = 0.0;
        }
    }

    for (e, mut tf, mut puff) in &mut puffs {
        puff.life -= dt;
        if puff.life <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        // Air drag — puff decelerates as it expands.
        puff.vel *= (1.0 - 2.0 * dt).max(0.0);
        tf.translation += puff.vel * dt;
        // Expand as it fades (frac goes 0 → 1 over the puff's life).
        let frac = 1.0 - puff.life / puff.max_life;
        tf.scale = Vec3::splat(1.0 + frac * 1.5);
    }
}

// ---------------------------------------------------------------------------
// Cleanup systems
// ---------------------------------------------------------------------------

/// Despawn every critter (recursive — nukes the mesh hierarchy, risk E2).
fn cleanup_critters(
    mut commands: Commands,
    critters: Query<Entity, With<Critter>>,
    mut spawn_pending: ResMut<CritterSpawnPending>,
) {
    for e in &critters {
        commands.entity(e).despawn();
    }
    spawn_pending.0 = true;
}

/// Despawn every lingering gib + puff particle.
fn cleanup_particles(
    mut commands: Commands,
    gibs: Query<Entity, With<Gib>>,
    puffs: Query<Entity, With<BloodPuff>>,
) {
    for e in &gibs {
        commands.entity(e).despawn();
    }
    for e in &puffs {
        commands.entity(e).despawn();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const fn hit_particles_enabled(reduced_motion: bool) -> bool {
    !reduced_motion
}

/// Body-only visual pose; gameplay movement and facing remain unchanged.
fn creature_visual_pose(base_y: f32, bob: f32, reduced_motion: bool) -> (f32, Quat) {
    if reduced_motion {
        (base_y, Quat::IDENTITY)
    } else {
        let waddle = bob.sin();
        (base_y + waddle * 0.05, Quat::from_rotation_z(waddle * 0.08))
    }
}

/// Number of additional critters produced by a single event-start message.
/// Keeping this independent from `ActiveEvent` prevents mid-run events from
/// changing the fresh-round population target.
const fn critter_burst_spawn_count(kind: EventKind) -> usize {
    match kind {
        EventKind::CritterBurst => CRITTER_COUNT,
        _ => 0,
    }
}

/// Stable mixed ordering guarantees every burst contains all three kinds.
const fn burst_critter_kind(index: usize) -> CritterKind {
    match index % 3 {
        0 => CritterKind::Pedestrian,
        1 => CritterKind::Cow,
        _ => CritterKind::Moose,
    }
}

/// Fresh-round critter population after applying the active road condition.
/// Baseline recycling and hits replace exactly one critter; burst extras are
/// deliberately outside this target and retire through natural recycling.
const fn effective_critter_target(modifier: &ActiveModifier) -> usize {
    CRITTER_COUNT * modifier.critter_count_multiplier()
}

/// Tiny LCG (matches `world.rs::rand`) — deterministic pseudo-random 0..1
/// without pulling in the `rand` crate (keeps the web build lean).
fn rand(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
    (*seed as f32) / (u32::MAX as f32)
}

/// Random unit vector in the XZ plane (y = 0).
fn rand_dir_xz(seed: &mut u32) -> Vec3 {
    let angle = rand(seed) * TAU;
    Vec3::new(angle.cos(), 0.0, angle.sin())
}

/// Horizontal unit forward for a car transform.
fn horizontal_forward(rotation: Quat) -> Vec3 {
    normalized_horizontal(rotation * Vec3::NEG_Z)
}

fn normalized_horizontal(direction: Vec3) -> Vec3 {
    let horizontal = Vec3::new(direction.x, 0.0, direction.z);
    if horizontal.length_squared() > f32::EPSILON {
        horizontal.normalize()
    } else {
        Vec3::NEG_Z
    }
}

/// Place a point in the car's heading-relative frame. Positive `ahead` follows
/// forward and positive `lateral` follows local +X (the car's right side).
fn car_relative_ground_pos(car_pos: Vec3, forward: Vec3, ahead: f32, lateral: f32) -> Vec3 {
    let forward = normalized_horizontal(forward);
    let right = Vec3::new(-forward.z, 0.0, forward.x);
    let mut pos =
        car_pos + forward * ahead + right * lateral.clamp(-LATERAL_SPREAD, LATERAL_SPREAD);
    pos.y = 0.0;
    pos
}

/// Whether a point has fallen behind the car along the car's current heading.
fn is_behind_car(pos: Vec3, car_pos: Vec3, car_forward: Vec3) -> bool {
    (pos - car_pos).dot(normalized_horizontal(car_forward)) < -BEHIND_THRESHOLD
}

fn should_recycle(pos: Vec3, car_pos: Vec3, car_forward: Vec3) -> bool {
    pos.distance(car_pos) > KEEP_RADIUS || is_behind_car(pos, car_pos, car_forward)
}

/// A position in the explicit offscreen envelope ahead of the car. The
/// minimum forward projection clears the camera and hit safety radii, while
/// bounded lateral spread keeps the far corner within `KEEP_RADIUS`.
fn respawn_ahead_pos(car_pos: Vec3, car_forward: Vec3, seed: &mut u32) -> Vec3 {
    let ahead = RESPAWN_AHEAD_MIN + rand(seed) * RESPAWN_AHEAD_RANGE;
    let lateral = (rand(seed) * 2.0 - 1.0) * LATERAL_SPREAD;
    let pos = car_relative_ground_pos(car_pos, car_forward, ahead, lateral);
    debug_assert!(is_safely_offscreen(pos, car_pos, car_forward));
    pos
}

/// Pure ground-plane camera-envelope check used by runtime assertions and
/// deterministic seed/heading tests.
fn is_safely_offscreen(pos: Vec3, car_pos: Vec3, car_forward: Vec3) -> bool {
    let delta = Vec3::new(pos.x - car_pos.x, 0.0, pos.z - car_pos.z);
    delta.dot(normalized_horizontal(car_forward)) > VISIBLE_VIEW_RADIUS
        && delta.length() > VISIBLE_VIEW_RADIUS
        && delta.length() > HIT_RADIUS
}

/// Pure contact-window decision used by the hit system and unit tests.
fn should_apply_penalty(cooldown_remaining: f32) -> bool {
    cooldown_remaining <= 0.0
}

#[derive(Debug, PartialEq)]
struct CritterDamageDecision {
    apply: bool,
    health: f32,
    lethal: bool,
}

/// Pure cooldown/damage decision. A blocked hit preserves bounded health; an
/// admitted hit applies modifier-scaled base damage with finite arithmetic,
/// clamps health to its representable non-negative range, and reports lethal
/// boundaries for the GameOver transition.
fn critter_damage_decision(
    health: f32,
    cooldown_remaining: f32,
    damage_multiplier: f32,
) -> CritterDamageDecision {
    let bounded_health = if health.is_nan() {
        0.0
    } else {
        health.clamp(0.0, f32::MAX)
    };
    if !should_apply_penalty(cooldown_remaining) {
        return CritterDamageDecision {
            apply: false,
            health: bounded_health,
            lethal: false,
        };
    }

    let multiplier = if damage_multiplier.is_nan() || damage_multiplier <= 0.0 {
        0.0
    } else {
        damage_multiplier
    };
    let damage = ((HIT_HEALTH_PENALTY as f64) * (multiplier as f64)).clamp(0.0, f32::MAX as f64);
    let health = ((bounded_health as f64) - damage).clamp(0.0, f32::MAX as f64) as f32;
    CritterDamageDecision {
        apply: true,
        health,
        lethal: health <= 0.0,
    }
}

/// Seed a `Local<u32>` RNG on first use with a per-system constant so the
/// systems' sequences don't start correlated (the LCG never produces 0 from a
/// non-zero seed, so this fires exactly once per system).
fn ensure_seeded(seed: &mut u32, initial: u32) {
    if *seed == 0 {
        *seed = initial;
    }
}

/// Per-kind wander speed (units/second), scaled by `GameConfig::max_speed` so
/// critters stay proportional to the car if the config is tuned.
fn critter_speed(kind: CritterKind, cfg: &GameConfig) -> f32 {
    match kind {
        CritterKind::Pedestrian => cfg.max_speed * 0.15, // ~1.8 u/s
        CritterKind::Cow => cfg.max_speed * 0.08,        // ~0.96 u/s
        CritterKind::Moose => cfg.max_speed * 0.12,      // ~1.44 u/s
    }
}

/// Resting Y offset of the body group (where the body mesh center sits above
/// the ground). Varies by kind — taller animals have higher body groups.
fn critter_body_base_y(kind: CritterKind) -> f32 {
    match kind {
        CritterKind::Pedestrian => 0.50,
        CritterKind::Cow => 0.55,
        CritterKind::Moose => 0.75,
    }
}

/// Contact card (width, length) per kind — preserving the prior blob sizes.
fn critter_shadow_size(kind: CritterKind) -> (f32, f32) {
    match kind {
        CritterKind::Pedestrian => (0.40, 0.40),
        CritterKind::Cow => (0.80, 1.20),
        CritterKind::Moose => (0.70, 1.30),
    }
}

#[cfg(any(target_arch = "wasm32", test))]
const fn critter_has_projected_shadow(_kind: CritterKind) -> bool {
    true
}

#[cfg(any(target_arch = "wasm32", test))]
const fn critter_caster_height(kind: CritterKind) -> f32 {
    match kind {
        CritterKind::Pedestrian => 1.05,
        CritterKind::Cow => 1.15,
        CritterKind::Moose => 1.65,
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn critter_projected_shadow_local_transform(kind: CritterKind, owner: &Transform) -> Transform {
    let (width, length) = critter_shadow_size(kind);
    let mut projected =
        projected_shadow_transform(Vec2::new(width, length), critter_caster_height(kind), 0.0);
    let inverse_owner_rotation = owner.rotation.inverse();
    projected.translation.y -= owner.translation.y;
    projected.translation = inverse_owner_rotation * projected.translation;
    projected.rotation = inverse_owner_rotation * projected.rotation;
    projected
}

#[cfg(test)]
const fn critter_shadow_card_count(kind: CritterKind, webgl: bool) -> usize {
    1 + (webgl && critter_has_projected_shadow(kind)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modifiers::ModifierKind;
    use std::f32::consts::{FRAC_PI_2, PI};

    #[test]
    fn critter_contact_sizes_and_platform_cardinality_are_preserved() {
        assert_eq!(critter_shadow_size(CritterKind::Pedestrian), (0.4, 0.4));
        assert_eq!(critter_shadow_size(CritterKind::Cow), (0.8, 1.2));
        assert_eq!(critter_shadow_size(CritterKind::Moose), (0.7, 1.3));
        for kind in [
            CritterKind::Pedestrian,
            CritterKind::Cow,
            CritterKind::Moose,
        ] {
            assert_eq!(critter_shadow_card_count(kind, false), 1);
            assert_eq!(critter_shadow_card_count(kind, true), 2);
        }
    }

    #[test]
    fn critter_projected_shadows_use_full_caster_heights() {
        assert_eq!(critter_caster_height(CritterKind::Pedestrian), 1.05);
        assert_eq!(critter_caster_height(CritterKind::Cow), 1.15);
        assert_eq!(critter_caster_height(CritterKind::Moose), 1.65);
    }

    #[test]
    fn turning_critters_keep_projected_cards_world_fixed() {
        for kind in [
            CritterKind::Pedestrian,
            CritterKind::Cow,
            CritterKind::Moose,
        ] {
            let (width, length) = critter_shadow_size(kind);
            let canonical = projected_shadow_transform(
                Vec2::new(width, length),
                critter_caster_height(kind),
                0.0,
            );
            for yaw in [0.0, FRAC_PI_2, PI, 3.0 * FRAC_PI_2] {
                let owner =
                    Transform::from_xyz(4.0, 0.35, -8.0).with_rotation(Quat::from_rotation_y(yaw));
                let local = critter_projected_shadow_local_transform(kind, &owner);
                let world_translation = owner.translation + owner.rotation * local.translation;
                let expected_translation = Vec3::new(owner.translation.x, 0.0, owner.translation.z)
                    + canonical.translation;
                assert!(world_translation.abs_diff_eq(expected_translation, 1e-5));
                assert!((owner.rotation * local.rotation).abs_diff_eq(canonical.rotation, 1e-5));
                assert_eq!(local.scale, canonical.scale);
            }
        }
    }

    #[test]
    fn critter_assets_reuse_global_toy_shadow_cache() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<ToyShadingAssets>()
            .init_resource::<CritterAssets>();
        let toy = app.world().resource::<ToyShadingAssets>();
        let critter = app.world().resource::<CritterAssets>();
        assert_eq!(critter.contact_shadow_mesh.id(), toy.contact_plane.id());
        assert_eq!(critter.cast_shadow_mesh.id(), toy.cast_plane.id());
        assert_eq!(critter.contact_shadow_mat.id(), toy.contact_material.id());
        assert_eq!(critter.cast_shadow_mat.id(), toy.cast_material.id());
    }

    #[test]
    fn reduced_motion_keeps_critter_body_static_and_suppresses_particles() {
        let (y, rotation) = creature_visual_pose(0.75, 2.1, true);
        assert_eq!(y, 0.75);
        assert_eq!(rotation, Quat::IDENTITY);
        assert!(!hit_particles_enabled(true));
        assert!(hit_particles_enabled(false));
    }

    #[test]
    fn only_critter_burst_spawns_one_mixed_base_count() {
        assert_eq!(
            critter_burst_spawn_count(EventKind::CritterBurst),
            CRITTER_COUNT
        );
        assert_eq!(critter_burst_spawn_count(EventKind::TrafficSurge), 0);
        assert_eq!(critter_burst_spawn_count(EventKind::ChickenBurst), 0);
        assert_eq!(critter_burst_spawn_count(EventKind::ComboFrenzy), 0);

        let kinds: Vec<_> = (0..CRITTER_COUNT).map(burst_critter_kind).collect();
        assert_eq!(
            kinds,
            vec![
                CritterKind::Pedestrian,
                CritterKind::Cow,
                CritterKind::Moose,
                CritterKind::Pedestrian,
                CritterKind::Cow,
            ]
        );
    }

    #[test]
    fn stampede_doubles_only_the_critter_population_target() {
        let standard = ActiveModifier(ModifierKind::Standard);
        let stampede = ActiveModifier(ModifierKind::Stampede);
        let frenzy = ActiveModifier(ModifierKind::ChickenFrenzy);

        assert_eq!(effective_critter_target(&standard), CRITTER_COUNT);
        assert_eq!(effective_critter_target(&stampede), CRITTER_COUNT * 2);
        assert_eq!(effective_critter_target(&frenzy), CRITTER_COUNT);
    }

    #[test]
    fn standard_and_glass_cannon_critter_damage_are_exact() {
        assert_eq!(
            critter_damage_decision(100.0, 0.0, ModifierKind::Standard.damage_multiplier()),
            CritterDamageDecision {
                apply: true,
                health: 75.0,
                lethal: false,
            }
        );
        assert_eq!(
            critter_damage_decision(100.0, 0.0, ModifierKind::GlassCannon.damage_multiplier(),),
            CritterDamageDecision {
                apply: true,
                health: 50.0,
                lethal: false,
            }
        );
    }

    #[test]
    fn critter_damage_lethal_boundaries_are_clamped() {
        assert_eq!(
            critter_damage_decision(25.0, 0.0, 1.0),
            CritterDamageDecision {
                apply: true,
                health: 0.0,
                lethal: true,
            }
        );
        assert_eq!(
            critter_damage_decision(25.5, 0.0, 1.0),
            CritterDamageDecision {
                apply: true,
                health: 0.5,
                lethal: false,
            }
        );
        assert_eq!(
            critter_damage_decision(10.0, 0.0, f32::INFINITY),
            CritterDamageDecision {
                apply: true,
                health: 0.0,
                lethal: true,
            }
        );
        assert_eq!(
            critter_damage_decision(f32::INFINITY, 0.0, 1.0),
            CritterDamageDecision {
                apply: true,
                health: f32::MAX,
                lethal: false,
            }
        );
    }

    #[test]
    fn cooldown_preserves_health_and_blocks_scaled_damage() {
        assert_eq!(
            critter_damage_decision(
                100.0,
                HIT_PENALTY_COOLDOWN,
                ModifierKind::GlassCannon.damage_multiplier(),
            ),
            CritterDamageDecision {
                apply: false,
                health: 100.0,
                lethal: false,
            }
        );
    }

    #[test]
    fn critter_ahead_placement_tracks_turned_heading() {
        let car_pos = Vec3::new(12.0, 0.0, -9.0);
        let forward = horizontal_forward(Quat::from_rotation_y(std::f32::consts::PI));
        let pos = car_relative_ground_pos(car_pos, forward, 20.0, 0.0);

        assert!((pos - Vec3::new(12.0, 0.0, 11.0)).length() < 0.0001);
    }

    #[test]
    fn critter_behind_check_is_heading_relative() {
        let car_pos = Vec3::new(-40.0, 0.0, 70.0);
        let south = Vec3::NEG_Z;

        assert!(is_behind_car(car_pos + Vec3::Z * 16.0, car_pos, south));
        assert!(!is_behind_car(car_pos - Vec3::Z * 16.0, car_pos, south));
    }

    #[test]
    fn critter_respawn_envelope_is_offscreen_for_headings_and_seeds() {
        let car_pos = Vec3::new(-13.0, 3.0, 29.0);
        let headings = [
            Vec3::NEG_Z,
            Vec3::X,
            Vec3::new(-0.8, 0.0, -0.6),
            Vec3::new(0.2, 9.0, 0.9),
        ];
        for heading in headings {
            let forward = normalized_horizontal(heading);
            let right = Vec3::new(-forward.z, 0.0, forward.x);
            for initial_seed in [1, 13, 0xACE0_2468, u32::MAX] {
                let mut seed = initial_seed;
                for _ in 0..16 {
                    let pos = respawn_ahead_pos(car_pos, heading, &mut seed);
                    let delta = Vec3::new(pos.x - car_pos.x, 0.0, pos.z - car_pos.z);
                    let ahead = delta.dot(forward);
                    let lateral = delta.dot(right);
                    assert!(is_safely_offscreen(pos, car_pos, heading));
                    assert!(ahead >= RESPAWN_AHEAD_MIN - 0.0001);
                    assert!(ahead <= RESPAWN_AHEAD_MIN + RESPAWN_AHEAD_RANGE + 0.0001);
                    assert!(lateral.abs() <= LATERAL_SPREAD + 0.0001);
                    assert!(delta.length() < KEEP_RADIUS);
                    assert!(!should_recycle(pos, car_pos, heading));
                }
            }
        }
    }
}
