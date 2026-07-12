//! AI critters (pedestrian / cow / moose) that wander near the roads and
//! that you must **NOT** hit.
//!
//! Distinct from chickens (which **award** score): hitting a critter is a
//! **PENALTY** — the car loses 25 health and 2 chicken-score, a red particle
//! burst fires, a bad-thud SFX plays, and a `CritterHit` message is written.
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
//! - `spawn_critters` runs `.in_set(SpawnSet)` and checks `RoundActive.0` to
//!   skip on resume from `Paused`.
//! - Critter entities have **no** `Collider` component.

use bevy::audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;
use std::f32::consts::TAU;

use crate::car::Car;
use crate::game::resources::{GameConfig, RoundActive, Score};
use crate::game::state::GameState;
use crate::game::SpawnSet;
use crate::health::Health;

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
const KEEP_RADIUS: f32 = 50.0;

/// Critters this far **behind** the car (in +Z) are recycled even if within
/// `KEEP_RADIUS` — the car drives toward -Z, so a critter at `z > car.z + 15`
/// has been left behind.
const BEHIND_THRESHOLD: f32 = 15.0;

/// Recycled critters respawn this many units ahead of the car (toward -Z),
/// at a random offset within `[RESPAWN_AHEAD_MIN, RESPAWN_AHEAD_MIN + RANGE]`.
const RESPAWN_AHEAD_MIN: f32 = 30.0;
const RESPAWN_AHEAD_RANGE: f32 = 20.0;

/// Initial scatter radius around the car (fresh round). Inner radius keeps the
/// first critter from spawning on top of the car (which would be an instant
/// penalty on round start).
const SCATTER_RADIUS: f32 = 40.0;
const SCATTER_INNER: f32 = 8.0;

/// X spread for scattered / respawned critters (keeps them in the drivable
/// corridor; the car's X clamp is ±24).
const X_SPREAD: f32 = 22.0;

/// Health lost per critter hit.
const HIT_HEALTH_PENALTY: f32 = 25.0;
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
#[derive(Component, Clone, Copy, Debug)]
pub enum CritterKind {
    Pedestrian,
    Cow,
    Moose,
}

/// The bob-animated body group of a critter (parent of the body mesh, head,
/// spots, antlers, eyes). `base_y` is the resting local Y offset;
/// `wander_critters` offsets it by `bob.sin() * amplitude` each frame for the
/// waddle. The legs are siblings of this group (children of the critter root)
/// so they stay grounded while the body bobs.
#[derive(Component)]
struct CritterBody {
    base_y: f32,
}

/// A red gib particle (small sphere) ejected on critter hit. Affected by
/// gravity + spin; despawns when `life` reaches 0.
#[derive(Component)]
struct Gib {
    vel: Vec3,
    life: f32,
    max_life: f32,
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

// ---------------------------------------------------------------------------
// Asset resource (FromWorld — meshes + materials + audio built via scope)
// ---------------------------------------------------------------------------

/// Pre-built mesh + material handles for the three critter models, the hit
/// particle burst, and the bad-thud SFX. Built once via `FromWorld` so the
/// handles exist before any `OnEnter(Playing)` / `Update` system spawns a
/// critter or plays the hit sound.
#[derive(Resource)]
pub struct CritterAssets {
    // --- Audio (bad-thud on penalty hit) ---
    thud: Handle<AudioSource>,
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
    shadow_mesh: Handle<Mesh>,
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
    shadow_mat: Handle<StandardMaterial>,
    gib_mat: Handle<StandardMaterial>,
    puff_mat: Handle<StandardMaterial>,
}

impl FromWorld for CritterAssets {
    fn from_world(world: &mut World) -> Self {
        // Load the hit SFX eagerly so the handle exists before `hit_critters`
        // tries to play it.
        let thud = world.resource::<AssetServer>().load("audio/hit.wav");

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
            // spawn. Eyes: tiny black spheres. Shadow: flat quad.
            let leg_mesh = meshes.add(Cylinder::new(0.06, 0.40));
            let eye_mesh = meshes.add(Sphere::new(0.035).mesh().uv(6, 4));
            let shadow_mesh = meshes.add(Plane3d::default().mesh().size(1.0, 1.0));

            // --- Particle meshes ---
            let gib_mesh = meshes.add(Sphere::new(0.09).mesh().uv(6, 4));
            let puff_mesh = meshes.add(Plane3d::default().mesh().size(0.3, 0.3));

            // --- Materials: pedestrian (neutral clothing + skin) ---
            let ped_body_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.30, 0.40, 0.70), // blue shirt
                perceptual_roughness: 0.85,
                ..default()
            });
            let ped_head_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.85, 0.70, 0.55), // skin
                perceptual_roughness: 0.8,
                ..default()
            });
            let ped_leg_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.20, 0.20, 0.25), // dark pants
                perceptual_roughness: 0.85,
                ..default()
            });

            // --- Materials: cow (white body + black spots + pinkish head) ---
            let cow_body_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.92, 0.92, 0.90),
                perceptual_roughness: 0.85,
                ..default()
            });
            let cow_spot_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.05, 0.05, 0.05),
                perceptual_roughness: 0.8,
                ..default()
            });
            let cow_head_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.82, 0.68, 0.62), // pinkish
                perceptual_roughness: 0.85,
                ..default()
            });

            // --- Materials: shared animal legs (dark brown, cow + moose) ---
            let animal_leg_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.15, 0.12, 0.10),
                perceptual_roughness: 0.9,
                ..default()
            });

            // --- Materials: moose (brown body + darker head + tan antlers) ---
            let moose_body_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.35, 0.25, 0.15), // brown
                perceptual_roughness: 0.9,
                ..default()
            });
            let moose_head_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.28, 0.20, 0.12), // dark brown
                perceptual_roughness: 0.9,
                ..default()
            });
            let antler_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.65, 0.55, 0.40), // tan
                perceptual_roughness: 0.7,
                ..default()
            });

            // --- Materials: shared eyes + shadow + particles ---
            let eye_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.02, 0.02, 0.02),
                perceptual_roughness: 0.3,
                metallic: 0.1,
                ..default()
            });
            let shadow_mat = materials.add(StandardMaterial {
                base_color: Color::srgba(0.0, 0.0, 0.0, 0.30),
                alpha_mode: AlphaMode::Blend,
                ..default()
            });
            // Red gib + red smoke for the penalty burst.
            let gib_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.80, 0.10, 0.08),
                perceptual_roughness: 0.6,
                ..default()
            });
            let puff_mat = materials.add(StandardMaterial {
                base_color: Color::srgba(0.70, 0.10, 0.08, 0.50),
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 1.0,
                ..default()
            });

            CritterAssets {
                thud,
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
                shadow_mesh,
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
                shadow_mat,
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
            // Fresh-round spawn: inside SpawnSet so it runs before reset_run
            // flips RoundActive (risk E11). Checks RoundActive.0 to skip on
            // resume from Paused.
            .add_systems(
                OnEnter(GameState::Playing),
                spawn_critters.in_set(SpawnSet),
            )
            // Hit detection runs before wandering (chained — they share
            // Transform access on Critter entities; ordering resolves the
            // borrow). update_particles is disjoint (Gib/BloodPuff comps) so
            // it runs concurrently.
            .add_systems(
                Update,
                (hit_critters, wander_critters)
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(
                Update,
                update_particles.run_if(in_state(GameState::Playing)),
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

/// Fresh-round spawn: scatter `CRITTER_COUNT` critters (random kinds) within
/// radius `SCATTER_RADIUS` of the car. Runs in `SpawnSet` (before `reset_run`)
/// and skips on resume from `Paused` (when `RoundActive.0` is already true).
fn spawn_critters(
    mut commands: Commands,
    assets: Res<CritterAssets>,
    cfg: Res<GameConfig>,
    car: Query<&Transform, (With<Car>, Without<Critter>)>,
    round_active: Res<RoundActive>,
    mut seed: Local<u32>,
) {
    if round_active.0 {
        return;
    }
    ensure_seeded(&mut seed, 0x2468_ACE0);
    let Ok(car_t) = car.single() else {
        return;
    };
    let car_pos = car_t.translation;

    for _ in 0..CRITTER_COUNT {
        let angle = rand(&mut seed) * TAU;
        let radius = SCATTER_INNER + rand(&mut seed) * (SCATTER_RADIUS - SCATTER_INNER);
        let pos = Vec3::new(
            (car_pos.x + angle.cos() * radius).clamp(-X_SPREAD, X_SPREAD),
            0.0,
            car_pos.z + angle.sin() * radius,
        );
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
///   └── blob shadow (flat quad at y=0.02, scaled per kind)
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
) {
    let speed = critter_speed(kind, cfg);
    let base_y = critter_body_base_y(kind);

    commands
        .spawn((
            Transform::from_translation(pos),
            Visibility::default(),
            Critter { dir, speed, timer, bob },
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

            // --- Blob shadow (flat on the ground, scaled per kind) ---
            let (sw, sl) = critter_shadow_size(kind);
            root.spawn((
                Mesh3d(assets.shadow_mesh.clone()),
                MeshMaterial3d(assets.shadow_mat.clone()),
                Transform::from_xyz(0.0, 0.02, 0.0)
                    .with_scale(Vec3::new(sw, 1.0, sl)),
            ));
        });
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
                Transform::from_xyz(x, 0.34, -0.12)
                    .with_rotation(Quat::from_rotation_x(0.5)),
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
        (Entity, &mut Critter, &mut Transform, &Children),
        Without<Car>,
    >,
    mut bodies: Query<(&mut Transform, &CritterBody), (Without<Critter>, Without<Car>)>,
    time: Res<Time>,
    mut seed: Local<u32>,
) {
    ensure_seeded(&mut seed, 0x1357_9BDF);
    let Ok(car_t) = car.single() else {
        return;
    };
    let car_pos = car_t.translation;
    let dt = time.delta_secs();

    for (e, mut critter, mut tf, children) in &mut critters {
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

        // --- Recycle critters that fell behind or drifted too far away ---
        let dist = tf.translation.distance(car_pos);
        let behind = tf.translation.z > car_pos.z + BEHIND_THRESHOLD;
        if dist > KEEP_RADIUS || behind {
            commands.entity(e).despawn();
            let new_pos = respawn_ahead_pos(car_pos, &mut seed);
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
            continue;
        }

        // --- Waddle: advance phase + animate the body group child ---
        critter.bob += dt * WADDLE_SPEED;
        let waddle = critter.bob.sin();
        for child_e in children.iter() {
            if let Ok((mut body_tf, body)) = bodies.get_mut(child_e) {
                body_tf.translation.y = body.base_y + waddle * 0.05;
                body_tf.rotation = Quat::from_rotation_z(waddle * 0.08);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hit system (PENALTY)
// ---------------------------------------------------------------------------

/// On car-to-critter contact (XZ distance < `HIT_RADIUS`): despawn the
/// critter, apply a **PENALTY** (`health.0 -= 25`, `score.chickens -= 2`),
/// write a `CritterHit` message, spawn a **red** particle burst, play a
/// bad-thud SFX (hit.wav at low volume), and respawn a critter ahead so the
/// population stays constant.
fn hit_critters(
    mut commands: Commands,
    assets: Res<CritterAssets>,
    cfg: Res<GameConfig>,
    car: Query<&Transform, (With<Car>, Without<Critter>)>,
    critters: Query<(Entity, &Transform, &CritterKind), With<Critter>>,
    mut health: ResMut<Health>,
    mut score: ResMut<Score>,
    mut critter_hits: MessageWriter<CritterHit>,
    mut seed: Local<u32>,
) {
    ensure_seeded(&mut seed, 0xACE0_2468);
    let Ok(car_t) = car.single() else {
        return;
    };
    let car_pos = car_t.translation;
    let hit_r2 = HIT_RADIUS * HIT_RADIUS;

    for (e, critter_t, &kind) in &critters {
        let dx = car_pos.x - critter_t.translation.x;
        let dz = car_pos.z - critter_t.translation.z;
        if dx * dx + dz * dz < hit_r2 {
            // --- Despawn the hit critter ---
            commands.entity(e).despawn();

            // --- PENALTY: lose health + lose chicken score ---
            health.0 = (health.0 - HIT_HEALTH_PENALTY).max(0.0);
            score.chickens = score.chickens.saturating_sub(HIT_SCORE_PENALTY);

            // --- Write the message (audio.rs / UI can react) ---
            critter_hits.write(CritterHit);

            // --- Red particle burst ---
            spawn_particle_burst(&mut commands, &assets, critter_t.translation, &mut seed);

            // --- Bad-thud SFX: hit.wav at low volume, auto-despawn sink ---
            commands.spawn((
                AudioPlayer::new(assets.thud.clone()),
                PlaybackSettings::DESPAWN.with_volume(Volume::Linear(0.5)),
            ));

            // --- Respawn a critter ahead so there's always something to avoid ---
            let new_pos = respawn_ahead_pos(car_pos, &mut seed);
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
    }
}

/// Spawn the penalty burst: ~8 red gib spheres (gravity + spin) + a few red
/// smoke puffs (expand + drag). All despawn within ~0.5s via
/// `update_particles`.
fn spawn_particle_burst(
    commands: &mut Commands,
    assets: &CritterAssets,
    pos: Vec3,
    seed: &mut u32,
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
        commands.spawn((
            Mesh3d(assets.gib_mesh.clone()),
            MeshMaterial3d(assets.gib_mat.clone()),
            Transform::from_translation(body_pos),
            Gib {
                vel,
                life: GIB_LIFE,
                max_life: GIB_LIFE,
                spin: (rand(seed) * 2.0 - 1.0) * 10.0,
            },
        ));
    }

    for _ in 0..PUFF_COUNT {
        let angle = rand(seed) * TAU;
        let horiz_speed = 0.5 + rand(seed) * 1.0;
        let vel = Vec3::new(
            angle.cos() * horiz_speed,
            0.5 + rand(seed) * 0.5,
            angle.sin() * horiz_speed,
        );
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

// ---------------------------------------------------------------------------
// Particle update
// ---------------------------------------------------------------------------

/// Advance gib + puff particles: gravity, motion, spin, expansion, and
/// despawn when `life` reaches 0. Runs only during `Playing`.
fn update_particles(
    mut commands: Commands,
    time: Res<Time>,
    mut gibs: Query<(Entity, &mut Transform, &mut Gib)>,
    mut puffs: Query<(Entity, &mut Transform, &mut BloodPuff), Without<Gib>>,
) {
    let dt = time.delta_secs();
    let t = time.elapsed_secs();

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
fn cleanup_critters(mut commands: Commands, critters: Query<Entity, With<Critter>>) {
    for e in &critters {
        commands.entity(e).despawn();
    }
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

/// A position ahead of the car (toward -Z, the driving direction) at a random
/// distance within `[RESPAWN_AHEAD_MIN, RESPAWN_AHEAD_MIN + RANGE]`, with a
/// random X offset inside the drivable corridor.
fn respawn_ahead_pos(car_pos: Vec3, seed: &mut u32) -> Vec3 {
    let ahead = RESPAWN_AHEAD_MIN + rand(seed) * RESPAWN_AHEAD_RANGE;
    let x = (rand(seed) * 2.0 - 1.0) * X_SPREAD;
    Vec3::new(x, 0.0, car_pos.z - ahead)
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
        CritterKind::Cow => cfg.max_speed * 0.08,       // ~0.96 u/s
        CritterKind::Moose => cfg.max_speed * 0.12,     // ~1.44 u/s
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

/// Shadow quad (width, length) per kind — bigger animals cast bigger shadows.
fn critter_shadow_size(kind: CritterKind) -> (f32, f32) {
    match kind {
        CritterKind::Pedestrian => (0.40, 0.40),
        CritterKind::Cow => (0.80, 1.20),
        CritterKind::Moose => (0.70, 1.30),
    }
}
