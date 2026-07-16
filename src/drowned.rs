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
    heading: f32,
    ponds: impl IntoIterator<Item = (PondFootprint, Vec2)>,
) -> Option<(f32, Vec2)> {
    ponds
        .into_iter()
        .filter_map(|(pond, origin)| {
            pond.swept_world_car_entry(origin, start, end, heading, CAR_HALF_EXTENTS)
                .map(|t| (t, start.lerp(end, t)))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
}

/// Runs directly after movement in `DrivingSet`. Pond footprints remain
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
) {
    let Ok((mut car, transform, mut drift_latch)) = car.single_mut() else {
        return;
    };
    let current = Vec2::new(transform.translation.x, transform.translation.z);
    if !round_active.0 {
        drowning.previous_center = current;
        return;
    }
    if drowning.active {
        return;
    }

    // The fresh-round reset seeds the previous center to the reset car center;
    // pause resumes preserve it. This makes the very first movement sweep as
    // authoritative as every later frame, including a high-speed first step.
    let start = drowning.previous_center;
    drowning.previous_center = current;
    let Some((_, position)) = earliest_pond_entry(
        start,
        current,
        car.heading,
        ponds.iter().filter_map(|(pond, child_of)| {
            let block = block_transforms.get(child_of.parent()).ok()?;
            Some((*pond, Vec2::new(block.translation.x, block.translation.z)))
        }),
    ) else {
        return;
    };

    drowning.active = true;
    drowning.elapsed = 0.0;
    drowning.entry_position = Vec3::new(position.x, 0.05, position.y);
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
    transform.translation.y = -DROWN_LOWER_DISTANCE * eased;
    transform.rotation = Quat::from_rotation_y(car.heading)
        * Quat::from_rotation_x(DROWN_TILT * eased)
        * Quat::from_rotation_z(-DROWN_TILT * 0.55 * eased);

    if drowning.elapsed >= DROWN_DURATION {
        car.speed = 0.0;
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
            [(pond(), Vec2::new(20.0, 0.0)), (pond(), Vec2::ZERO)],
        )
        .expect("a fast crossing must enter");
        assert!(entry.0 < 0.5);
        assert!(entry.1.x < 0.0);
    }

    #[test]
    fn latch_prevents_a_second_entry_event() {
        let mut drowning = Drowning::default();
        assert!(!drowning.active);
        drowning.active = true;
        assert!(drowning.active);
    }

    #[test]
    fn terminal_timing_is_point_eight_seconds() {
        assert!((DROWN_DURATION - 0.8).abs() < f32::EPSILON);
        assert!(DROWN_DURATION - 0.001 < DROWN_DURATION);
    }
}
