//! Rich wandering chickens + particle burst on hit.
//!
//! This module is the sole owner of all chicken logic (Wave 1 deleted the old
//! flat-chicken code from `world.rs`). It provides:
//!
//! - `Chicken { dir, timer, bob }` — a wandering chicken with a rich
//!   parent/child mesh hierarchy (ellipsoid body, sphere head, red comb,
//!   orange beak, cylinder legs, black sphere eyes) built from procedural
//!   primitive meshes.
//! - `ChickenAssets` — a `FromWorld` resource holding all mesh + material
//!   handles for the chicken model and the hit particle burst (built together
//!   via `resource_scope`, per risk E3).
//! - `ChickensPlugin` — wires the spawn / wander / hit / particle / cleanup
//!   systems. Fresh-round spawn runs inside `crate::game::SpawnSet` and uses a
//!   cleanup-driven latch so it is compatible with either reset ordering.
//!
//! Contracts honoured:
//! - `ChickenHit` is already registered as a message in `game/mod.rs`; this
//!   module only **writes** it via `MessageWriter` (never re-registers).
//! - `spawn_chickens` runs `.in_set(SpawnSet)` and consumes a cleanup-driven
//!   fresh-round latch, so pause/resume skips spawning regardless of reset order.
//! - Shadows are gated by the directional light in `world.rs`; chicken
//!   `StandardMaterial`s need no shadow config.

use bevy::prelude::*;
use std::f32::consts::TAU;

use crate::car::Car;
use crate::game::SpawnSet;
use crate::game::events::ChickenHit;
use crate::game::resources::{GameConfig, Score};
use crate::game::state::GameState;
use crate::modifiers::ActiveModifier;

// ---------------------------------------------------------------------------
// Tuning constants
// ---------------------------------------------------------------------------

/// Initial chickens scattered around the car at the start of a fresh round.
const CHICKEN_COUNT: usize = 14;

/// Chicken wander speed as a fraction of `GameConfig::max_speed` (chickens are
/// much slower than the car). With the default max_speed 12.0 → 2.4 u/s.
const CHICKEN_SPEED_RATIO: f32 = 0.2;

/// Waddle phase advance rate (radians / second). `bob.sin()` produces the
/// oscillating vertical + sway offset.
const WADDLE_SPEED: f32 = 8.0;

/// Car-to-chicken hit distance (XZ plane). ~1.0 matches the car's ~0.9 radius.
const HIT_RADIUS: f32 = 1.0;

/// Chickens farther than this from the car are recycled (despawned + respawned
/// ahead) so the flock stays near the endless driver.
const KEEP_RADIUS: f32 = 50.0;

/// Chickens this far behind the car along its current heading are recycled
/// even if they remain within `KEEP_RADIUS`.
const BEHIND_THRESHOLD: f32 = 15.0;

/// Recycled chickens respawn this many units along the car's current forward
/// axis, at a random offset within `[RESPAWN_AHEAD_MIN, ... + RANGE]`.
const RESPAWN_AHEAD_MIN: f32 = 30.0;
const RESPAWN_AHEAD_RANGE: f32 = 20.0;

/// Initial scatter radius around the car (fresh round). Inner radius keeps the
/// first chicken from spawning on top of the car.
const SCATTER_RADIUS: f32 = 40.0;
const SCATTER_INNER: f32 = 5.0;

/// Maximum lateral spread from the car's current position and heading for
/// scattered / respawned chickens.
const LATERAL_SPREAD: f32 = 22.0;

/// Particle burst tuning (web-friendly: small, capped by natural despawn).
const FEATHER_COUNT: usize = 8;
const PUFF_COUNT: usize = 4;
const FEATHER_LIFE: f32 = 0.5;
const PUFF_LIFE: f32 = 0.4;
const FEATHER_GRAVITY: f32 = 6.0;

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// A wandering chicken.
///
/// - `dir`  — current heading as a unit vector in the XZ plane.
/// - `timer`— seconds until the next random direction pick.
/// - `bob`  — advancing waddle phase; `bob.sin()` drives the body child's
///            vertical bob + z-rotation sway.
///
/// The entity also carries a `Transform` (world position + Y-rotation to face
/// `dir`) and a `Children`-based mesh hierarchy (see `spawn_one_rich_chicken`).
#[derive(Component)]
pub struct Chicken {
    pub dir: Vec3,
    pub timer: f32,
    pub bob: f32,
}

/// Set after round cleanup and consumed by the next fresh-round spawn. This
/// remains reliable whether `reset_run` executes before or after `SpawnSet`,
/// while pause/resume leaves it false.
#[derive(Resource)]
struct ChickenSpawnPending(bool);

impl Default for ChickenSpawnPending {
    fn default() -> Self {
        Self(true)
    }
}

/// The bob-animated body group of a chicken (parent of the body mesh, head,
/// comb, beak, eyes). `base_y` is the resting local Y offset; `wander_chickens`
/// offsets it by `bob.sin() * 0.05` each frame for the waddle. The legs are
/// siblings of this group (children of the chicken root) so they stay grounded
/// while the body bobs.
#[derive(Component)]
struct ChickenBody {
    base_y: f32,
}

/// A feather particle (small sphere) ejected on chicken hit. Affected by
/// gravity + spin; despawns when `life` reaches 0.
#[derive(Component)]
struct Feather {
    vel: Vec3,
    life: f32,
    spin: f32,
}

/// A puff particle (flat expanding quad) for the dust burst on chicken hit.
/// Expands + decelerates; despawns when `life` reaches 0.
#[derive(Component)]
struct Puff {
    vel: Vec3,
    life: f32,
    max_life: f32,
}

// ---------------------------------------------------------------------------
// Asset resource (FromWorld — meshes + materials built together via scope)
// ---------------------------------------------------------------------------

/// Pre-built mesh + material handles for the rich chicken model and the hit
/// particle burst. Built once via `FromWorld` so the handles exist before any
/// `OnEnter(Playing)` / `Update` system tries to spawn a chicken.
#[derive(Resource)]
pub struct ChickenAssets {
    // Chicken parts
    body_mesh: Handle<Mesh>,
    head_mesh: Handle<Mesh>,
    comb_mesh: Handle<Mesh>,
    beak_mesh: Handle<Mesh>,
    leg_mesh: Handle<Mesh>,
    eye_mesh: Handle<Mesh>,
    shadow_mesh: Handle<Mesh>,
    // Particle burst
    feather_mesh: Handle<Mesh>,
    puff_mesh: Handle<Mesh>,
    // Materials (head reuses body_mat; legs share leg_mat)
    body_mat: Handle<StandardMaterial>,
    comb_mat: Handle<StandardMaterial>,
    beak_mat: Handle<StandardMaterial>,
    leg_mat: Handle<StandardMaterial>,
    eye_mat: Handle<StandardMaterial>,
    shadow_mat: Handle<StandardMaterial>,
    feather_mat: Handle<StandardMaterial>,
    puff_mat: Handle<StandardMaterial>,
}

impl FromWorld for ChickenAssets {
    fn from_world(world: &mut World) -> Self {
        // Build meshes + materials together inside a `resource_scope` so we
        // never hold `&mut Assets<Mesh>` and `&mut Assets<StandardMaterial>`
        // without scoping (risk E3 — mirrors `textures.rs::TextureAssets`).
        world.resource_scope::<Assets<Mesh>, _>(|world, mut meshes| {
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();

            // --- Chicken part meshes (primitives) ---
            // Body: unit sphere, scaled to an ellipsoid at spawn via Transform.
            let body_mesh = meshes.add(Sphere::new(1.0).mesh().uv(12, 8));
            let head_mesh = meshes.add(Sphere::new(0.16).mesh().uv(10, 6));
            let comb_mesh = meshes.add(Cuboid::new(0.06, 0.08, 0.06));
            let beak_mesh = meshes.add(Cuboid::new(0.10, 0.05, 0.06));
            let leg_mesh = meshes.add(Cylinder::new(0.03, 0.30));
            let eye_mesh = meshes.add(Sphere::new(0.035).mesh().uv(6, 4));
            // Blob shadow: flat dark quad under the chicken.
            let shadow_mesh = meshes.add(Plane3d::default().mesh().size(0.5, 0.5));

            // --- Particle meshes ---
            let feather_mesh = meshes.add(Sphere::new(0.08).mesh().uv(6, 4));
            let puff_mesh = meshes.add(Plane3d::default().mesh().size(0.3, 0.3));

            // --- Materials ---
            let body_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.95, 0.93, 0.85), // cream-white
                perceptual_roughness: 0.85,
                ..default()
            });
            let comb_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.85, 0.15, 0.12), // red
                perceptual_roughness: 0.7,
                ..default()
            });
            let beak_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.95, 0.55, 0.10), // orange
                perceptual_roughness: 0.6,
                ..default()
            });
            let leg_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.90, 0.65, 0.15), // yellow-orange
                perceptual_roughness: 0.6,
                ..default()
            });
            let eye_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.02, 0.02, 0.02), // near-black
                perceptual_roughness: 0.3,
                metallic: 0.1,
                ..default()
            });
            let shadow_mat = materials.add(StandardMaterial {
                base_color: Color::srgba(0.0, 0.0, 0.0, 0.30),
                alpha_mode: AlphaMode::Blend,
                ..default()
            });
            let feather_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.95, 0.93, 0.85),
                perceptual_roughness: 0.85,
                ..default()
            });
            let puff_mat = materials.add(StandardMaterial {
                base_color: Color::srgba(0.90, 0.90, 0.90, 0.50),
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 1.0,
                ..default()
            });

            ChickenAssets {
                body_mesh,
                head_mesh,
                comb_mesh,
                beak_mesh,
                leg_mesh,
                eye_mesh,
                shadow_mesh,
                feather_mesh,
                puff_mesh,
                body_mat,
                comb_mat,
                beak_mat,
                leg_mat,
                eye_mat,
                shadow_mat,
                feather_mat,
                puff_mat,
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct ChickensPlugin;

impl Plugin for ChickensPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChickenAssets>()
            .init_resource::<ChickenSpawnPending>()
            // Fresh-round spawn: inside SpawnSet and guarded by a cleanup-
            // driven latch, so reset ordering and pause/resume are both safe.
            .add_systems(OnEnter(GameState::Playing), spawn_chickens.in_set(SpawnSet))
            // Hit detection runs before wandering (chained — they share
            // Transform access on Chicken entities; ordering resolves the
            // borrow). update_particles is disjoint (Feather/Puff components)
            // so it runs concurrently.
            .add_systems(
                Update,
                (hit_chickens, wander_chickens)
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(
                Update,
                update_particles.run_if(in_state(GameState::Playing)),
            )
            // Recursive despawn of all chickens + particles on round end.
            .add_systems(
                OnEnter(GameState::GameOver),
                (cleanup_chickens, cleanup_particles),
            )
            .add_systems(
                OnEnter(GameState::Menu),
                (cleanup_chickens, cleanup_particles),
            );
    }
}

// ---------------------------------------------------------------------------
// Spawn systems
// ---------------------------------------------------------------------------

/// Fresh-round spawn: scatter the modifier-adjusted chicken target within
/// radius `SCATTER_RADIUS` of the car. Runs in `SpawnSet` and consumes the
/// cleanup-driven fresh-round latch, so it is independent of reset ordering.
fn spawn_chickens(
    mut commands: Commands,
    assets: Res<ChickenAssets>,
    modifier: Res<ActiveModifier>,
    car: Query<&Transform, (With<Car>, Without<Chicken>)>,
    mut spawn_pending: ResMut<ChickenSpawnPending>,
    mut seed: Local<u32>,
) {
    if !spawn_pending.0 {
        return;
    }
    ensure_seeded(&mut seed, 0x1234_5678);
    let Ok(car_t) = car.single() else {
        return;
    };
    spawn_pending.0 = false;
    let car_pos = car_t.translation;
    let forward = horizontal_forward(car_t.rotation);

    for _ in 0..effective_chicken_target(&modifier) {
        let angle = rand(&mut seed) * TAU;
        let radius = SCATTER_INNER + rand(&mut seed) * (SCATTER_RADIUS - SCATTER_INNER);
        let lateral = (angle.cos() * radius).clamp(-LATERAL_SPREAD, LATERAL_SPREAD);
        let longitudinal = angle.sin() * radius;
        let pos = car_relative_ground_pos(car_pos, forward, longitudinal, lateral);
        spawn_one_rich_chicken(
            &mut commands,
            &assets,
            pos,
            rand_dir_xz(&mut seed),
            1.5 + rand(&mut seed) * 2.0,
            rand(&mut seed) * TAU,
        );
    }
}

/// Build one rich chicken as a parent + children hierarchy.
///
/// Hierarchy:
/// ```text
/// chicken_root (Transform: world pos + heading, Chicken, Visibility)
///   ├── ChickenBody { base_y } (bob group — animated each frame)
///   │     ├── body mesh (unit Sphere scaled to ellipsoid)
///   │     └── head (Sphere)
///   │           ├── comb × 3 (small red Cuboids)
///   │           ├── beak (orange Cuboid, front = -Z)
///   │           └── eye × 2 (black Spheres)
///   ├── leg × 2 (Cylinders, children of root — stay grounded)
///   └── blob shadow (flat quad at y=0.02)
/// ```
///
/// The body group (`ChickenBody`) is animated by `wander_chickens`: its
/// `translation.y` bobs and its `rotation.z` sways with the waddle phase.
/// Because the body group has no scale, the head (its child) is positioned in
/// unscaled local units — the ellipsoid scale lives on the body **mesh** child
/// only, so the head isn't squished.
fn spawn_one_rich_chicken(
    commands: &mut Commands,
    assets: &ChickenAssets,
    pos: Vec3,
    dir: Vec3,
    timer: f32,
    bob: f32,
) {
    commands
        .spawn((
            Transform::from_translation(pos),
            Visibility::default(),
            Chicken { dir, timer, bob },
        ))
        .with_children(|root| {
            // --- Bob-animated body group (head + body mesh nest under it) ---
            root.spawn((
                Transform::from_xyz(0.0, 0.35, 0.0),
                Visibility::default(),
                ChickenBody { base_y: 0.35 },
            ))
            .with_children(|body| {
                // Body ellipsoid: unit sphere scaled to a chicken shape.
                body.spawn((
                    Mesh3d(assets.body_mesh.clone()),
                    MeshMaterial3d(assets.body_mat.clone()),
                    Transform::from_scale(Vec3::new(0.28, 0.22, 0.32)),
                ));
                // Head (reuses body material).
                body.spawn((
                    Mesh3d(assets.head_mesh.clone()),
                    MeshMaterial3d(assets.body_mat.clone()),
                    Transform::from_xyz(0.0, 0.28, 0.0),
                ))
                .with_children(|head| {
                    // Red comb: 3 small cuboids on top of the head.
                    for &(x, y) in &[(0.0_f32, 0.18_f32), (-0.07, 0.15), (0.07, 0.15)] {
                        head.spawn((
                            Mesh3d(assets.comb_mesh.clone()),
                            MeshMaterial3d(assets.comb_mat.clone()),
                            Transform::from_xyz(x, y, 0.0),
                        ));
                    }
                    // Orange beak (front = -Z, the chicken's facing direction).
                    head.spawn((
                        Mesh3d(assets.beak_mesh.clone()),
                        MeshMaterial3d(assets.beak_mat.clone()),
                        Transform::from_xyz(0.0, 0.0, -0.18),
                    ));
                    // Black eyes on the sides of the head.
                    for &x in &[-0.1_f32, 0.1] {
                        head.spawn((
                            Mesh3d(assets.eye_mesh.clone()),
                            MeshMaterial3d(assets.eye_mat.clone()),
                            Transform::from_xyz(x, 0.04, -0.08),
                        ));
                    }
                });
            });

            // --- Legs (children of root, not the bob group — stay grounded) ---
            for &x in &[-0.1_f32, 0.1] {
                root.spawn((
                    Mesh3d(assets.leg_mesh.clone()),
                    MeshMaterial3d(assets.leg_mat.clone()),
                    Transform::from_xyz(x, 0.15, 0.0),
                ));
            }

            // --- Blob shadow (flat on the ground under the chicken) ---
            root.spawn((
                Mesh3d(assets.shadow_mesh.clone()),
                MeshMaterial3d(assets.shadow_mat.clone()),
                Transform::from_xyz(0.0, 0.02, 0.0),
            ));
        });
}

// ---------------------------------------------------------------------------
// Wander system
// ---------------------------------------------------------------------------

/// Move chickens by `dir`, periodically pick a new random heading, face it,
/// recycle chickens that fall behind / drift beyond `KEEP_RADIUS`, and animate
/// the waddle bob on the body child.
fn wander_chickens(
    mut commands: Commands,
    assets: Res<ChickenAssets>,
    cfg: Res<GameConfig>,
    car: Query<&Transform, (With<Car>, Without<Chicken>)>,
    mut chickens: Query<(Entity, &mut Chicken, &mut Transform, &Children)>,
    mut bodies: Query<(&mut Transform, &ChickenBody), (Without<Chicken>, Without<Car>)>,
    time: Res<Time>,
    mut seed: Local<u32>,
) {
    ensure_seeded(&mut seed, 0x9ABC_DEF0);
    let Ok(car_t) = car.single() else {
        return;
    };
    let car_pos = car_t.translation;
    let car_forward = horizontal_forward(car_t.rotation);
    let dt = time.delta_secs();
    let speed = cfg.max_speed * CHICKEN_SPEED_RATIO;

    for (e, mut chicken, mut tf, children) in &mut chickens {
        // --- Periodically pick a new random heading ---
        chicken.timer -= dt;
        if chicken.timer <= 0.0 {
            chicken.dir = rand_dir_xz(&mut seed);
            chicken.timer = 1.5 + rand(&mut seed) * 2.0;
        }

        // --- Move (XZ plane only; y stays 0) ---
        tf.translation += chicken.dir * speed * dt;

        // --- Face the heading (rotate Y so the beak points along dir) ---
        let heading = (-chicken.dir.x).atan2(-chicken.dir.z);
        tf.rotation = Quat::from_rotation_y(heading);

        // --- Recycle chickens that fell behind or drifted too far away ---
        if should_recycle(tf.translation, car_pos, car_forward) {
            commands.entity(e).despawn();
            let new_pos = respawn_ahead_pos(car_pos, car_forward, &mut seed);
            spawn_one_rich_chicken(
                &mut commands,
                &assets,
                new_pos,
                rand_dir_xz(&mut seed),
                1.5 + rand(&mut seed) * 2.0,
                rand(&mut seed) * TAU,
            );
            continue;
        }

        // --- Waddle: advance phase + animate the body group child ---
        chicken.bob += dt * WADDLE_SPEED;
        let waddle = chicken.bob.sin();
        for child_e in children.iter() {
            if let Ok((mut body_tf, body)) = bodies.get_mut(child_e) {
                body_tf.translation.y = body.base_y + waddle * 0.05;
                body_tf.rotation = Quat::from_rotation_z(waddle * 0.08);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hit system
// ---------------------------------------------------------------------------

/// On car-to-chicken contact (XZ distance < `HIT_RADIUS`): despawn the chicken,
/// bump `Score.chickens` by its normal point plus any road-condition bonus,
/// write a `ChickenHit` message (audio.rs plays the hit SFX), spawn a feather +
/// puff particle burst, and respawn one chicken ahead of the car so the flock
/// population stays constant at its fresh-round target.
fn hit_chickens(
    mut commands: Commands,
    assets: Res<ChickenAssets>,
    modifier: Res<ActiveModifier>,
    car: Query<&Transform, (With<Car>, Without<Chicken>)>,
    chickens: Query<(Entity, &Transform), With<Chicken>>,
    mut score: ResMut<Score>,
    mut chicken_hits: MessageWriter<ChickenHit>,
    mut seed: Local<u32>,
) {
    ensure_seeded(&mut seed, 0x5678_9ABC);
    let Ok(car_t) = car.single() else {
        return;
    };
    let car_pos = car_t.translation;
    let car_forward = horizontal_forward(car_t.rotation);
    let hit_r2 = HIT_RADIUS * HIT_RADIUS;

    for (e, chicken_t) in &chickens {
        let dx = car_pos.x - chicken_t.translation.x;
        let dz = car_pos.z - chicken_t.translation.z;
        if dx * dx + dz * dz < hit_r2 {
            commands.entity(e).despawn();
            // Keep combo handling on the single ChickenHit message; only the
            // direct chicken award receives the road-condition bonus here.
            score.chickens += chicken_score_per_hit(&modifier);
            chicken_hits.write(ChickenHit);
            spawn_particle_burst(&mut commands, &assets, chicken_t.translation, &mut seed);
            // Respawn ahead so the player always has chickens to chase.
            let new_pos = respawn_ahead_pos(car_pos, car_forward, &mut seed);
            spawn_one_rich_chicken(
                &mut commands,
                &assets,
                new_pos,
                rand_dir_xz(&mut seed),
                1.5 + rand(&mut seed) * 2.0,
                rand(&mut seed) * TAU,
            );
        }
    }
}

/// Spawn the enhanced hit burst: ~8 feather spheres (gravity + spin) + a few
/// puff quads (expand + drag). All despawn within ~0.5s via `update_particles`.
fn spawn_particle_burst(
    commands: &mut Commands,
    assets: &ChickenAssets,
    pos: Vec3,
    seed: &mut u32,
) {
    let body_pos = pos + Vec3::new(0.0, 0.30, 0.0);
    let ground_pos = pos + Vec3::new(0.0, 0.10, 0.0);

    for _ in 0..FEATHER_COUNT {
        let angle = rand(seed) * TAU;
        let horiz_speed = 1.5 + rand(seed) * 2.5;
        let vel = Vec3::new(
            angle.cos() * horiz_speed,
            2.0 + rand(seed) * 2.5, // upward pop
            angle.sin() * horiz_speed,
        );
        commands.spawn((
            Mesh3d(assets.feather_mesh.clone()),
            MeshMaterial3d(assets.feather_mat.clone()),
            Transform::from_translation(body_pos),
            Feather {
                vel,
                life: FEATHER_LIFE,
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
            Puff {
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

/// Advance feather + puff particles: gravity, motion, spin, expansion, and
/// despawn when `life` reaches 0. Runs only during `Playing`.
fn update_particles(
    mut commands: Commands,
    time: Res<Time>,
    mut feathers: Query<(Entity, &mut Transform, &mut Feather)>,
    mut puffs: Query<(Entity, &mut Transform, &mut Puff), Without<Feather>>,
) {
    let dt = time.delta_secs();
    let t = time.elapsed_secs();

    for (e, mut tf, mut feather) in &mut feathers {
        feather.life -= dt;
        if feather.life <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        feather.vel.y -= FEATHER_GRAVITY * dt;
        tf.translation += feather.vel * dt;
        // Tumble for visual interest.
        tf.rotation =
            Quat::from_rotation_y(t * feather.spin) * Quat::from_rotation_x(t * feather.spin * 0.7);
        // Don't sink through the ground.
        if tf.translation.y < 0.05 {
            tf.translation.y = 0.05;
            feather.vel.y = 0.0;
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

/// Despawn every chicken (recursive — nukes the mesh hierarchy, risk E2).
fn cleanup_chickens(
    mut commands: Commands,
    chickens: Query<Entity, With<Chicken>>,
    mut spawn_pending: ResMut<ChickenSpawnPending>,
) {
    for e in &chickens {
        commands.entity(e).despawn();
    }
    spawn_pending.0 = true;
}

/// Despawn every lingering feather + puff particle.
fn cleanup_particles(
    mut commands: Commands,
    feathers: Query<Entity, With<Feather>>,
    puffs: Query<Entity, With<Puff>>,
) {
    for e in &feathers {
        commands.entity(e).despawn();
    }
    for e in &puffs {
        commands.entity(e).despawn();
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Fresh-round chicken population after applying the active road condition.
const fn effective_chicken_target(modifier: &ActiveModifier) -> usize {
    modifier.chicken_target(CHICKEN_COUNT)
}

/// Direct score awarded by one chicken hit. Combo scoring remains driven by
/// the single `ChickenHit` message and is deliberately not included here.
const fn chicken_score_per_hit(modifier: &ActiveModifier) -> u32 {
    1 + modifier.chicken_score_bonus()
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

/// Horizontal unit forward for a car transform. The fallback only matters for
/// a malformed rotation that projects entirely onto Y.
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

/// Keep/recycle decision expressed entirely in the car's current frame.
fn should_recycle(pos: Vec3, car_pos: Vec3, car_forward: Vec3) -> bool {
    pos.distance(car_pos) > KEEP_RADIUS || is_behind_car(pos, car_pos, car_forward)
}

/// A position ahead of the car at a random forward distance and a random
/// car-relative lateral offset centered on the car.
fn respawn_ahead_pos(car_pos: Vec3, car_forward: Vec3, seed: &mut u32) -> Vec3 {
    let ahead = RESPAWN_AHEAD_MIN + rand(seed) * RESPAWN_AHEAD_RANGE;
    let lateral = (rand(seed) * 2.0 - 1.0) * LATERAL_SPREAD;
    car_relative_ground_pos(car_pos, car_forward, ahead, lateral)
}

/// Seed a `Local<u32>` RNG on first use with a per-system constant so the three
/// systems' sequences don't start correlated (the LCG never produces 0 from a
/// non-zero seed, so this fires exactly once per system).
fn ensure_seeded(seed: &mut u32, initial: u32) {
    if *seed == 0 {
        *seed = initial;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modifiers::ModifierKind;

    #[test]
    fn chicken_frenzy_changes_only_the_population_target() {
        let standard = ActiveModifier(ModifierKind::Standard);
        let frenzy = ActiveModifier(ModifierKind::ChickenFrenzy);
        let stampede = ActiveModifier(ModifierKind::Stampede);

        assert_eq!(effective_chicken_target(&standard), CHICKEN_COUNT);
        assert_eq!(effective_chicken_target(&frenzy), 35);
        assert_eq!(effective_chicken_target(&stampede), CHICKEN_COUNT);
    }

    #[test]
    fn chicken_frenzy_adds_one_direct_point_per_hit() {
        let standard = ActiveModifier(ModifierKind::Standard);
        let frenzy = ActiveModifier(ModifierKind::ChickenFrenzy);
        let glass_cannon = ActiveModifier(ModifierKind::GlassCannon);

        assert_eq!(chicken_score_per_hit(&standard), 1);
        assert_eq!(chicken_score_per_hit(&frenzy), 2);
        assert_eq!(chicken_score_per_hit(&glass_cannon), 1);
    }

    #[test]
    fn ahead_placement_tracks_zero_ninety_and_one_eighty_degree_headings() {
        let origin = Vec3::new(7.0, 3.0, -4.0);
        let cases = [
            (0.0, Vec3::new(7.0, 0.0, -14.0)),
            (std::f32::consts::FRAC_PI_2, Vec3::new(-3.0, 0.0, -4.0)),
            (std::f32::consts::PI, Vec3::new(7.0, 0.0, 6.0)),
        ];

        for (yaw, expected) in cases {
            let forward = horizontal_forward(Quat::from_rotation_y(yaw));
            let actual = car_relative_ground_pos(origin, forward, 10.0, 0.0);
            assert!(
                (actual - expected).length() < 0.0001,
                "{actual:?} != {expected:?}"
            );
        }
    }

    #[test]
    fn lateral_placement_is_car_relative_and_bounded() {
        let car_pos = Vec3::new(100.0, 0.0, -80.0);
        let forward = horizontal_forward(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        let right = Vec3::new(-forward.z, 0.0, forward.x);
        let pos = car_relative_ground_pos(car_pos, forward, 12.0, LATERAL_SPREAD * 3.0);
        let relative = pos - car_pos;

        assert!((relative.dot(right) - LATERAL_SPREAD).abs() < 0.0001);
        assert!((relative.dot(forward) - 12.0).abs() < 0.0001);
    }

    #[test]
    fn behind_check_uses_the_current_heading() {
        let car_pos = Vec3::new(20.0, 0.0, 30.0);
        let east = Vec3::X;

        assert!(is_behind_car(car_pos - east * 16.0, car_pos, east));
        assert!(!is_behind_car(car_pos + east * 16.0, car_pos, east));
        assert!(!is_behind_car(car_pos + Vec3::Z * 40.0, car_pos, east));
    }
}
