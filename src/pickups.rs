//! Power-ups (T11 / T16): periodic glowing orbs the car drives through to
//! activate a temporary or instant effect.
//!
//! Five kinds:
//! - **SpeedBoost** (blue): nudges `car.speed` upward for ~4s (cap ~1.6× max).
//! - **CoinMagnet** (gold): pulls a capped set of nearby coins toward the car
//!   for ~4s so the existing `world.rs::collect_coins` (distance < 1.2)
//!   scoops them up.
//! - **Health** (green): instant — restores `Health` by +35 (cap 100).
//! - **Time** (cyan clock): instant — adds +5s to `TimeLeft` (cap 99).
//! - **MegaCoin** (big gold): instant — +5 coins to `Score` + writes a
//!   `CoinCollected` message (-> `audio.rs` plays the coin chime +
//!   `combos.rs` applies the multiplier).
//!
//! Design notes:
//! - Power-up orbs are **top-level entities** (NOT chunk-root children), so
//!   their `Transform` is world-space — no `GlobalTransform` needed for
//!   pickup distance checks. Coins, by contrast, are chunk-root children
//!   whose `Transform` is LOCAL; the magnet uses `GlobalTransform` to read a
//!   coin's world position and applies the same world delta to the coin's
//!   local `Transform` (valid because chunk roots have translation only —
//!   no rotation/scale — so local and world translation deltas coincide).
//! - All gameplay effects are driven from resources + queries **owned here**;
//!   `car.rs` / `world.rs` / `health.rs` are never edited. `Health`,
//!   `TimeLeft`, `Score` are read via `ResMut`; `CoinCollected` is written
//!   via `MessageWriter` (already registered in `game/mod.rs`).
//! - Counts are capped (≤ 4 active orbs) and spawn timers bind the rate
//!   (~8–12s) for web friendliness.
//! - The orb mesh is a glowing emissive UV-sphere on a tiny pedestal; bloom
//!   is on, so `emissive: LinearRgba::rgb(...)` makes them pop. MegaCoin
//!   uses a larger orb mesh so it reads as a "big gold" prize.
//! - Instant pickups (Health/Time/MegaCoin) have no timer bar — they fire
//!   their effect + despawn immediately, with a brief colored screen flash
//!   for feedback. Timed pickups (SpeedBoost/CoinMagnet) get UI bars.
//! - Owns its UI (active-power-up icons + remaining-time bars, bottom-center
//!   above the health bar); does not touch `ui.rs`.

use bevy::audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume};
use bevy::color::LinearRgba;
use bevy::prelude::*;
use bevy::text::FontSize;

use crate::audio::AudioBaseGain;
use crate::car::Car;
use crate::game::SpawnSet;
use crate::game::events::CoinCollected;
use crate::game::resources::{GameConfig, Score, TimeLeft};
use crate::game::state::GameState;
use crate::health::Health;
use crate::world::Coin;

// ===========================================================================
// Tuning constants
// ===========================================================================

/// Radius (world units) around the car in which a power-up can spawn.
const SPAWN_RADIUS: f32 = 25.0;
/// Forward bias: power-ups spawn ahead along the car's current heading, in a
/// cone spanning ±`SPAWN_HALF_CONE` radians around the forward axis.
const SPAWN_HALF_CONE: f32 = 0.7; // ~40°
/// Reachable lateral half-width measured from the car, not world X.
const SPAWN_LATERAL_RANGE: f32 = 22.0;
/// Min/max seconds between power-up spawns (re-rolled each spawn).
const SPAWN_INTERVAL_MIN: f32 = 8.0;
const SPAWN_INTERVAL_MAX: f32 = 12.0;
/// Max simultaneous power-up orbs kept alive (web-friendly cap). Bumped to 4
/// for T16's wider kind variety.
const MAX_ACTIVE_PICKUPS: usize = 4;
/// Distance (world units) at which the car collects a power-up.
const PICKUP_RADIUS: f32 = 1.2;

/// SpeedBoost duration (seconds).
const SPEED_BOOST_DURATION: f32 = 4.0;
/// SpeedBoost acceleration added to `car.speed` each second while active.
const SPEED_BOOST_ACCEL: f32 = 20.0;
/// SpeedBoost cap multiplier over `GameConfig::max_speed`.
const SPEED_BOOST_CAP_MULT: f32 = 1.6;

/// CoinMagnet duration (seconds).
const MAGNET_DURATION: f32 = 4.0;
/// Only coins within this world-space XZ radius are eligible for attraction.
const MAGNET_RADIUS: f32 = 10.0;
/// Fraction of the distance to the car a coin closes per second (0..1; higher
/// = snappier pull). Applied as `pos += (car - pos) * STRENGTH * dt`.
const MAGNET_STRENGTH: f32 = 3.0;
/// Hard per-frame cap on coins moved by the magnet. Selection keeps only the
/// nearest candidates and uses entity order to break equal-distance ties.
const MAX_MAGNET_COINS_PER_FRAME: usize = 24;

/// Health pickup: amount restored (clamped to `HEALTH_MAX`).
const HEALTH_RESTORE: f32 = 35.0;
/// Health cap (matches `health.rs::HEALTH_MAX`).
const HEALTH_MAX: f32 = 100.0;

/// TimeBonus pickup: seconds added to `TimeLeft`.
const TIME_BONUS: f32 = 5.0;
/// TimeBonus cap — `TimeLeft` is clamped to this so a pile of clocks can't
/// inflate the round indefinitely (matches the ~99s ceiling).
const TIME_CAP: f32 = 99.0;

/// MegaCoin pickup: coins added to `Score`.
const MEGA_COIN_AMOUNT: u32 = 5;

/// Orb mesh radius (icosahedron circumscribed sphere radius).
const ORB_RADIUS: f32 = 0.45;
/// MegaCoin orb radius (bigger so it reads as a prize).
const MEGA_ORB_RADIUS: f32 = 0.72;
/// Pedestal height (the orb floats ~this far above the ground).
const ORB_HOVER_Y: f32 = 1.0;

/// UI: width of a power-up timer bar track (px).
const UI_BAR_W: f32 = 110.0;
/// UI: height of a power-up timer bar fill (px).
const UI_BAR_H: f32 = 8.0;
/// UI: bottom offset — sits above the health bar (which is at `bottom: 12` +
/// its panel padding ~ 8 + label + bar + text ≈ 70px tall, so 84 clears it).
const UI_BOTTOM: f32 = 84.0;

/// Instant-pickup flash: full-screen tint lifetime (seconds).
const PICKUP_FLASH_DURATION: f32 = 0.3;
/// Instant-pickup flash: peak alpha (fades to 0 over the duration).
const PICKUP_FLASH_PEAK_ALPHA: f32 = 0.22;

// ===========================================================================
// Resources
// ===========================================================================

/// Remaining seconds of active SpeedBoost (0 = inactive). Owned here.
#[derive(Resource, Default)]
pub struct SpeedBoostTimer(pub f32);

/// Remaining seconds of active CoinMagnet (0 = inactive). Owned here.
#[derive(Resource, Default)]
pub struct MagnetTimer(pub f32);

/// Cleanup-driven fresh-round latch, independent of `reset_run` ordering.
#[derive(Resource)]
struct PickupResetPending(bool);

impl Default for PickupResetPending {
    fn default() -> Self {
        Self(true)
    }
}

/// Shared mesh + per-kind emissive material handles for power-up orbs. Built
/// once via `FromWorld` (resource-scoping `Assets<Mesh>` then
/// `Assets<StandardMaterial>` — mirrors `textures.rs::TextureAssets` and
/// `effects.rs::EffectsAssets`).
#[derive(Resource)]
pub struct PickupAssets {
    /// Icosahedron mesh shared by the standard orb kinds.
    orb_mesh: Handle<Mesh>,
    /// Larger orb mesh used by MegaCoin (reads as a "big gold" prize).
    mega_orb_mesh: Handle<Mesh>,
    /// Tiny dark cylinder pedestal mesh.
    pedestal_mesh: Handle<Mesh>,
    /// SpeedBoost orb material (glowing blue).
    boost_mat: Handle<StandardMaterial>,
    /// CoinMagnet orb material (glowing gold/orange).
    magnet_mat: Handle<StandardMaterial>,
    /// Health orb material (glowing green).
    health_mat: Handle<StandardMaterial>,
    /// TimeBonus orb material (glowing cyan — distinct from SpeedBoost blue).
    time_mat: Handle<StandardMaterial>,
    /// MegaCoin orb material (glowing bright gold — distinct from magnet).
    megacoin_mat: Handle<StandardMaterial>,
    /// Pedestal material (dark, unlit).
    pedestal_mat: Handle<StandardMaterial>,
}

impl FromWorld for PickupAssets {
    fn from_world(world: &mut World) -> Self {
        // Scope meshes first (like textures.rs scopes Images), then grab
        // materials inside the closure so we never hold two `&mut Assets<…>`
        // at once without scoping (risk E3).
        world.resource_scope::<Assets<Mesh>, _>(|world, mut meshes| {
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();

            // Icosphere-ish UV-sphere for the orb — a smooth faceted
            // ball that catches the emissive glow nicely.
            let orb_mesh = meshes.add(Sphere::new(ORB_RADIUS).mesh().uv(12, 8));
            // Larger orb for MegaCoin.
            let mega_orb_mesh = meshes.add(Sphere::new(MEGA_ORB_RADIUS).mesh().uv(12, 8));
            // Tiny pedestal cylinder under the orb.
            let pedestal_mesh = meshes.add(Cylinder::new(0.22, 0.25));

            // SpeedBoost: bright glowing blue. `LinearRgba` emissive so it
            // pops with bloom (T9 rendering is HDR + tonemapping + bloom).
            let boost_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.25, 0.45, 1.0),
                emissive: LinearRgba::rgb(0.4, 0.8, 2.2),
                ..default()
            });

            // CoinMagnet: glowing gold/orange.
            let magnet_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(1.0, 0.7, 0.15),
                emissive: LinearRgba::rgb(2.2, 1.4, 0.25),
                ..default()
            });

            // Health: glowing green cross orb.
            let health_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.2, 0.9, 0.3),
                emissive: LinearRgba::rgb(0.3, 2.0, 0.45),
                ..default()
            });

            // TimeBonus: glowing cyan clock orb (distinct from SpeedBoost's
            // deeper blue — brighter + greener hue reads as "time").
            let time_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.2, 0.72, 0.95),
                emissive: LinearRgba::rgb(0.3, 1.7, 2.1),
                ..default()
            });

            // MegaCoin: bright glowing gold — richer than CoinMagnet so the
            // bigger orb reads as a premium prize.
            let megacoin_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(1.0, 0.85, 0.2),
                emissive: LinearRgba::rgb(2.6, 2.0, 0.4),
                ..default()
            });

            // Pedestal: dark, unlit (reads as a little stand).
            let pedestal_mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.12, 0.12, 0.14),
                unlit: true,
                ..default()
            });

            PickupAssets {
                orb_mesh,
                mega_orb_mesh,
                pedestal_mesh,
                boost_mat,
                magnet_mat,
                health_mat,
                time_mat,
                megacoin_mat,
                pedestal_mat,
            }
        })
    }
}

/// Preloaded pickup SFX handle (reuses the existing coin.wav — a bright chime
/// fits a power-up grab). Built eagerly via `FromWorld` like `audio.rs`.
#[derive(Resource)]
struct PickupAudio {
    sfx: Handle<AudioSource>,
}

impl FromWorld for PickupAudio {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();
        PickupAudio {
            sfx: asset_server.load("audio/coin.wav"),
        }
    }
}

// ===========================================================================
// Components
// ===========================================================================

/// Which power-up an orb grants when collected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerKind {
    SpeedBoost,
    CoinMagnet,
    Health,
    Time,
    MegaCoin,
}

/// Tag + kind for a power-up orb entity (top-level, world Transform).
#[derive(Component)]
pub struct PowerUp {
    kind: PowerKind,
}

/// UI root for the active-power-up panel (bottom-center, above the health
/// bar). Despawned on exit from `Playing` and respawned on (re)enter.
#[derive(Component)]
struct PowerUpUiRoot;

/// Marker for the SpeedBoost timer-bar fill (width refreshed each frame
/// while active).
#[derive(Component)]
struct BoostBarFill;

/// Marker for the CoinMagnet timer-bar fill.
#[derive(Component)]
struct MagnetBarFill;

/// Marker for a whole power-up UI row (icon + bar) carrying which effect it
/// represents, so `update_powerup_ui` can show/hide + drive the right bar.
#[derive(Component, Clone, Copy)]
struct PowerUpRow {
    kind: PowerKind,
}

/// Full-screen colored tint spawned when an instant pickup (Health / Time /
/// MegaCoin) is collected; fades out and despawns. Provides a quick "I got
/// something" flash since those kinds have no timer bar.
#[derive(Component)]
struct PickupFlash {
    /// Seconds remaining in the flash.
    t: f32,
    /// Base RGB (0..1) of the tint — rebuilt with a fading alpha each frame.
    r: f32,
    g: f32,
    b: f32,
}

// ===========================================================================
// Plugin
// ===========================================================================

pub struct PickupsPlugin;

impl Plugin for PickupsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PickupAssets>()
            .init_resource::<PickupAudio>()
            .init_resource::<SpeedBoostTimer>()
            .init_resource::<MagnetTimer>()
            .init_resource::<PickupResetPending>()
            // Fresh-round reset uses a cleanup-driven latch, so it remains
            // correct whether `reset_run` runs before or after `SpawnSet`.
            .add_systems(OnEnter(GameState::Playing), reset_pickups.in_set(SpawnSet))
            // UI lifecycle tied to the Playing state (despawned on exit so a
            // pause/resume cycle respawns it cleanly, like the HUD/health bar).
            .add_systems(OnEnter(GameState::Playing), spawn_powerup_ui)
            .add_systems(OnExit(GameState::Playing), despawn_marker::<PowerUpUiRoot>)
            // Update gameplay systems (spawn / collect / effects / animation).
            .add_systems(
                Update,
                (
                    spawn_pickup,
                    collect_pickup,
                    apply_speed_boost,
                    apply_coin_magnet,
                    animate_pickups,
                )
                    .run_if(in_state(GameState::Playing)),
            )
            // UI refresh runs in every state so the bars recolor even while
            // paused; the query is trivial when the UI root is absent.
            .add_systems(Update, update_powerup_ui)
            // Flash fade runs in every state so a flash spawned right before
            // a state transition still fades + despawns (it self-despawns,
            // so no separate OnExit cleanup is needed).
            .add_systems(Update, update_pickup_flash)
            // Clean up any lingering power-up orbs on round end / menu return.
            .add_systems(OnEnter(GameState::GameOver), cleanup_pickups)
            .add_systems(OnEnter(GameState::Menu), cleanup_pickups);
    }
}

// ===========================================================================
// Spawn — periodically place a power-up near the car, biased ahead
// ===========================================================================

/// Per-spawn state carried in a system `Local`: the countdown to the next
/// spawn (re-rolled within [MIN, MAX] after each spawn) and a PRNG seed.
#[derive(Default)]
struct SpawnState {
    timer: f32,
    seed: u32,
    initialized: bool,
}

/// Every frame while `Playing`, tick the spawn timer; when it elapses and the
/// active-orb count is below the cap, spawn a new power-up ahead of the car.
/// The kind is picked by weighted probability (see [`pick_kind`]).
fn spawn_pickup(
    mut commands: Commands,
    assets: Res<PickupAssets>,
    car: Query<&Transform, With<Car>>,
    powerups: Query<Entity, With<PowerUp>>,
    time: Res<Time>,
    mut state: Local<SpawnState>,
) {
    let dt = time.delta_secs();
    if !state.initialized {
        // First frame: start the timer at the max so we don't drop a pickup
        // instantly on round start (gives the player a moment to drive).
        state.timer = SPAWN_INTERVAL_MAX;
        state.seed = 0x9e37_79b9;
        state.initialized = true;
    }

    state.timer -= dt;
    if state.timer > 0.0 {
        return;
    }

    // Re-arm the timer with a fresh random interval.
    state.timer =
        SPAWN_INTERVAL_MIN + rand(&mut state.seed) * (SPAWN_INTERVAL_MAX - SPAWN_INTERVAL_MIN);

    // Cap active orbs (web-friendly). If at cap, skip this spawn window.
    if powerups.iter().count() >= MAX_ACTIVE_PICKUPS {
        return;
    }

    let Ok(car_t) = car.single() else {
        return;
    };
    let car_pos = car_t.translation;

    // Spawn somewhere in a cone AHEAD of the car's current heading, then
    // bound reachability in the car-relative lateral axis (never world X).
    let forward = horizontal_forward(car_t.rotation);
    let angle = (rand(&mut state.seed) * 2.0 - 1.0) * SPAWN_HALF_CONE;
    let dist = SPAWN_RADIUS * (0.55 + rand(&mut state.seed) * 0.45); // 55..100% of radius
    let pos = pickup_spawn_pos(car_pos, forward, angle, dist);

    // Weighted kind pick: Health 15%, Time 25%, MegaCoin 15%,
    // SpeedBoost 30%, CoinMagnet 15%.
    let kind = pick_kind(rand(&mut state.seed));

    // Resolve the orb mesh + emissive material for this kind. MegaCoin uses
    // the larger orb mesh; all others share the standard orb mesh.
    let (orb_mesh, orb_mat) = match kind {
        PowerKind::SpeedBoost => (assets.orb_mesh.clone(), assets.boost_mat.clone()),
        PowerKind::CoinMagnet => (assets.orb_mesh.clone(), assets.magnet_mat.clone()),
        PowerKind::Health => (assets.orb_mesh.clone(), assets.health_mat.clone()),
        PowerKind::Time => (assets.orb_mesh.clone(), assets.time_mat.clone()),
        PowerKind::MegaCoin => (assets.mega_orb_mesh.clone(), assets.megacoin_mat.clone()),
    };

    // Top-level entity: world Transform (no chunk parent). The orb sits at
    // ORB_HOVER_Y; the pedestal is a child at ground level.
    commands
        .spawn((
            Transform::from_translation(pos),
            Visibility::default(),
            PowerUp { kind },
        ))
        .with_children(|p| {
            // Glowing orb (the power-up visual).
            p.spawn((
                Mesh3d(orb_mesh),
                MeshMaterial3d(orb_mat),
                Transform::from_xyz(0.0, 0.0, 0.0),
            ));
            // Tiny pedestal at the base (ground level = orb_y - hover).
            p.spawn((
                Mesh3d(assets.pedestal_mesh.clone()),
                MeshMaterial3d(assets.pedestal_mat.clone()),
                Transform::from_xyz(0.0, -ORB_HOVER_Y + 0.125, 0.0),
            ));
        });
}

// ===========================================================================
// Collect — car drives through an orb, activate the effect
// ===========================================================================

/// When the car is within `PICKUP_RADIUS` of a power-up orb, despawn the orb
/// and apply the corresponding effect. Timed kinds (SpeedBoost / CoinMagnet)
/// arm a timer resource; instant kinds (Health / Time / MegaCoin) apply their
/// effect immediately + spawn a brief colored flash. Plays a pickup SFX
/// (reuses coin.wav) via `AudioPlayer` + `PlaybackSettings::DESPAWN`.
fn collect_pickup(
    car: Query<&Transform, With<Car>>,
    mut powerups: Query<(Entity, &PowerUp, &Transform), Without<Car>>,
    mut commands: Commands,
    mut boost: ResMut<SpeedBoostTimer>,
    mut magnet: ResMut<MagnetTimer>,
    mut health: ResMut<Health>,
    mut timeleft: ResMut<TimeLeft>,
    mut score: ResMut<Score>,
    mut coin_events: MessageWriter<CoinCollected>,
    audio: Res<PickupAudio>,
) {
    let Ok(car_t) = car.single() else {
        return;
    };
    let car_pos = car_t.translation;
    // Only consider XZ distance (orbs hover above the ground; the car is at
    // y≈0). Ignoring Y means driving under an orb still collects it.
    let car_xz = Vec3::new(car_pos.x, 0.0, car_pos.z);

    for (e, power, tf) in &mut powerups {
        let p_xz = Vec3::new(tf.translation.x, 0.0, tf.translation.z);
        if car_xz.distance(p_xz) < PICKUP_RADIUS {
            // Activate the effect. Timed kinds arm a timer; instant kinds
            // mutate their resource / score + flash. Every match arm is
            // covered so adding a kind later stays exhaustive.
            match power.kind {
                PowerKind::SpeedBoost => boost.0 = SPEED_BOOST_DURATION,
                PowerKind::CoinMagnet => magnet.0 = MAGNET_DURATION,
                PowerKind::Health => {
                    health.0 = (health.0 + HEALTH_RESTORE).min(HEALTH_MAX);
                    spawn_pickup_flash(&mut commands, health_flash_rgb());
                }
                PowerKind::Time => {
                    timeleft.0 = (timeleft.0 + TIME_BONUS).min(TIME_CAP);
                    spawn_pickup_flash(&mut commands, time_flash_rgb());
                }
                PowerKind::MegaCoin => {
                    score.coins += MEGA_COIN_AMOUNT;
                    // One message -> audio.rs plays the coin chime + combos.rs
                    // applies the combo multiplier (the +5 score is applied
                    // directly above; the message is the "coin got" signal).
                    coin_events.write(CoinCollected);
                    spawn_pickup_flash(&mut commands, megacoin_flash_rgb());
                }
            }
            // Despawn the orb (recursive in 0.19 — nukes the orb + pedestal
            // children; safe, risk E2).
            commands.entity(e).despawn();
            // Pickup chime (reuses coin.wav; DESPAWN reclaims the audio entity
            // once the clip finishes). Carries its authored gain as
            // `AudioBaseGain` so the live master bridge scales it without
            // compounding (mirrors `audio.rs` one-shots).
            commands.spawn((
                AudioPlayer::new(audio.sfx.clone()),
                PlaybackSettings::DESPAWN.with_volume(Volume::Linear(0.8)),
                AudioBaseGain(0.8),
            ));
        }
    }
}

// ===========================================================================
// Effects — driven entirely from resources + queries owned here
// ===========================================================================

/// While `SpeedBoostTimer > 0`, add a forward boost acceleration to
/// `car.speed` each frame, capped at `max_speed * 1.6`. Decrement the timer.
/// Directly mutating `car.speed` is fine — `move_car`'s eased approach still
/// drives the car; this just keeps pushing the effective speed higher while
/// the boost lasts.
fn apply_speed_boost(
    mut car: Query<&mut Car>,
    mut timer: ResMut<SpeedBoostTimer>,
    cfg: Res<GameConfig>,
    time: Res<Time>,
) {
    if timer.0 <= 0.0 {
        return;
    }
    let dt = time.delta_secs();
    let cap = cfg.max_speed * SPEED_BOOST_CAP_MULT;

    if let Ok(mut c) = car.single_mut() {
        // Only boost while moving forward (don't help reversing). Nudge speed
        // up toward the cap; if the car is braking/coasting this still pushes
        // it forward so the boost feels meaningful.
        if c.speed >= 0.0 {
            c.speed = (c.speed + SPEED_BOOST_ACCEL * dt).min(cap);
        }
    }

    timer.0 = (timer.0 - dt).max(0.0);
}

/// While `MagnetTimer > 0`, pull at most the nearest
/// [`MAX_MAGNET_COINS_PER_FRAME`] coins inside [`MAGNET_RADIUS`] toward the
/// car, in world space. `world.rs::collect_coins` remains responsible for
/// collection once a coin is within its normal collection radius.
///
/// Coins are chunk-root children → their `Transform` is LOCAL. We read each
/// coin's world position via `GlobalTransform`, compute the world-space pull
/// delta, and apply that same delta to the coin's local `Transform`. This is
/// valid because chunk roots carry only translation (no rotation/scale), so a
/// world-space translation delta equals the local-space translation delta.
/// Freshly spawned coins whose transform propagation still reads as
/// `GlobalTransform::IDENTITY` are ignored for this frame.
fn apply_coin_magnet(
    car: Query<&Transform, (With<Car>, Without<Coin>)>,
    mut coins: Query<(Entity, &GlobalTransform, &mut Transform), (With<Coin>, Without<Car>)>,
    mut timer: ResMut<MagnetTimer>,
    time: Res<Time>,
    mut nearest: Local<Vec<RankedMagnetCandidate<Entity, (Entity, Vec3)>>>,
) {
    if timer.0 <= 0.0 {
        return;
    }
    let dt = time.delta_secs();
    let Ok(car_t) = car.single() else {
        timer.0 = (timer.0 - dt).max(0.0);
        return;
    };
    let car_xz = car_t.translation;

    // Retain this small allocation between frames. Its length can never grow
    // beyond the fixed cap, and the insertion helper keeps it nearest-first,
    // independent of ECS query iteration order.
    nearest.clear();
    if nearest.capacity() < MAX_MAGNET_COINS_PER_FRAME {
        nearest.reserve_exact(MAX_MAGNET_COINS_PER_FRAME);
    }
    for (entity, gt, _) in &mut coins {
        let propagated = has_propagated_global_transform(gt);
        if let Some(step) = magnet_attraction_step(gt.translation(), car_xz, dt, propagated) {
            insert_capped_nearest(
                &mut *nearest,
                RankedMagnetCandidate {
                    distance_squared: step.distance_squared,
                    stable_key: entity,
                    value: (entity, step.delta),
                },
                MAX_MAGNET_COINS_PER_FRAME,
            );
        }
    }

    for candidate in nearest.drain(..) {
        let (entity, delta) = candidate.value;
        if let Ok((_, _, mut tf)) = coins.get_mut(entity) {
            // Chunk roots are translation-only, so this world-space delta is
            // also the correct delta for the child coin's local Transform.
            tf.translation += delta;
        }
    }

    timer.0 = (timer.0 - dt).max(0.0);
}

// ===========================================================================
// Animation — gentle bob + spin for each orb
// ===========================================================================

/// Bob each power-up orb vertically and spin it so it reads as a lively
/// pickup. We animate the root entity's rotation (spin around Y) and its
/// translation Y (gentle bob around the hover height, with a per-orb phase
/// derived from its XZ position so multiple orbs don't bounce in lockstep).
/// The orb + pedestal mesh children ride along with the root transform.
fn animate_pickups(mut powerups: Query<(&mut Transform, &PowerUp)>, time: Res<Time>) {
    let t = time.elapsed_secs();
    for (mut tf, _power) in &mut powerups {
        // Spin around Y.
        let spin = t * 1.5;
        tf.rotation = Quat::from_rotation_y(spin);
        // Gentle bob around the hover height, with a per-orb phase so they
        // don't all bounce in sync.
        let phase = tf.translation.x * 1.7 + tf.translation.z * 0.9;
        tf.translation.y = ORB_HOVER_Y + (t * 1.8 + phase).sin() * 0.18;
    }
}

// ===========================================================================
// Reset / cleanup
// ===========================================================================

/// Fresh-round reset: zero the effect timers and despawn any active power-up
/// orbs (covers ALL `PowerUp` kinds). The cleanup-driven latch skips pause
/// resume and remains safe regardless of `reset_run` / `SpawnSet` ordering.
fn reset_pickups(
    mut boost: ResMut<SpeedBoostTimer>,
    mut magnet: ResMut<MagnetTimer>,
    powerups: Query<Entity, With<PowerUp>>,
    mut commands: Commands,
    mut reset_pending: ResMut<PickupResetPending>,
) {
    if !reset_pending.0 {
        return;
    }
    reset_pending.0 = false;
    boost.0 = 0.0;
    magnet.0 = 0.0;
    // `With<PowerUp>` matches every kind, so new kinds are covered for free.
    for e in &powerups {
        commands.entity(e).despawn();
    }
}

/// Despawn every active power-up orb (e.g. on GameOver / Menu). Covers ALL
/// `PowerUp` kinds. Recursive despawn in 0.19 is safe here (nukes the orb +
/// pedestal children).
fn cleanup_pickups(
    mut commands: Commands,
    powerups: Query<Entity, With<PowerUp>>,
    mut boost: ResMut<SpeedBoostTimer>,
    mut magnet: ResMut<MagnetTimer>,
    mut reset_pending: ResMut<PickupResetPending>,
) {
    // `With<PowerUp>` matches every kind, so new kinds are covered for free.
    for e in &powerups {
        commands.entity(e).despawn();
    }
    // Also zero timers so no effect bleeds into the next round.
    boost.0 = 0.0;
    magnet.0 = 0.0;
    reset_pending.0 = true;
}

/// Despawn every entity tagged with marker `M` (mirrors `ui.rs` / `health.rs`).
fn despawn_marker<M: Component>(mut commands: Commands, q: Query<Entity, With<M>>) {
    for e in &q {
        commands.entity(e).despawn();
    }
}

// ===========================================================================
// UI — active power-up icons + remaining-time bars (bottom-center, above
// the health bar)
// ===========================================================================

/// Spawn the bottom-center power-up panel. Two rows (SpeedBoost, CoinMagnet),
/// each with a colored icon label and a timer bar. Instant kinds (Health /
/// Time / MegaCoin) have no row — they flash on collect instead. Lives only
/// while `Playing` (despawned by [`despawn_marker::<PowerUpUiRoot>`] on exit).
fn spawn_powerup_ui(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: px(UI_BOTTOM),
                left: px(0.0),
                width: Val::Percent(100.0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                flex_direction: FlexDirection::Column,
                ..default()
            },
            PowerUpUiRoot,
        ))
        .with_children(|col| {
            // Inner panel so the rows sit in a tidy boxed cluster.
            col.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(px(6.0)),
                    row_gap: px(4.0),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.30)),
            ))
            .with_children(|panel| {
                // SpeedBoost row (timed — has a bar).
                powerup_row(panel, "BOOST", boost_color(), PowerKind::SpeedBoost);
                // CoinMagnet row (timed — has a bar).
                powerup_row(panel, "MAGNET", magnet_color(), PowerKind::CoinMagnet);
            });
        });
}

/// Build one power-up UI row: an icon (colored dot + label) and a timer bar
/// track with a colored fill child. The row starts hidden (inactive); the
/// update system shows it when its effect is active. Only timed kinds get a
/// real fill marker; the instant-kind arms exist purely for match
/// exhaustiveness (no row is spawned for them).
fn powerup_row(parent: &mut ChildSpawnerCommands, label: &str, color: Color, kind: PowerKind) {
    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(6.0),
                ..default()
            },
            PowerUpRow { kind },
            // Start hidden until the effect activates.
            Visibility::Hidden,
        ))
        .with_children(|row| {
            // Colored icon dot.
            row.spawn((
                Node {
                    width: px(10.0),
                    height: px(10.0),
                    ..default()
                },
                BackgroundColor(color),
            ));
            // Label.
            row.spawn((
                Text::new(label),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(Color::srgba(0.85, 0.85, 0.9, 1.0)),
                Node {
                    width: px(52.0),
                    ..default()
                },
            ));
            // Bar track (dark) with a colored fill child. The fill carries a
            // kind-specific marker so `update_powerup_ui` can drive its width
            // via a typed query. Timed kinds get a real fill marker; instant
            // kinds get a bare track (no row is spawned for them, but the
            // match stays exhaustive).
            let track_node = Node {
                width: px(UI_BAR_W),
                height: px(UI_BAR_H),
                ..default()
            };
            let track_bg = BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6));
            let fill_node = Node {
                width: px(0.0),
                height: Val::Percent(100.0),
                ..default()
            };
            let fill_bg = BackgroundColor(color);
            match kind {
                PowerKind::SpeedBoost => {
                    row.spawn((track_node, track_bg)).with_child((
                        fill_node,
                        fill_bg,
                        BoostBarFill,
                    ));
                }
                PowerKind::CoinMagnet => {
                    row.spawn((track_node, track_bg)).with_child((
                        fill_node,
                        fill_bg,
                        MagnetBarFill,
                    ));
                }
                // Instant kinds: no timer bar. These arms are unreachable
                // (no row is spawned for them) but keep the match exhaustive.
                PowerKind::Health | PowerKind::Time | PowerKind::MegaCoin => {
                    row.spawn((track_node, track_bg));
                }
            }
        });
}

/// Refresh the power-up UI each frame: show/hide each row based on whether
/// its effect is active, and set the timer-bar fill width to the remaining
/// fraction. Runs in every state; the query is empty when the UI root is
/// absent (e.g. in Menu), so it's a no-op then. Instant kinds report a 0
/// fraction (they have no row, but the match stays exhaustive).
fn update_powerup_ui(
    boost: Res<SpeedBoostTimer>,
    magnet: Res<MagnetTimer>,
    mut rows: Query<(&PowerUpRow, &mut Visibility)>,
    mut boost_fills: Query<&mut Node, (With<BoostBarFill>, Without<MagnetBarFill>)>,
    mut magnet_fills: Query<&mut Node, (With<MagnetBarFill>, Without<BoostBarFill>)>,
) {
    let boost_frac = (boost.0 / SPEED_BOOST_DURATION).clamp(0.0, 1.0);
    let magnet_frac = (magnet.0 / MAGNET_DURATION).clamp(0.0, 1.0);

    // Drive the fill widths directly (one entity each).
    for mut node in &mut boost_fills {
        node.width = px(UI_BAR_W * boost_frac);
    }
    for mut node in &mut magnet_fills {
        node.width = px(UI_BAR_W * magnet_frac);
    }

    // Show/hide each row based on whether its effect is active. Instant kinds
    // never have a row, so their arms report 0.0 (unreachable but exhaustive).
    for (row, mut vis) in &mut rows {
        let frac = match row.kind {
            PowerKind::SpeedBoost => boost_frac,
            PowerKind::CoinMagnet => magnet_frac,
            PowerKind::Health | PowerKind::Time | PowerKind::MegaCoin => 0.0,
        };
        *vis = if frac > 0.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

// ===========================================================================
// Instant-pickup flash — brief colored screen tint on Health/Time/MegaCoin
// ===========================================================================

/// Spawn a full-screen colored overlay that fades over [`PICKUP_FLASH_DURATION`].
/// `rgb` is the (r, g, b) tint in 0..1.
fn spawn_pickup_flash(commands: &mut Commands, rgb: (f32, f32, f32)) {
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: px(0.0),
            left: px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(Color::srgba(rgb.0, rgb.1, rgb.2, PICKUP_FLASH_PEAK_ALPHA)),
        PickupFlash {
            t: PICKUP_FLASH_DURATION,
            r: rgb.0,
            g: rgb.1,
            b: rgb.2,
        },
    ));
}

/// Fade the flash alpha toward 0 and despawn once expired. Runs in every
/// state so a flash spawned right before a state transition still fades out.
fn update_pickup_flash(
    mut commands: Commands,
    time: Res<Time>,
    mut q: Query<(Entity, &mut PickupFlash, &mut BackgroundColor)>,
) {
    for (e, mut flash, mut bg) in &mut q {
        flash.t -= time.delta_secs();
        if flash.t <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        let frac = (flash.t / PICKUP_FLASH_DURATION).clamp(0.0, 1.0);
        bg.0 = Color::srgba(flash.r, flash.g, flash.b, PICKUP_FLASH_PEAK_ALPHA * frac);
    }
}

// ===========================================================================
// Colors
// ===========================================================================

/// SpeedBoost UI color (matches the orb's blue).
fn boost_color() -> Color {
    Color::srgb(0.35, 0.6, 1.0)
}

/// CoinMagnet UI color (matches the orb's gold).
fn magnet_color() -> Color {
    Color::srgb(1.0, 0.78, 0.2)
}

/// Health flash tint RGB (green).
fn health_flash_rgb() -> (f32, f32, f32) {
    (0.2, 0.9, 0.3)
}

/// TimeBonus flash tint RGB (cyan).
fn time_flash_rgb() -> (f32, f32, f32) {
    (0.2, 0.72, 0.95)
}

/// MegaCoin flash tint RGB (bright gold).
fn megacoin_flash_rgb() -> (f32, f32, f32) {
    (1.0, 0.85, 0.2)
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Integer weights make the selection boundaries exact and easy to audit.
/// The 15 points removed from CoinMagnet are split between Health, Time, and
/// SpeedBoost; MegaCoin remains unchanged.
const POWER_KIND_WEIGHTS: [(PowerKind, u32); 5] = [
    (PowerKind::Health, 15),
    (PowerKind::Time, 25),
    (PowerKind::MegaCoin, 15),
    (PowerKind::SpeedBoost, 30),
    (PowerKind::CoinMagnet, 15),
];
const POWER_KIND_WEIGHT_TOTAL: u32 = 100;

/// Pure weighted-selection boundary. `bucket` values outside the weighted
/// range clamp to its final bucket, keeping CoinMagnet at exactly 15/100.
fn kind_for_weight_bucket(bucket: u32) -> PowerKind {
    let bucket = bucket.min(POWER_KIND_WEIGHT_TOTAL - 1);
    let mut boundary = 0;
    for (kind, weight) in POWER_KIND_WEIGHTS {
        boundary += weight;
        if bucket < boundary {
            return kind;
        }
    }
    unreachable!("power-up weights must cover the full bucket range")
}

/// Weighted power-up kind picker. `r` is expected to be uniform in [0, 1].
fn pick_kind(r: f32) -> PowerKind {
    let normalized = if r.is_finite() {
        r.clamp(0.0, 1.0)
    } else {
        0.0
    };
    kind_for_weight_bucket((normalized * POWER_KIND_WEIGHT_TOTAL as f32) as u32)
}

#[derive(Clone, Copy, Debug)]
struct MagnetAttractionStep {
    distance_squared: f32,
    delta: Vec3,
}

/// Pure attraction calculation. Returning `None` leaves unresolved or distant
/// coins untouched. The interpolation factor is clamped to prevent overshoot.
fn magnet_attraction_step(
    world: Vec3,
    car: Vec3,
    dt: f32,
    global_transform_propagated: bool,
) -> Option<MagnetAttractionStep> {
    if !global_transform_propagated {
        return None;
    }

    let target = Vec3::new(car.x, world.y, car.z);
    let to_car = target - world;
    let distance_squared = to_car.length_squared();
    if distance_squared > MAGNET_RADIUS * MAGNET_RADIUS {
        return None;
    }

    let fraction = (MAGNET_STRENGTH * dt.max(0.0)).clamp(0.0, 1.0);
    Some(MagnetAttractionStep {
        distance_squared,
        delta: to_car * fraction,
    })
}

/// An identity global transform usually means transform propagation has not
/// reached a freshly spawned child coin yet. Using it as world-space data
/// would pull from the world origin and corrupt the local/world conversion.
fn has_propagated_global_transform(global: &GlobalTransform) -> bool {
    *global != GlobalTransform::IDENTITY
}

#[derive(Clone, Copy, Debug)]
struct RankedMagnetCandidate<K, V> {
    distance_squared: f32,
    stable_key: K,
    value: V,
}

fn magnet_candidate_precedes<K: Ord, V>(
    left: &RankedMagnetCandidate<K, V>,
    right: &RankedMagnetCandidate<K, V>,
) -> bool {
    left.distance_squared
        .total_cmp(&right.distance_squared)
        .then_with(|| left.stable_key.cmp(&right.stable_key))
        .is_lt()
}

/// Insert into a nearest-first fixed-cap set without ever temporarily growing
/// beyond `cap`. Stable-key tie-breaking makes the result query-order neutral.
fn insert_capped_nearest<K: Ord, V>(
    candidates: &mut Vec<RankedMagnetCandidate<K, V>>,
    candidate: RankedMagnetCandidate<K, V>,
    cap: usize,
) {
    if cap == 0 {
        return;
    }

    if candidates.len() < cap {
        candidates.push(candidate);
    } else if magnet_candidate_precedes(&candidate, candidates.last().unwrap()) {
        *candidates.last_mut().unwrap() = candidate;
    } else {
        return;
    }

    let mut index = candidates.len() - 1;
    while index > 0 && magnet_candidate_precedes(&candidates[index], &candidates[index - 1]) {
        candidates.swap(index, index - 1);
        index -= 1;
    }
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

/// Pure car-relative pickup placement. The cone always contributes a positive
/// forward component, while lateral displacement is bounded around the car.
fn pickup_spawn_pos(car_pos: Vec3, forward: Vec3, angle: f32, distance: f32) -> Vec3 {
    let forward = normalized_horizontal(forward);
    let right = Vec3::new(-forward.z, 0.0, forward.x);
    let (sin, cos) = angle.clamp(-SPAWN_HALF_CONE, SPAWN_HALF_CONE).sin_cos();
    let ahead = distance.max(0.0) * cos;
    let lateral = (distance.max(0.0) * sin).clamp(-SPAWN_LATERAL_RANGE, SPAWN_LATERAL_RANGE);
    let mut pos = car_pos + forward * ahead + right * lateral;
    pos.y = ORB_HOVER_Y;
    pos
}

/// Tiny LCG for deterministic-but-varied placement without pulling in `rand`
/// (matches the `world.rs` / `chickens.rs` style).
fn rand(seed: &mut u32) -> f32 {
    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
    (*seed as f32) / (u32::MAX as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pickup_ahead_tracks_zero_ninety_and_one_eighty_degree_headings() {
        let car_pos = Vec3::new(5.0, 0.0, 9.0);
        let cases = [
            (0.0, Vec3::new(5.0, ORB_HOVER_Y, -1.0)),
            (
                std::f32::consts::FRAC_PI_2,
                Vec3::new(-5.0, ORB_HOVER_Y, 9.0),
            ),
            (std::f32::consts::PI, Vec3::new(5.0, ORB_HOVER_Y, 19.0)),
        ];

        for (yaw, expected) in cases {
            let forward = horizontal_forward(Quat::from_rotation_y(yaw));
            let actual = pickup_spawn_pos(car_pos, forward, 0.0, 10.0);
            assert!(
                (actual - expected).length() < 0.0001,
                "{actual:?} != {expected:?}"
            );
            assert!((actual - car_pos).dot(forward) > 0.0);
        }
    }

    #[test]
    fn pickup_lateral_range_is_centered_on_car_not_world_x() {
        let car_pos = Vec3::new(200.0, 0.0, -150.0);
        let forward = horizontal_forward(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        let right = Vec3::new(-forward.z, 0.0, forward.x);
        let pos = pickup_spawn_pos(car_pos, forward, SPAWN_HALF_CONE, 1_000.0);
        let relative = pos - car_pos;

        assert!((relative.dot(right) - SPAWN_LATERAL_RANGE).abs() < 0.0001);
        assert!(relative.dot(forward) > 0.0);
        assert!(pos.x.abs() > 22.0);
    }

    #[test]
    fn kind_weights_and_boundaries_are_exact() {
        assert_eq!(
            POWER_KIND_WEIGHTS,
            [
                (PowerKind::Health, 15),
                (PowerKind::Time, 25),
                (PowerKind::MegaCoin, 15),
                (PowerKind::SpeedBoost, 30),
                (PowerKind::CoinMagnet, 15),
            ]
        );
        assert_eq!(
            POWER_KIND_WEIGHTS
                .iter()
                .map(|(_, weight)| weight)
                .sum::<u32>(),
            POWER_KIND_WEIGHT_TOTAL
        );

        let mut magnet_count = 0;
        for bucket in 0..POWER_KIND_WEIGHT_TOTAL {
            if kind_for_weight_bucket(bucket) == PowerKind::CoinMagnet {
                magnet_count += 1;
            }
        }
        assert_eq!(kind_for_weight_bucket(14), PowerKind::Health);
        assert_eq!(kind_for_weight_bucket(15), PowerKind::Time);
        assert_eq!(kind_for_weight_bucket(39), PowerKind::Time);
        assert_eq!(kind_for_weight_bucket(40), PowerKind::MegaCoin);
        assert_eq!(kind_for_weight_bucket(54), PowerKind::MegaCoin);
        assert_eq!(kind_for_weight_bucket(55), PowerKind::SpeedBoost);
        assert_eq!(kind_for_weight_bucket(84), PowerKind::SpeedBoost);
        assert_eq!(kind_for_weight_bucket(85), PowerKind::CoinMagnet);
        assert!(magnet_count * 100 <= POWER_KIND_WEIGHT_TOTAL * 15);
    }

    #[test]
    fn nearest_candidate_set_has_a_hard_deterministic_cap() {
        let mut candidates = Vec::with_capacity(MAX_MAGNET_COINS_PER_FRAME);
        for key in (0_u32..80).rev() {
            insert_capped_nearest(
                &mut candidates,
                RankedMagnetCandidate {
                    distance_squared: (key % 30) as f32,
                    stable_key: key,
                    value: key,
                },
                MAX_MAGNET_COINS_PER_FRAME,
            );
            assert!(candidates.len() <= MAX_MAGNET_COINS_PER_FRAME);
        }

        assert_eq!(candidates.len(), MAX_MAGNET_COINS_PER_FRAME);
        assert!(
            candidates
                .windows(2)
                .all(|pair| { !magnet_candidate_precedes(&pair[1], &pair[0]) })
        );
        assert_eq!(candidates[0].stable_key, 0);
    }

    #[test]
    fn magnet_step_cannot_overshoot_the_car() {
        let world = Vec3::new(-8.0, 2.0, 0.0);
        let car = Vec3::new(0.0, 99.0, 0.0);
        let step = magnet_attraction_step(world, car, 10.0, true).unwrap();
        let moved = world + step.delta;

        assert_eq!(moved, Vec3::new(0.0, 2.0, 0.0));
        assert!(step.delta.length() <= (Vec3::new(car.x, world.y, car.z) - world).length());
    }

    #[test]
    fn magnet_leaves_far_coins_untouched() {
        let world = Vec3::new(MAGNET_RADIUS + 0.01, 1.0, 0.0);
        assert!(magnet_attraction_step(world, Vec3::ZERO, 1.0 / 60.0, true).is_none());
    }

    #[test]
    fn identity_global_transform_is_treated_as_unpropagated() {
        assert!(!has_propagated_global_transform(&GlobalTransform::IDENTITY));
        assert!(
            magnet_attraction_step(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), 1.0 / 60.0, false)
                .is_none()
        );
    }
}
