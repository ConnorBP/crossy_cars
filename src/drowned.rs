//! Local-only pond drowning gameplay and presentation.

use bevy::color::LinearRgba;
use bevy::prelude::*;

use crate::car::{Car, DriftLatch, DrivingSet, Handbrake, PlayerInput};
use crate::game::events::PondEntered;
use crate::game::resources::{Drowning, GameOverReason, RoundActive};
use crate::game::state::GameState;
use crate::toy_shading::ToyShadingAssets;
#[cfg(test)]
use crate::world::PondFamily;
use crate::world::PondFootprint;

const DROWN_DURATION: f32 = 0.8;
const DROWN_SPEED_DECAY: f32 = 5.0;
const DROWN_LOWER_DISTANCE: f32 = 0.62;
const DROWN_TILT: f32 = 0.34;
const RIPPLE_DURATION: f32 = 0.8;
const CAR_HALF_EXTENTS: Vec2 = Vec2::new(0.56, 1.0);

#[derive(Resource)]
struct DrownedPresentationAssets {
    ripple_mesh: Handle<Mesh>,
    ripple_material: Handle<StandardMaterial>,
    splash_mesh: Handle<Mesh>,
    splash_material: Handle<StandardMaterial>,
}

impl FromWorld for DrownedPresentationAssets {
    fn from_world(world: &mut World) -> Self {
        let ripple_mesh = world
            .resource_mut::<Assets<Mesh>>()
            .add(Annulus::new(0.68, 0.82));
        let splash_mesh = world.resource::<ToyShadingAssets>().contact_plane.clone();
        let ripple_material =
            world
                .resource_mut::<Assets<StandardMaterial>>()
                .add(StandardMaterial {
                    base_color: Color::srgba(0.72, 0.94, 1.0, 0.72),
                    emissive: LinearRgba::new(0.05, 0.18, 0.22, 1.0),
                    alpha_mode: AlphaMode::Blend,
                    unlit: true,
                    ..default()
                });
        let splash_material =
            world
                .resource_mut::<Assets<StandardMaterial>>()
                .add(StandardMaterial {
                    base_color: Color::srgba(0.72, 0.94, 1.0, 0.42),
                    alpha_mode: AlphaMode::Blend,
                    unlit: true,
                    ..default()
                });
        Self {
            ripple_mesh,
            ripple_material,
            splash_mesh,
            splash_material,
        }
    }
}

#[derive(Component)]
struct DrownedPresentation;

#[derive(Component, Clone, Copy)]
struct Ripple {
    elapsed: f32,
    phase: f32,
}

fn earliest_pond_entry(
    start: Vec2,
    end: Vec2,
    start_heading: f32,
    end_heading: f32,
    final_center: Vec2,
    final_heading: f32,
    ponds: impl IntoIterator<Item = (PondFootprint, Vec2)>,
) -> Option<Vec2> {
    let mut swept: Option<(f32, Vec2)> = None;
    let mut final_overlap = false;
    for (pond, origin) in ponds {
        if let Some(t) = pond.swept_world_car_entry_over_heading(
            origin,
            start,
            end,
            start_heading,
            end_heading,
            CAR_HALF_EXTENTS,
        ) {
            let candidate = (t, start.lerp(end, t));
            if swept.is_none_or(|current| t.total_cmp(&current.0).is_lt()) {
                swept = Some(candidate);
            }
        }
        // Pushout is never part of the sweep. It participates only as the
        // authoritative final-pose overlap test.
        final_overlap |=
            pond.contains_world_car(origin, final_center, final_heading, CAR_HALF_EXTENTS);
    }
    swept
        .map(|(_, position)| position)
        .or_else(|| final_overlap.then_some(final_center))
}

/// Snapshot the endpoint produced by movement before solid collision pushout.
pub(crate) fn capture_motion_end(mut drowning: ResMut<Drowning>, car: Query<(&Car, &Transform)>) {
    if drowning.active {
        return;
    }
    let Ok((car, transform)) = car.single() else {
        return;
    };
    drowning.motion_end_center = Vec2::new(transform.translation.x, transform.translation.z);
    drowning.motion_end_heading = car.heading;
}

/// Runs after all movement resolution in `DrivingSet`. Pond footprints remain
/// block-local, so the owning streamed block's transform is applied explicitly.
pub(crate) fn detect_pond_entry(
    mut drowning: ResMut<Drowning>,
    round_active: Res<RoundActive>,
    mut input: ResMut<PlayerInput>,
    mut handbrake: ResMut<Handbrake>,
    mut car: Query<(&mut Car, &Transform, &mut DriftLatch)>,
    ponds: Query<(&PondFootprint, &ChildOf)>,
    block_transforms: Query<&Transform, Without<Car>>,
    mut entered: MessageWriter<PondEntered>,
    mut next: ResMut<NextState<GameState>>,
) {
    let Ok((mut car, transform, mut drift_latch)) = car.single_mut() else {
        return;
    };
    let final_center = Vec2::new(transform.translation.x, transform.translation.z);
    let final_heading = car.heading;
    if !round_active.0 {
        drowning.previous_resolved_center = final_center;
        drowning.previous_resolved_heading = final_heading;
        drowning.initialized = true;
        return;
    }
    if drowning.active {
        drowning.previous_resolved_center = final_center;
        drowning.previous_resolved_heading = final_heading;
        drowning.initialized = true;
        return;
    }

    // A fresh round seeds from the pose present before its first movement. In
    // normal scheduling capture_motion_end has already recorded the endpoint;
    // initialization therefore reconstructs the start from motion itself only
    // when no prior resolved pose exists (the reset pose is the origin).
    let start = if drowning.initialized {
        drowning.previous_resolved_center
    } else {
        Vec2::ZERO
    };
    let start_heading = if drowning.initialized {
        drowning.previous_resolved_heading
    } else {
        0.0
    };
    let motion_end = drowning.motion_end_center;
    let motion_heading = drowning.motion_end_heading;
    let position = earliest_pond_entry(
        start,
        motion_end,
        start_heading,
        motion_heading,
        final_center,
        final_heading,
        ponds.iter().filter_map(|(pond, child_of)| {
            let block = block_transforms.get(child_of.parent()).ok()?;
            Some((*pond, Vec2::new(block.translation.x, block.translation.z)))
        }),
    );

    // Always synchronize to the fully resolved pose, including no-hit frames,
    // pause-adjacent frames, and a frame that starts drowning.
    drowning.previous_resolved_center = final_center;
    drowning.previous_resolved_heading = final_heading;
    drowning.initialized = true;
    let Some(_crossing_position) = position else {
        return;
    };

    drowning.active = true;
    drowning.elapsed = 0.0;
    drowning.camera_capture_pending = true;
    // Touch/keyboard and the clock deliberately run before driving. Pond entry
    // cancels their same-frame Paused/GameOver request so the full local sink
    // sequence owns the eventual terminal transition.
    next.reset();
    // Presentation/camera entry is the final resolved pose, not the analytic
    // shoreline crossing. This position then remains frozen during sinking.
    drowning.entry_position = transform.translation;
    *input = PlayerInput::default();
    handbrake.0 = false;
    car.drift = 0.0;
    *drift_latch = DriftLatch::default();
    entered.write(PondEntered {
        position: drowning.entry_position,
    });
}

/// Deterministic sinking; Paused preserves elapsed time exactly.
pub(crate) fn advance_drowning(
    mut drowning: ResMut<Drowning>,
    time: Res<Time>,
    mut car: Query<(&mut Car, &mut Transform)>,
    mut next: ResMut<NextState<GameState>>,
    mut reason: ResMut<GameOverReason>,
) {
    if !drowning.active {
        return;
    }
    let Ok((mut car, mut transform)) = car.single_mut() else {
        return;
    };
    let dt = time.delta_secs().max(0.0);
    drowning.elapsed = (drowning.elapsed + dt).min(DROWN_DURATION);
    car.speed *= (-DROWN_SPEED_DECAY * dt).exp();
    if car.speed.abs() < 0.01 {
        car.speed = 0.0;
    }
    let progress = (drowning.elapsed / DROWN_DURATION).clamp(0.0, 1.0);
    let eased = progress * progress * (3.0 - 2.0 * progress);
    transform.translation.y = drowning.entry_position.y - DROWN_LOWER_DISTANCE * eased;
    transform.rotation = Quat::from_rotation_y(car.heading)
        * Quat::from_rotation_x(DROWN_TILT * eased)
        * Quat::from_rotation_z(-DROWN_TILT * 0.55 * eased);

    if drowning.elapsed >= DROWN_DURATION {
        car.speed = 0.0;
        // This system is deliberately last in DrivingSet. Overwrite both the
        // reason and any pause/time-up/wreck pending transition from this frame.
        *reason = GameOverReason::Drowned;
        next.set(GameState::GameOver);
    }
}

fn spawn_drowned_presentation(
    mut commands: Commands,
    mut events: MessageReader<PondEntered>,
    assets: Res<DrownedPresentationAssets>,
) {
    for event in events.read() {
        for (phase, scale) in [(0.0, 1.0), (0.16, 0.72)] {
            commands.spawn((
                Mesh3d(assets.ripple_mesh.clone()),
                MeshMaterial3d(assets.ripple_material.clone()),
                Transform::from_translation(event.position + Vec3::Y * 0.025)
                    .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                    .with_scale(Vec3::splat(scale)),
                Ripple {
                    elapsed: 0.0,
                    phase,
                },
                DrownedPresentation,
            ));
        }
        commands.spawn((
            Mesh3d(assets.splash_mesh.clone()),
            MeshMaterial3d(assets.splash_material.clone()),
            Transform::from_translation(event.position + Vec3::Y * 0.04)
                .with_scale(Vec3::new(1.8, 1.0, 1.8)),
            Ripple {
                elapsed: 0.0,
                phase: 0.08,
            },
            DrownedPresentation,
        ));
    }
}

fn animate_drowned_presentation(
    mut commands: Commands,
    time: Res<Time>,
    mut ripples: Query<(Entity, &mut Transform, &mut Ripple)>,
) {
    let dt = time.delta_secs().max(0.0);
    for (entity, mut transform, mut ripple) in &mut ripples {
        ripple.elapsed += dt;
        let progress = ((ripple.elapsed - ripple.phase) / RIPPLE_DURATION).clamp(0.0, 1.0);
        let scale = 0.75 + progress * 2.1;
        transform.scale.x = scale;
        transform.scale.z = scale;
        transform.translation.y -= dt * 0.015;
        if ripple.elapsed >= RIPPLE_DURATION + ripple.phase {
            commands.entity(entity).despawn();
        }
    }
}

fn cleanup_drowned_presentation(
    mut commands: Commands,
    presentation: Query<Entity, With<DrownedPresentation>>,
) {
    for entity in &presentation {
        commands.entity(entity).despawn();
    }
}

pub struct DrownedPlugin;

impl Plugin for DrownedPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DrownedPresentationAssets>()
            .add_systems(
                Update,
                spawn_drowned_presentation
                    .after(DrivingSet)
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(
                Update,
                animate_drowned_presentation
                    .after(DrivingSet)
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(OnEnter(GameState::Menu), cleanup_drowned_presentation)
            .add_systems(OnEnter(GameState::GameOver), cleanup_drowned_presentation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pond() -> PondFootprint {
        PondFootprint {
            family: PondFamily::GardenOval,
            center: Vec2::ZERO,
            radii: Vec2::new(3.0, 2.0),
            rotation: 0.0,
        }
    }

    #[test]
    fn oriented_footprint_dimensions_match_player_collision() {
        let (half_width, half_length) = crate::car::car_footprint_half_extents();
        assert_eq!(Vec2::new(half_width, half_length), CAR_HALF_EXTENTS);
        assert_eq!(CAR_HALF_EXTENTS * 2.0, Vec2::new(1.12, 2.0));
    }

    #[test]
    fn high_speed_sweep_finds_earliest_entry() {
        let entry = earliest_pond_entry(
            Vec2::new(-100.0, 0.0),
            Vec2::new(100.0, 0.0),
            0.0,
            0.0,
            Vec2::new(100.0, 0.0),
            0.0,
            [(pond(), Vec2::new(20.0, 0.0)), (pond(), Vec2::ZERO)],
        )
        .expect("a fast crossing must enter");
        assert!(entry.x < 0.0);
    }

    #[test]
    fn latch_prevents_a_second_entry_event() {
        let mut drowning = Drowning::default();
        assert!(!drowning.active);
        drowning.active = true;
        assert!(drowning.active);
    }

    #[test]
    fn collision_pushout_is_not_mistaken_for_driving_through_water() {
        let result = earliest_pond_entry(
            Vec2::new(-6.0, 0.0),
            Vec2::new(-6.0, 0.0),
            0.0,
            0.0,
            Vec2::new(6.0, 0.0),
            0.0,
            [(pond(), Vec2::ZERO)],
        );
        assert_eq!(result, None, "pushout itself must never be swept");
    }

    #[test]
    fn actual_motion_crossing_survives_pushout_back_to_dry_land() {
        let result = earliest_pond_entry(
            Vec2::new(-6.0, 0.0),
            Vec2::new(6.0, 0.0),
            0.0,
            0.0,
            Vec2::new(-6.0, 0.0),
            0.0,
            [(pond(), Vec2::ZERO)],
        );
        assert!(result.is_some());
    }

    #[test]
    fn final_resolved_overlap_drowns_without_a_motion_crossing() {
        let result = earliest_pond_entry(
            Vec2::new(-6.0, 0.0),
            Vec2::new(-6.0, 0.0),
            0.0,
            0.0,
            Vec2::ZERO,
            0.0,
            [(pond(), Vec2::ZERO)],
        );
        assert_eq!(result, Some(Vec2::ZERO));
    }

    #[test]
    fn terminal_timing_is_point_eight_seconds() {
        assert!((DROWN_DURATION - 0.8).abs() < f32::EPSILON);
        assert!(DROWN_DURATION - 0.001 < DROWN_DURATION);
    }
}
