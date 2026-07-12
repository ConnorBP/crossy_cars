//! Tire trails: skid marks on the ground during fast turns + dust/smoke
//! particles at the wheels.
//!
//! This module is self-contained and wired by the orchestrator with
//! `mod effects; add_plugins(EffectsPlugin)`. It only *reads* the car
//! (`crate::car::Car` + `Transform`) and ground facts from `crate::world`
//! (ground plane at y = 0; tire marks sit just above at y ≈ 0.02 to avoid
//! z-fighting). No other `.rs` file is edited.
//!
//! Web/perf notes:
//! - All mark/particle counts are **capped** and **pooled** (fixed max,
//!   recycle oldest). When the pool is full we re-purpose an existing entity
//!   (give it a fresh transform + reset its age/material) instead of spawning
//!   a new one, so the entity count is bounded forever.
//! - Materials: rather than one handle per type (which would make per-entity
//!   alpha fade impossible — you can't tweak a shared material per entity), we
//!   use a **small fixed ladder** of fade-step materials (8 for marks, 6 for
//!   smoke) from full alpha down to ~transparent. Each entity swaps its
//!   material handle as it ages. That's a bounded, tiny handle count (web-
//!   friendly: 14 handles total, not 320 per-entity materials) and gives true
//!   alpha fade as the task asks.
//! - No custom shaders (primitives only): the WebGL2 16-byte uniform rule
//!   doesn't bite.

use bevy::prelude::*;

use crate::car::Car;
use crate::game::events::ObstacleHit;
use crate::game::state::GameState;

// ===========================================================================
// Constants — tuned for the car in `car.rs`
// ===========================================================================

/// Rear-wheel offsets in CAR-LOCAL space (x, z). The car drives toward -Z at
/// heading 0, so the rear is +Z. The wheels in `car.rs` sit at
/// `(±0.6, 0.7)`/`(±0.6, -0.7)`; the rear pair is the +0.7 (z) ones. Y is the
/// ground-contact height for the mark; particles start a touch higher.
const REAR_WHEEL_X: f32 = 0.6;
const REAR_WHEEL_Z: f32 = 0.7;

/// Y for tire-mark quads — just above the ground (y = 0) to avoid z-fighting
/// with the road plane (world.rs renders the road at y = 0.02, so we sit a
/// hair above it).
const MARK_Y: f32 = 0.025;

/// Tire-mark quad footprint (world units). LEN is along travel (Z at heading
/// 0), WID is across. `Plane3d::default().mesh().size(x, z)` maps the first
/// arg to the X extent and the second to the Z extent (confirmed against
/// `world.rs`'s road: `size(8.0, length)` → 8 wide in X, length along Z), so
/// we pass WID first, LEN second to put the length along travel.
const MARK_LEN: f32 = 0.5;
const MARK_WID: f32 = 0.22;

/// Smoke particle base radius (sphere diameter is 2× this).
const SMOKE_RADIUS: f32 = 0.18;

/// "Fast turn" thresholds. A skid happens when the car is turning quickly
/// *and* moving fast enough that the tires would actually scrub.
/// `ANG_VEL_THRESHOLD` is radians/sec; `SPEED_THRESHOLD` is world u/sec.
const ANG_VEL_THRESHOLD: f32 = 1.2;
const SPEED_THRESHOLD: f32 = 4.0;

/// Tire-mark lifetime (seconds) before it fades out fully and is recycled.
const MARK_LIFETIME: f32 = 3.5;

/// Smoke-particle lifetime (seconds): rise + expand + fade.
const SMOKE_LIFETIME: f32 = 0.45;

/// Number of alpha-fade steps for marks (full → ~transparent). Bounded so the
/// total material handle count stays tiny (web-friendly).
const MARK_FADE_STEPS: usize = 8;
/// Peak (fresh) alpha for a tire mark.
const MARK_PEAK_ALPHA: f32 = 0.55;

/// Number of alpha-fade steps for smoke puffs.
const SMOKE_FADE_STEPS: usize = 6;
/// Peak alpha for a fresh smoke puff.
const SMOKE_PEAK_ALPHA: f32 = 0.5;

/// Pool caps (web-friendly: bounded entity count). When full, an existing
/// entity is recycled (re-positioned + age/material reset) instead of
/// spawning new.
const MARK_POOL_CAP: usize = 240;
const SMOKE_POOL_CAP: usize = 80;

/// How often (seconds) we lay down a new mark pair while skidding. Keeps the
/// trail dense but bounded — at 60fps this would otherwise spawn 120/s.
const MARK_EMIT_INTERVAL: f32 = 0.03;

/// How often we emit a smoke puff burst while skidding.
const SMOKE_EMIT_INTERVAL: f32 = 0.05;

/// Emit a smoke puff on obstacle hits above this impact speed.
const HIT_SMOKE_SPEED: f32 = 3.0;

// ===========================================================================
// Assets — FromWorld so handles exist before any Update system runs
// ===========================================================================

/// Shared mesh + fade-ladder material handles for marks and smoke. Built once
/// via `FromWorld` (resource-scoping `Assets<Mesh>` then
/// `Assets<StandardMaterial>` — mirrors `textures.rs::TextureAssets`).
///
/// The material fields are **Vec**s of handles forming an alpha ladder from
/// full (index 0) to ~transparent (last index); entities swap to the bucket
/// matching their age for per-entity alpha fade without per-entity materials.
#[derive(Resource)]
pub struct EffectsAssets {
    /// Flat dark quad used for every tire mark (many entities share it).
    mark_mesh: Handle<Mesh>,
    /// Alpha ladder for marks: `[0]` = full alpha, `[MARK_FADE_STEPS-1]` =
    /// ~transparent. Length `MARK_FADE_STEPS`.
    mark_materials: Vec<Handle<StandardMaterial>>,
    /// Small sphere mesh for every smoke particle.
    smoke_mesh: Handle<Mesh>,
    /// Alpha ladder for smoke puffs. Length `SMOKE_FADE_STEPS`.
    smoke_materials: Vec<Handle<StandardMaterial>>,
}

impl FromWorld for EffectsAssets {
    fn from_world(world: &mut World) -> Self {
        // Scope meshes first (like textures.rs scopes Images), then grab
        // materials inside the closure so we never hold two `&mut Assets<…>`
        // at once without scoping (risk E3).
        world.resource_scope::<Assets<Mesh>, _>(|world, mut meshes| {
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();

            // Flat quad lying in the XZ plane (normal +Y) for tire marks.
            // Plane3d::default() already lies in XZ; size(WID, LEN) puts the
            // length along Z (travel at heading 0) and width along X.
            let mark_mesh = meshes.add(Plane3d::default().mesh().size(MARK_WID, MARK_LEN));

            // Alpha ladder for marks: dark, unlit, Blend, from peak → ~0.
            let mark_materials = fade_ladder(
                &mut materials,
                MARK_FADE_STEPS,
                Color::srgb(0.03, 0.03, 0.03),
                MARK_PEAK_ALPHA,
            );

            // Small sphere for smoke puffs.
            let smoke_mesh = meshes.add(Sphere::new(SMOKE_RADIUS).mesh().uv(8, 6));

            // Alpha ladder for smoke: soft grey, unlit, Blend, peak → ~0.
            let smoke_materials = fade_ladder(
                &mut materials,
                SMOKE_FADE_STEPS,
                Color::srgb(0.62, 0.60, 0.55),
                SMOKE_PEAK_ALPHA,
            );

            EffectsAssets {
                mark_mesh,
                mark_materials,
                smoke_mesh,
                smoke_materials,
            }
        })
    }
}

/// Build a ladder of `steps` `StandardMaterial` handles all sharing the same
/// `rgb` but with alpha linearly interpolated from `peak_alpha` (index 0) down
/// to `peak_alpha / steps` (last index, ~transparent — not exactly 0 so the
/// final step still renders faintly before despawn/recycle). All unlit +
/// `AlphaMode::Blend` so they composite over the road/car without lighting
/// interaction.
fn fade_ladder(
    materials: &mut Assets<StandardMaterial>,
    steps: usize,
    rgb: Color,
    peak_alpha: f32,
) -> Vec<Handle<StandardMaterial>> {
    let [r, g, b] = [
        rgb.to_linear().red,
        rgb.to_linear().green,
        rgb.to_linear().blue,
    ];
    (0..steps)
        .map(|i| {
            // frac: 0.0 at full alpha (i=0) → ~1.0 at last step.
            let frac = if steps <= 1 {
                0.0
            } else {
                i as f32 / (steps - 1) as f32
            };
            // Ease the fade a touch (alpha holds up then drops) for a nicer
            // tail: alpha = peak * (1 - frac)^1.3.
            let a = peak_alpha * (1.0 - frac).powf(1.3);
            materials.add(StandardMaterial {
                base_color: Color::srgba(r as f32, g as f32, b as f32, a.max(0.02)),
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                ..default()
            })
        })
        .collect()
}

// ===========================================================================
// Components
// ===========================================================================

/// A tire-mark quad. `age` counts up from 0; when it exceeds `MARK_LIFETIME`
/// the entity is hidden and becomes a candidate for recycling. The material
/// handle is swapped each frame to the alpha bucket matching `age` (true
/// per-entity alpha fade via a bounded material ladder).
#[derive(Component)]
struct TireMark {
    age: f32,
}

/// A smoke/dust particle. `age` counts up; at `SMOKE_LIFETIME` it is hidden
/// and remains available for deterministic pool reuse. `seed` gives each puff
/// a slightly different rise/expansion so a burst doesn't look synchronized.
#[derive(Component)]
struct Smoke {
    age: f32,
    /// Per-particle random 0..1 for varied rise speed + drift.
    seed: f32,
}

// ===========================================================================
// Plugin
// ===========================================================================

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EffectsAssets>()
            .add_systems(
                Update,
                (emit_tire_effects, fade_tire_marks, update_smoke)
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            )
            // Pausing preserves both visible effects and their reusable pool
            // slots. Only terminal round destinations purge the pools; this
            // also covers paused restarts, which deliberately route via Menu.
            .add_systems(OnEnter(GameState::GameOver), cleanup_effects)
            .add_systems(OnEnter(GameState::Menu), cleanup_effects);
    }
}

// ===========================================================================
// Emit — detect fast turns and lay down marks + smoke
// ===========================================================================

/// Per-car state carried in a system `Local` (we can't add a component to the
/// car without editing `car.rs`). Tracks the previous heading (to derive
/// angular velocity) and emit timers to bound spawn rate.
#[derive(Default)]
struct EmitState {
    prev_heading: f32,
    mark_timer: f32,
    smoke_timer: f32,
    initialized: bool,
}

/// Detect fast turns and spawn tire marks + smoke at the rear wheels. Also
/// reads `ObstacleHit` for a small impact smoke puff (multiple readers of the
/// same message are fine — T2 reads it too).
fn emit_tire_effects(
    car: Query<(&Car, &Transform), (With<Car>, Without<TireMark>, Without<Smoke>)>,
    mut commands: Commands,
    assets: Res<EffectsAssets>,
    time: Res<Time>,
    mut state: Local<EmitState>,
    mut marks: Query<
        (
            Entity,
            &mut TireMark,
            &mut Transform,
            &mut Visibility,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        (With<TireMark>, Without<Smoke>),
    >,
    mut smokes: Query<
        (
            Entity,
            &mut Smoke,
            &mut Transform,
            &mut Visibility,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        (With<Smoke>, Without<TireMark>),
    >,
    mut obstacle_hits: MessageReader<ObstacleHit>,
) {
    let Ok((car, car_t)) = car.single() else {
        return;
    };
    let dt = time.delta_secs();
    let mut mark_emissions = Vec::with_capacity(2);
    let mut smoke_emissions = Vec::with_capacity(10);

    // --- First-frame init: seed prev_heading so we don't emit on frame 0. ---
    if !state.initialized {
        state.prev_heading = car.heading;
        state.initialized = true;
    }

    // Angular velocity (radians/sec). Wrapped into a sane range by taking the
    // shortest signed delta so a heading wrap (e.g. 2π → 0) doesn't produce a
    // phantom huge angular velocity.
    let dh = shortest_heading_delta(car.heading - state.prev_heading);
    let ang_vel = dh / dt.max(1e-4);
    state.prev_heading = car.heading;

    let skidding = ang_vel.abs() > ANG_VEL_THRESHOLD && car.speed.abs() > SPEED_THRESHOLD;

    // --- Rear-wheel world positions ---
    // Local offsets rotated by the car's heading (yaw about Y), then offset
    // by the car's translation. Y is the ground-contact height.
    let (left_w, right_w) = rear_wheel_world(car_t, REAR_WHEEL_X, REAR_WHEEL_Z);

    // Forward direction along travel (for orienting marks + driving smoke
    // drift). Matches `car.rs` forward = (-sin h, 0, -cos h).
    let fwd = Vec3::new(-car.heading.sin(), 0.0, -car.heading.cos());

    // Gather this frame's requests before touching either pool. Processing a
    // batch lets each existing entity be claimed at most once and lets pending
    // Commands spawns count against the cap immediately.
    state.mark_timer -= dt;
    if skidding && state.mark_timer <= 0.0 {
        state.mark_timer = MARK_EMIT_INTERVAL;
        for &pos in &[left_w, right_w] {
            mark_emissions.push(MarkEmission {
                pos,
                fwd,
                heading: car.heading,
            });
        }
    }

    state.smoke_timer -= dt;
    if skidding && state.smoke_timer <= 0.0 {
        state.smoke_timer = SMOKE_EMIT_INTERVAL;
        for &pos in &[left_w, right_w] {
            for _ in 0..2 {
                if smoke_emissions.len() < SMOKE_POOL_CAP {
                    smoke_emissions.push(SmokeEmission { pos, fwd });
                }
            }
        }
    }

    // Impact smoke is independent of skidding. Requests beyond the complete
    // pool are immaterial (there cannot be more than the cap visible at once),
    // so do not accumulate an unbounded message burst.
    for hit in obstacle_hits.read() {
        if hit.impact_speed >= HIT_SMOKE_SPEED {
            let center = car_t.translation + fwd * 0.9;
            for _ in 0..6 {
                if smoke_emissions.len() >= SMOKE_POOL_CAP {
                    break;
                }
                smoke_emissions.push(SmokeEmission {
                    pos: Vec3::new(
                        center.x + (rand_local() - 0.5) * 0.6,
                        MARK_Y + 0.1,
                        center.z + (rand_local() - 0.5) * 0.6,
                    ),
                    fwd,
                });
            }
        }
    }

    apply_mark_emissions(&mut commands, &assets, &mut marks, &mark_emissions);
    apply_smoke_emissions(&mut commands, &assets, &mut smokes, &smoke_emissions);
}

/// Compute the world-space ground-contact points of the two rear wheels from
/// the car's transform + local wheel offsets. Returns (left, right) where
/// left = +X offset, right = -X offset (in car-local space).
fn rear_wheel_world(car_t: &Transform, half_x: f32, rear_z: f32) -> (Vec3, Vec3) {
    // Car-local point → world: rotate by the car's rotation (yaw about Y),
    // then translate. The car has no scale, so this is exact.
    let rot = car_t.rotation;
    let local_left = Vec3::new(half_x, MARK_Y, rear_z);
    let local_right = Vec3::new(-half_x, MARK_Y, rear_z);
    let left = car_t.translation + rot * local_left;
    let right = car_t.translation + rot * local_right;
    (left, right)
}

#[derive(Clone, Copy)]
struct MarkEmission {
    pos: Vec3,
    fwd: Vec3,
    heading: f32,
}

#[derive(Clone, Copy)]
struct SmokeEmission {
    pos: Vec3,
    fwd: Vec3,
}

/// Apply a mark batch deterministically. Hidden/expired entities are consumed
/// first. If none exist, free pool capacity is used; only a full pool causes
/// the oldest active marks to be recycled. Each entity is claimed once, so a
/// two-wheel emission cannot repeatedly teleport the same fresh mark.
fn apply_mark_emissions(
    commands: &mut Commands,
    assets: &EffectsAssets,
    marks: &mut Query<
        (
            Entity,
            &mut TireMark,
            &mut Transform,
            &mut Visibility,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        (With<TireMark>, Without<Smoke>),
    >,
    emissions: &[MarkEmission],
) {
    let mut entities = Vec::new();
    let mut slots = Vec::new();
    for (entity, mark, _tf, visibility, _mat) in marks.iter_mut() {
        entities.push(entity);
        slots.push(PoolSlot::new(
            mark.age,
            *visibility == Visibility::Hidden || mark.age >= MARK_LIFETIME,
            entity.index().index(),
        ));
    }

    let existing = entities.len();
    let mut pending_spawns = 0;
    for emission in emissions {
        let rot = Quat::from_rotation_y(emission.heading);
        let pos = emission.pos - emission.fwd * (MARK_LEN * 0.5);
        let transform = Transform::from_translation(pos).with_rotation(rot);

        let reuse = take_pool_slot(&mut slots, true).or_else(|| {
            if pool_has_spawn_capacity(existing, pending_spawns, MARK_POOL_CAP) {
                None
            } else {
                take_pool_slot(&mut slots, false)
            }
        });

        if let Some(index) = reuse {
            if let Ok((_entity, mut mark, mut tf, mut visibility, mut mat)) =
                marks.get_mut(entities[index])
            {
                mark.age = 0.0;
                *tf = transform;
                *visibility = Visibility::Visible;
                mat.0 = assets.mark_materials[0].clone();
            }
        } else if pool_has_spawn_capacity(existing, pending_spawns, MARK_POOL_CAP) {
            commands.spawn((
                Mesh3d(assets.mark_mesh.clone()),
                MeshMaterial3d(assets.mark_materials[0].clone()),
                transform,
                Visibility::Visible,
                TireMark { age: 0.0 },
            ));
            pending_spawns += 1;
        }
        // If every existing slot was already claimed and no capacity remains,
        // the batch already represents a complete pool; discard the excess.
    }
}

/// Smoke uses the same allocation policy as marks and is kept as a stable
/// hidden pool after expiry rather than despawned and recreated every burst.
fn apply_smoke_emissions(
    commands: &mut Commands,
    assets: &EffectsAssets,
    smokes: &mut Query<
        (
            Entity,
            &mut Smoke,
            &mut Transform,
            &mut Visibility,
            &mut MeshMaterial3d<StandardMaterial>,
        ),
        (With<Smoke>, Without<TireMark>),
    >,
    emissions: &[SmokeEmission],
) {
    let mut entities = Vec::new();
    let mut slots = Vec::new();
    for (entity, smoke, _tf, visibility, _mat) in smokes.iter_mut() {
        entities.push(entity);
        slots.push(PoolSlot::new(
            smoke.age,
            *visibility == Visibility::Hidden || smoke.age >= SMOKE_LIFETIME,
            entity.index().index(),
        ));
    }

    let existing = entities.len();
    let mut pending_spawns = 0;
    for emission in emissions {
        let seed = rand_local();
        let start_pos = emission.pos - emission.fwd * 0.1 + Vec3::new(0.0, 0.1, 0.0);
        let scale = 0.6 + seed * 0.4;
        let transform = Transform::from_translation(start_pos).with_scale(Vec3::splat(scale));

        let reuse = take_pool_slot(&mut slots, true).or_else(|| {
            if pool_has_spawn_capacity(existing, pending_spawns, SMOKE_POOL_CAP) {
                None
            } else {
                take_pool_slot(&mut slots, false)
            }
        });

        if let Some(index) = reuse {
            if let Ok((_entity, mut smoke, mut tf, mut visibility, mut mat)) =
                smokes.get_mut(entities[index])
            {
                smoke.age = 0.0;
                smoke.seed = seed;
                *tf = transform;
                *visibility = Visibility::Visible;
                mat.0 = assets.smoke_materials[0].clone();
            }
        } else if pool_has_spawn_capacity(existing, pending_spawns, SMOKE_POOL_CAP) {
            commands.spawn((
                Mesh3d(assets.smoke_mesh.clone()),
                MeshMaterial3d(assets.smoke_materials[0].clone()),
                transform,
                Visibility::Visible,
                Smoke { age: 0.0, seed },
            ));
            pending_spawns += 1;
        }
    }
}

// ===========================================================================
// Fade + recycle — advance age, fade alpha (material swap), hide when expired
// ===========================================================================

/// Advance each tire mark's age and fade its alpha by swapping the material
/// handle to the bucket matching its age fraction. At end of life, hide the
/// entity (it stays in the pool for recycling by `emit_tire_effects`).
fn fade_tire_marks(
    time: Res<Time>,
    assets: Res<EffectsAssets>,
    mut marks: Query<(
        &mut TireMark,
        &mut Transform,
        &mut Visibility,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
) {
    let dt = time.delta_secs();
    let last = MARK_FADE_STEPS - 1;
    for (mut mark, mut tf, mut vis, mut mat) in marks.iter_mut() {
        mark.age += dt;
        if mark.age >= MARK_LIFETIME {
            // Expired: hide it. Keep the entity for recycling (the emitter
            // re-shows + repositions when it picks this slot). Hiding is
            // cheaper than despawn+respawn and keeps the pool stable.
            *vis = Visibility::Hidden;
            continue;
        }
        // Make sure a recycled (previously hidden) mark is visible again.
        *vis = Visibility::Visible;
        // Alpha bucket: 0 at fresh → last at end-of-life.
        let t = mark.age / MARK_LIFETIME;
        let bucket = ((t * MARK_FADE_STEPS as f32) as usize).min(last);
        let want = assets.mark_materials[bucket].clone();
        // Only swap the handle when the bucket changes (≤ MARK_FADE_STEPS
        // swaps per mark over its whole life — trivial churn).
        if mat.0 != want {
            mat.0 = want;
        }
        // Reset any scale a previous lifecycle may have left (recycled marks
        // inherit the old transform via `insert`, but we overwrite Transform
        // on recycle, so scale is already 1; keep it 1 here for safety).
        if tf.scale != Vec3::ONE {
            tf.scale = Vec3::ONE;
        }
    }
}

/// Advance each smoke puff: rise (+Y), expand (scale up), and fade alpha by
/// material swap. Expired puffs are hidden but retained as reusable pool slots.
fn update_smoke(
    time: Res<Time>,
    assets: Res<EffectsAssets>,
    mut smokes: Query<(
        &mut Smoke,
        &mut Transform,
        &mut Visibility,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
) {
    let dt = time.delta_secs();
    let last = SMOKE_FADE_STEPS - 1;
    for (mut smoke, mut tf, mut visibility, mut mat) in smokes.iter_mut() {
        smoke.age += dt;
        if smoke.age >= SMOKE_LIFETIME {
            *visibility = Visibility::Hidden;
            continue;
        }
        *visibility = Visibility::Visible;
        let t = smoke.age / SMOKE_LIFETIME;
        // Alpha bucket for fade.
        let bucket = ((t * SMOKE_FADE_STEPS as f32) as usize).min(last);
        let want = assets.smoke_materials[bucket].clone();
        if mat.0 != want {
            mat.0 = want;
        }
        // Rise: up to ~0.5u over the puff's life, varied by seed.
        let rise = (0.15 + smoke.seed * 0.35) * t;
        // Expand: scale from ~0.6 → ~1.6 over the life (+ seed jitter).
        let scale = 0.6 + t * 1.0 + smoke.seed * 0.2;
        // Slight sideways drift, seeded.
        let drift = (smoke.seed - 0.5) * 0.3 * t;
        tf.translation.y = (tf.translation.y + rise * dt * 4.0).min(2.0);
        tf.translation.x += drift * dt;
        tf.scale = Vec3::splat(scale);
    }
}

// ===========================================================================
// Cleanup — purge all marks/smoke when a round ends
// ===========================================================================

/// Despawn every tire mark and smoke puff on entry to GameOver or Menu.
/// Playing -> Paused intentionally does not call this, so pause/resume retains
/// the stable pools. Recursive despawn in 0.19 is safe here (leaf entities).
fn cleanup_effects(
    mut commands: Commands,
    marks: Query<Entity, With<TireMark>>,
    smokes: Query<Entity, With<Smoke>>,
) {
    for entity in &marks {
        commands.entity(entity).despawn();
    }
    for entity in &smokes {
        commands.entity(entity).despawn();
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Read-only allocation metadata used to make pool selection independent of
/// ECS iteration/mutation. Entity indices provide a stable equal-age tie-break.
#[derive(Clone, Copy, Debug)]
struct PoolSlot {
    age: f32,
    reusable: bool,
    tie_breaker: u32,
    available: bool,
}

impl PoolSlot {
    fn new(age: f32, reusable: bool, tie_breaker: u32) -> Self {
        Self {
            age,
            reusable,
            tie_breaker,
            available: true,
        }
    }
}

/// Claim the best slot: reusable entries before active entries, then greatest
/// age (oldest) within that class. With `reusable_only`, active effects are not
/// considered while the caller still has room to grow the pool.
fn take_pool_slot(slots: &mut [PoolSlot], reusable_only: bool) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (index, slot) in slots.iter().enumerate() {
        if !slot.available || (reusable_only && !slot.reusable) {
            continue;
        }
        let replace = match best {
            None => true,
            Some(best_index) => {
                let current = &slots[best_index];
                (slot.reusable && !current.reusable)
                    || (slot.reusable == current.reusable
                        && (slot.age.total_cmp(&current.age).is_gt()
                            || (slot.age.total_cmp(&current.age).is_eq()
                                && slot.tie_breaker < current.tie_breaker)))
            }
        };
        if replace {
            best = Some(index);
        }
    }
    if let Some(index) = best {
        slots[index].available = false;
    }
    best
}

/// Include deferred spawns in capacity checks so several emissions in one
/// system invocation can never queue enough Commands to overrun a hard cap.
fn pool_has_spawn_capacity(existing: usize, pending_spawns: usize, cap: usize) -> bool {
    existing.saturating_add(pending_spawns) < cap
}

#[cfg(test)]
mod tests {
    use super::{PoolSlot, pool_has_spawn_capacity, take_pool_slot};

    #[test]
    fn reuse_prefers_expired_then_oldest_with_stable_ties() {
        let mut slots = [
            PoolSlot::new(9.0, false, 1),
            PoolSlot::new(4.0, true, 8),
            PoolSlot::new(4.0, true, 3),
            PoolSlot::new(8.0, false, 2),
        ];

        // Reusable beats even an older active slot; equal ages use the stable
        // entity-index tie-break. Claimed slots cannot be selected twice.
        assert_eq!(take_pool_slot(&mut slots, false), Some(2));
        assert_eq!(take_pool_slot(&mut slots, false), Some(1));
        assert_eq!(take_pool_slot(&mut slots, false), Some(0));
        assert_eq!(take_pool_slot(&mut slots, false), Some(3));
        assert_eq!(take_pool_slot(&mut slots, false), None);
    }

    #[test]
    fn reusable_only_does_not_teleport_an_active_effect() {
        let mut slots = [PoolSlot::new(2.0, false, 1)];
        assert_eq!(take_pool_slot(&mut slots, true), None);
        assert_eq!(take_pool_slot(&mut slots, false), Some(0));
    }

    #[test]
    fn pending_spawns_respect_the_hard_cap() {
        assert!(pool_has_spawn_capacity(78, 1, 80));
        assert!(!pool_has_spawn_capacity(78, 2, 80));
        assert!(!pool_has_spawn_capacity(80, 0, 80));
        assert!(!pool_has_spawn_capacity(usize::MAX, 1, 80));
    }
}

/// Shortest signed delta in `(-π, π]` for a raw heading difference (handles
/// the 2π wrap so heading 0 → 2π doesn't read as a huge angular velocity).
fn shortest_heading_delta(raw: f32) -> f32 {
    use std::f32::consts::TAU;
    let mut d = raw % TAU;
    if d > std::f32::consts::PI {
        d -= TAU;
    } else if d < -std::f32::consts::PI {
        d += TAU;
    }
    d
}

/// Tiny per-call pseudo-random 0..1 without pulling in `rand` (matches the
/// `world.rs` LCG style). Uses a static atomic counter as the seed source.
fn rand_local() -> f32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEED: AtomicU32 = AtomicU32::new(0x1234_5678u32);
    let s = SEED.fetch_add(1664525, Ordering::Relaxed);
    (s.wrapping_mul(1664525).wrapping_add(1013904223)) as f32 / (u32::MAX as f32)
}
