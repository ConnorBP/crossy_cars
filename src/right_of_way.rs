//! Playable rules-v3 Right of Way conduct.
//!
//! All score arithmetic is delegated to `roady_score_rules::v3::RightOfWay`.
//! This module owns only world interactions, deterministic target placement,
//! canonical-event projection, and presentation messages.

use bevy::color::LinearRgba;
use bevy::prelude::*;
use roady_score_rules::v3::{self, canonical};

use crate::car::{Car, DrivingSet, InputFrozen};
use crate::chickens::Chicken;
use crate::critters::CritterHit;
use crate::game::events::ChickenHit;
use crate::game::resources::{Drowning, RoundActive, TimeLeft};
use crate::game::state::GameState;
use crate::game::{SpawnSet, TerminalFinalizeSet};
use crate::game_modes::{ActivePlayClock, ActiveRunRules, Conduct};
use crate::ledger::{CanonicalEventQueue, PendingCanonicalEvent};
use crate::objectives::ObjectiveFinalizeSet;
use crate::toy_shading::{ToyMaterialFamily, toy_material};

const INTERACTION_RADIUS: f32 = 1.35;
const PACKAGE_Y: f32 = 0.65;

/// Score-bearing contacts are gathered during `Update` and committed together
/// in `PostUpdate`.  This makes simultaneous package/courtesy/animal/wave/coin
/// contacts follow the canonical gameplay-kind order rather than Bevy's
/// otherwise unspecified cross-plugin system order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingRightOfWayAction {
    Delivery {
        active_ms: u64,
        target_index: u32,
    },
    Courtesy {
        active_ms: u64,
        chicken_id: u32,
    },
    Animal {
        active_ms: u64,
        animal_kind: u8,
        ordinal: u32,
    },
    Wave {
        active_ms: u64,
        wave_index: u32,
    },
    Coin {
        active_ms: u64,
        stable_id: u64,
    },
}

impl PendingRightOfWayAction {
    const fn sort_key(self) -> (u64, u8, u64) {
        match self {
            Self::Delivery {
                active_ms,
                target_index,
            } => (active_ms, 9, target_index as u64),
            Self::Courtesy {
                active_ms,
                chicken_id,
            } => (active_ms, 10, chicken_id as u64),
            Self::Animal {
                active_ms,
                animal_kind,
                ordinal,
            } => (active_ms, 11, ((animal_kind as u64) << 32) | ordinal as u64),
            Self::Wave {
                active_ms,
                wave_index,
            } => (active_ms, 12, wave_index as u64),
            Self::Coin {
                active_ms,
                stable_id,
            } => (active_ms, 13, stable_id),
        }
    }
}

#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingRightOfWayActions(pub Vec<PendingRightOfWayAction>);

impl PendingRightOfWayActions {
    pub fn coin(&mut self, active_ms: u64, stable_id: u64) {
        self.0.push(PendingRightOfWayAction::Coin {
            active_ms,
            stable_id,
        });
    }

    pub fn wave(&mut self, active_ms: u64, wave_index: u32) {
        self.0.push(PendingRightOfWayAction::Wave {
            active_ms,
            wave_index,
        });
    }
}

/// Systems producing chicken/critter/coin contacts run before this set. It is
/// the single collision-order seam used by Right of Way interactions.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RightOfWayInteractionSet;

#[derive(Resource, Clone, Debug)]
pub struct RightOfWayRun {
    pub score: v3::RightOfWay,
    pub courtesy_gate: v3::CourtesyGate,
    pub failed: bool,
    last_active_ms: u64,
    next_target: u32,
    next_chicken_id: u32,
}

impl Default for RightOfWayRun {
    fn default() -> Self {
        Self {
            score: v3::RightOfWay::with_remaining(60_000),
            courtesy_gate: default(),
            failed: false,
            last_active_ms: 0,
            next_target: 0,
            next_chicken_id: 0,
        }
    }
}

impl RightOfWayRun {
    fn reject(&mut self) {
        self.failed = true;
    }
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct StableChickenId(pub u32);

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackagePickup {
    pub target_index: u32,
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeliveryTarget {
    pub target_index: u32,
}

#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackagePickedUp;
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackageDelivered(pub u8);
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct CourtesyAwarded;

#[derive(Resource)]
struct RightOfWayAssets {
    package_mesh: Handle<Mesh>,
    delivery_mesh: Handle<Mesh>,
    package_material: Handle<StandardMaterial>,
    delivery_material: Handle<StandardMaterial>,
}

impl FromWorld for RightOfWayAssets {
    fn from_world(world: &mut World) -> Self {
        world.resource_scope::<Assets<Mesh>, _>(|world, mut meshes| {
            // Deliberately chunky silhouette and a broad emissive ring remain
            // readable against the production microtextured/PBR world.
            let package_mesh = meshes.add(Cuboid::new(1.0, 0.72, 0.82));
            let delivery_mesh = meshes.add(Torus::new(1.35, 1.62));
            let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
            let package_material = materials.add(toy_material(
                ToyMaterialFamily::RawWood,
                StandardMaterial {
                    base_color: Color::srgb(0.92, 0.52, 0.12),
                    ..default()
                },
            ));
            let delivery_material = materials.add(toy_material(
                ToyMaterialFamily::Ceramic,
                StandardMaterial {
                    base_color: Color::srgb(0.15, 0.95, 0.45),
                    emissive: LinearRgba::rgb(0.15, 1.4, 0.35),
                    ..default()
                },
            ));
            Self {
                package_mesh,
                delivery_mesh,
                package_material,
                delivery_material,
            }
        })
    }
}

pub struct RightOfWayPlugin;

impl Plugin for RightOfWayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RightOfWayRun>()
            .init_resource::<PendingRightOfWayActions>()
            .init_resource::<RightOfWayAssets>()
            .add_message::<PackagePickedUp>()
            .add_message::<PackageDelivered>()
            .add_message::<CourtesyAwarded>()
            .configure_sets(Update, RightOfWayInteractionSet.after(DrivingSet))
            .add_systems(
                OnEnter(GameState::Playing),
                setup_fresh_run.in_set(SpawnSet),
            )
            .add_systems(
                Update,
                (
                    assign_stable_chicken_ids,
                    tick_conduct_clock,
                    collect_packages,
                    observe_courtesy,
                )
                    .chain()
                    .in_set(RightOfWayInteractionSet)
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(
                PostUpdate,
                (
                    queue_animal_hit_messages,
                    apply_pending_actions,
                    reject_failed_run,
                )
                    .chain()
                    .before(ObjectiveFinalizeSet)
                    .before(TerminalFinalizeSet)
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(OnEnter(GameState::Menu), cleanup_targets)
            .add_systems(OnEnter(GameState::GameOver), cleanup_targets);
    }
}

fn is_right_of_way(rules: &ActiveRunRules) -> bool {
    rules.conduct == Conduct::RightOfWay
}

fn setup_fresh_run(
    mut commands: Commands,
    assets: Res<RightOfWayAssets>,
    rules: Res<ActiveRunRules>,
    round: Res<RoundActive>,
    time_left: Res<TimeLeft>,
    mut run: ResMut<RightOfWayRun>,
    mut pending: ResMut<PendingRightOfWayActions>,
    existing: Query<Entity, Or<(With<PackagePickup>, With<DeliveryTarget>)>>,
) {
    if round.0 {
        return;
    }
    for entity in &existing {
        commands.entity(entity).despawn();
    }
    *run = RightOfWayRun::default();
    pending.0.clear();
    run.score.remaining_ms = seconds_to_ms(time_left.0);
    if is_right_of_way(&rules) {
        for index in 0..3 {
            spawn_package(
                &mut commands,
                &assets,
                target_position(Vec3::ZERO, index, false),
                index,
            );
        }
        run.next_target = 3;
    }
}

fn cleanup_targets(
    mut commands: Commands,
    targets: Query<Entity, Or<(With<PackagePickup>, With<DeliveryTarget>)>>,
) {
    for entity in &targets {
        commands.entity(entity).despawn();
    }
}

fn target_position(origin: Vec3, index: u32, delivery: bool) -> Vec3 {
    // No RNG or ECS iteration participates: the target stream is fully fixed.
    let lane = [-10.0, 8.0, -6.0, 11.0, 4.0, -12.0][index as usize % 6];
    let ahead = if delivery {
        24.0
    } else {
        14.0 + (index % 3) as f32 * 7.0
    };
    Vec3::new(
        origin.x + lane,
        if delivery { 0.08 } else { PACKAGE_Y },
        origin.z - ahead,
    )
}

fn spawn_package(
    commands: &mut Commands,
    assets: &RightOfWayAssets,
    position: Vec3,
    target_index: u32,
) {
    commands.spawn((
        PackagePickup { target_index },
        Mesh3d(assets.package_mesh.clone()),
        MeshMaterial3d(assets.package_material.clone()),
        Transform::from_translation(position),
    ));
}

fn spawn_delivery(
    commands: &mut Commands,
    assets: &RightOfWayAssets,
    position: Vec3,
    target_index: u32,
) {
    commands.spawn((
        DeliveryTarget { target_index },
        Mesh3d(assets.delivery_mesh.clone()),
        MeshMaterial3d(assets.delivery_material.clone()),
        Transform::from_translation(position).with_rotation(Quat::from_rotation_x(1.57)),
    ));
}

fn assign_stable_chicken_ids(
    rules: Res<ActiveRunRules>,
    mut commands: Commands,
    mut run: ResMut<RightOfWayRun>,
    chickens: Query<Entity, (With<Chicken>, Without<StableChickenId>)>,
) {
    if !is_right_of_way(&rules) || run.failed {
        return;
    }
    let mut entities: Vec<_> = chickens.iter().collect();
    entities.sort_by_key(|entity| entity.to_bits());
    for entity in entities {
        let Some(next) = run.next_chicken_id.checked_add(1) else {
            run.reject();
            return;
        };
        commands
            .entity(entity)
            .insert(StableChickenId(run.next_chicken_id));
        run.next_chicken_id = next;
    }
}

fn tick_conduct_clock(
    rules: Res<ActiveRunRules>,
    clock: Res<ActivePlayClock>,
    mut run: ResMut<RightOfWayRun>,
) {
    if !is_right_of_way(&rules) || run.failed {
        return;
    }
    let now = clock.milliseconds();
    let elapsed = now.saturating_sub(run.last_active_ms);
    run.score.tick_guilt(elapsed);
    run.last_active_ms = now;
}

#[allow(clippy::too_many_arguments)]
fn collect_packages(
    mut commands: Commands,
    assets: Res<RightOfWayAssets>,
    rules: Res<ActiveRunRules>,
    clock: Res<ActivePlayClock>,
    frozen: Res<InputFrozen>,
    drowning: Res<Drowning>,
    car: Query<&Transform, With<Car>>,
    packages: Query<(Entity, &Transform, &PackagePickup), Without<Car>>,
    deliveries: Query<(Entity, &Transform, &DeliveryTarget), (With<DeliveryTarget>, Without<Car>)>,
    mut run: ResMut<RightOfWayRun>,
    mut queue: ResMut<CanonicalEventQueue>,
    mut pending: ResMut<PendingRightOfWayActions>,
    mut pickups: MessageWriter<PackagePickedUp>,
) {
    if !is_right_of_way(&rules) || run.failed || frozen.0 || drowning.active {
        return;
    }
    let Ok(car) = car.single() else { return };
    let car_pos = car.translation;
    let radius2 = INTERACTION_RADIUS * INTERACTION_RADIUS;

    let mut package_hits: Vec<_> = packages
        .iter()
        .filter(|(_, tf, _)| xz_distance_squared(car_pos, tf.translation) <= radius2)
        .collect();
    package_hits.sort_by_key(|(_, _, package)| package.target_index);
    let mut delivery_spawn_queued = !deliveries.is_empty();
    for (entity, _, package) in package_hits {
        let before = run.score.carried_packages;
        if !run.score.pickup_package() {
            break;
        }
        commands.entity(entity).despawn();
        queue.push(PendingCanonicalEvent {
            active_ms: clock.milliseconds(),
            stable_id: u64::from(package.target_index),
            payload: canonical::EventPayload::PackagePickup {
                carried_before: before,
                carried_after: run.score.carried_packages,
            },
        });
        pickups.write(PackagePickedUp);
        let index = run.next_target;
        run.next_target = match index.checked_add(1) {
            Some(next) => next,
            None => {
                run.reject();
                return;
            }
        };
        spawn_package(
            &mut commands,
            &assets,
            target_position(car_pos, index, false),
            index,
        );
        if !delivery_spawn_queued {
            spawn_delivery(
                &mut commands,
                &assets,
                target_position(car_pos, index, true),
                index,
            );
            delivery_spawn_queued = true;
        }
    }

    let mut delivery_hits: Vec<_> = deliveries
        .iter()
        .filter(|(_, tf, _)| xz_distance_squared(car_pos, tf.translation) <= radius2)
        .collect();
    delivery_hits.sort_by_key(|(_, _, target)| target.target_index);
    if run.score.carried_packages > 0 {
        if let Some((entity, _, target)) = delivery_hits.first() {
            commands.entity(*entity).despawn();
            pending.0.push(PendingRightOfWayAction::Delivery {
                active_ms: clock.milliseconds(),
                target_index: target.target_index,
            });
        }
    }
}

fn observe_courtesy(
    rules: Res<ActiveRunRules>,
    clock: Res<ActivePlayClock>,
    frozen: Res<InputFrozen>,
    drowning: Res<Drowning>,
    car: Query<(&Car, &Transform)>,
    chickens: Query<(&StableChickenId, &Transform), With<Chicken>>,
    mut run: ResMut<RightOfWayRun>,
    mut pending: ResMut<PendingRightOfWayActions>,
) {
    if !is_right_of_way(&rules) || run.failed || frozen.0 || drowning.active {
        return;
    }
    let Ok((car, car_tf)) = car.single() else {
        return;
    };
    let mut observed: Vec<_> = chickens.iter().collect();
    observed.sort_by_key(|(id, _)| id.0);
    for (id, tf) in observed {
        let distance = xz_distance_squared(car_tf.translation, tf.translation).sqrt();
        if !run
            .courtesy_gate
            .observe(id.0, clock.milliseconds(), car.speed.abs(), distance)
        {
            continue;
        }
        pending.0.push(PendingRightOfWayAction::Courtesy {
            active_ms: clock.milliseconds(),
            chicken_id: id.0,
        });
    }
}

fn queue_animal_hit_messages(
    rules: Res<ActiveRunRules>,
    clock: Res<ActivePlayClock>,
    mut chickens: MessageReader<ChickenHit>,
    mut critters: MessageReader<CritterHit>,
    run: Res<RightOfWayRun>,
    mut pending: ResMut<PendingRightOfWayActions>,
) {
    let chicken_count = chickens.read().count();
    let critter_count = critters.read().count();
    if !is_right_of_way(&rules) || run.failed {
        return;
    }
    let now = clock.milliseconds();
    for (ordinal, animal_kind) in std::iter::repeat_n(0_u8, chicken_count)
        .chain(std::iter::repeat_n(1_u8, critter_count))
        .enumerate()
    {
        let Ok(ordinal) = u32::try_from(ordinal) else {
            return;
        };
        pending.0.push(PendingRightOfWayAction::Animal {
            active_ms: now,
            animal_kind,
            ordinal,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_pending_actions(
    rules: Res<ActiveRunRules>,
    mut pending: ResMut<PendingRightOfWayActions>,
    mut run: ResMut<RightOfWayRun>,
    mut time_left: ResMut<TimeLeft>,
    mut queue: ResMut<CanonicalEventQueue>,
    mut delivered: MessageWriter<PackageDelivered>,
    mut courtesy_awards: MessageWriter<CourtesyAwarded>,
) {
    if !is_right_of_way(&rules) {
        pending.0.clear();
        return;
    }
    pending.0.sort_by_key(|action| action.sort_key());
    for action in pending.0.drain(..) {
        if run.failed {
            break;
        }
        match action {
            PendingRightOfWayAction::Delivery { active_ms, .. } => {
                run.score.remaining_ms = seconds_to_ms(time_left.0);
                let count = run.score.carried_packages;
                for ordinal in 0..count {
                    let chain_index = run.score.delivery_chain;
                    let premium_bps = run.score.premium_bps;
                    let guilt = run.score.guilt_remaining_ms != 0;
                    let remaining_before_ms = run.score.remaining_ms;
                    match run.score.deliver_package() {
                        Ok(Some(award)) => {
                            queue.push(PendingCanonicalEvent {
                                active_ms,
                                stable_id: u64::from(ordinal),
                                payload: canonical::EventPayload::PackageDelivery {
                                    delivered_ordinal_within_dropoff: ordinal,
                                    chain_index,
                                    base: award.base,
                                    premium_bps,
                                    guilt,
                                    credited: award.credited,
                                    accumulator_before: award.before,
                                    accumulator_after: award.after,
                                    remaining_before_ms,
                                    remaining_after_ms: run.score.remaining_ms,
                                },
                            });
                            delivered.write(PackageDelivered(ordinal));
                        }
                        _ => {
                            run.reject();
                            break;
                        }
                    }
                }
                time_left.0 = ms_to_seconds(run.score.remaining_ms);
            }
            PendingRightOfWayAction::Courtesy {
                active_ms,
                chicken_id,
            } => {
                let premium_bps = run.score.premium_bps;
                let guilt = run.score.guilt_remaining_ms != 0;
                match run.score.courtesy() {
                    Ok(award) => {
                        queue.push(PendingCanonicalEvent {
                            active_ms,
                            stable_id: u64::from(chicken_id),
                            payload: canonical::EventPayload::CourtesyAward {
                                chicken_stable_id: chicken_id,
                                premium_bps,
                                guilt,
                                credited: award.credited,
                                accumulator_before: award.before,
                                accumulator_after: award.after,
                                cooldown_after_ms: v3::COURTESY_COOLDOWN_MS as u32,
                            },
                        });
                        if award.credited > 0 {
                            courtesy_awards.write(CourtesyAwarded);
                        }
                    }
                    Err(_) => run.reject(),
                }
            }
            PendingRightOfWayAction::Animal {
                active_ms,
                animal_kind,
                ..
            } => {
                let premium_before_bps = run.score.premium_bps;
                match run.score.animal_hit() {
                    Ok((before, after)) => queue.push(PendingCanonicalEvent {
                        active_ms,
                        stable_id: u64::from(run.score.animal_hits),
                        payload: canonical::EventPayload::AnimalHit {
                            animal_kind,
                            delta: v3::ANIMAL_HIT_DELTA as i32,
                            premium_before_bps,
                            premium_after_bps: run.score.premium_bps,
                            guilt_after_ms: run.score.guilt_remaining_ms,
                            accumulator_before: before,
                            accumulator_after: after,
                        },
                    }),
                    Err(_) => run.reject(),
                }
            }
            PendingRightOfWayAction::Wave {
                active_ms,
                wave_index,
            } => {
                let premium_bps = run.score.premium_bps;
                let guilt = run.score.guilt_remaining_ms != 0;
                match run.score.wave(true) {
                    Ok(Some(award)) => queue.push(PendingCanonicalEvent {
                        active_ms,
                        stable_id: u64::from(wave_index),
                        payload: canonical::EventPayload::WaveAward {
                            base: award.base,
                            premium_bps,
                            guilt,
                            credited: award.credited,
                            accumulator_before: award.before,
                            accumulator_after: award.after,
                        },
                    }),
                    _ => run.reject(),
                }
            }
            PendingRightOfWayAction::Coin {
                active_ms,
                stable_id,
            } => {
                run.score.remaining_ms = seconds_to_ms(time_left.0);
                let remaining_before_ms = run.score.remaining_ms;
                let premium_bps = run.score.premium_bps;
                let guilt = run.score.guilt_remaining_ms != 0;
                match run.score.coin() {
                    Ok(award) => {
                        queue.push(PendingCanonicalEvent {
                            active_ms,
                            stable_id,
                            payload: canonical::EventPayload::CoinAward {
                                base: award.base,
                                premium_bps,
                                guilt,
                                credited: award.credited,
                                accumulator_before: award.before,
                                accumulator_after: award.after,
                                remaining_before_ms,
                                remaining_after_ms: run.score.remaining_ms,
                            },
                        });
                        time_left.0 = ms_to_seconds(run.score.remaining_ms);
                    }
                    Err(_) => run.reject(),
                }
            }
        }
    }
}

fn reject_failed_run(
    rules: Res<ActiveRunRules>,
    run: Res<RightOfWayRun>,
    mut next: ResMut<NextState<GameState>>,
) {
    if is_right_of_way(&rules) && run.failed {
        // Checked-arithmetic/protocol rejection is not a playable terminal and
        // must never produce a score snapshot or submission package.
        next.set(GameState::Menu);
    }
}

pub const fn seconds_to_ms(seconds: f32) -> u64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        0
    } else {
        (seconds as f64 * 1_000.0) as u64
    }
}
fn ms_to_seconds(ms: u64) -> f32 {
    ms as f32 / 1_000.0
}
fn xz_distance_squared(a: Vec3, b: Vec3) -> f32 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    dx * dx + dz * dz
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_stream_is_deterministic_and_distinguishes_delivery() {
        let origin = Vec3::new(4.0, 0.0, -8.0);
        assert_eq!(
            target_position(origin, 2, false),
            target_position(origin, 2, false)
        );
        assert_ne!(
            target_position(origin, 2, false),
            target_position(origin, 2, true)
        );
    }

    #[test]
    fn package_delivery_is_sequential_and_clock_caps() {
        let mut row = v3::RightOfWay::with_remaining(89_000);
        for _ in 0..3 {
            assert!(row.pickup_package());
        }
        assert!(!row.pickup_package());
        let awards = [
            row.deliver_package().unwrap().unwrap().base,
            row.deliver_package().unwrap().unwrap().base,
            row.deliver_package().unwrap().unwrap().base,
        ];
        assert_eq!(awards, [5, 6, 7]);
        assert_eq!(row.remaining_ms, 90_000);
        assert_eq!(row.carried_packages, 0);
    }

    #[test]
    fn pause_style_clock_conversion_is_bounded_and_exact_at_milliseconds() {
        assert_eq!(seconds_to_ms(60.0), 60_000);
        assert_eq!(seconds_to_ms(-1.0), 0);
        assert_eq!(seconds_to_ms(f32::NAN), 0);
    }

    #[test]
    fn simultaneous_actions_have_protocol_kind_then_stable_id_order() {
        let mut actions = [
            PendingRightOfWayAction::Coin {
                active_ms: 9,
                stable_id: 2,
            },
            PendingRightOfWayAction::Animal {
                active_ms: 9,
                animal_kind: 1,
                ordinal: 0,
            },
            PendingRightOfWayAction::Courtesy {
                active_ms: 9,
                chicken_id: 7,
            },
            PendingRightOfWayAction::Delivery {
                active_ms: 9,
                target_index: 3,
            },
            PendingRightOfWayAction::Coin {
                active_ms: 9,
                stable_id: 1,
            },
        ];
        actions.sort_by_key(|action| action.sort_key());
        assert!(matches!(
            actions[0],
            PendingRightOfWayAction::Delivery { .. }
        ));
        assert!(matches!(
            actions[1],
            PendingRightOfWayAction::Courtesy { .. }
        ));
        assert!(matches!(actions[2], PendingRightOfWayAction::Animal { .. }));
        assert!(matches!(
            actions[3],
            PendingRightOfWayAction::Coin { stable_id: 1, .. }
        ));
        assert!(matches!(
            actions[4],
            PendingRightOfWayAction::Coin { stable_id: 2, .. }
        ));
    }

    #[test]
    fn courtesy_bands_rearm_cooldown_and_zero_credit_are_exact() {
        let mut gate = v3::CourtesyGate::default();
        assert!(!gate.observe(1, 0, 4.0, 1.0));
        assert!(gate.observe(1, 0, 4.0, 1.0001));
        assert!(!gate.observe(1, 500, 4.0, 1.5));
        assert!(!gate.observe(1, 501, 4.0, 2.13));
        assert!(gate.observe(1, 501, 4.0, 2.12));

        let mut row = v3::RightOfWay::new();
        row.premium_bps = 0;
        let award = row.courtesy().unwrap();
        assert_eq!(award.credited, 0);
        assert_eq!(row.courtesy_count, 0);
    }

    #[test]
    fn hit_decay_guilt_chain_and_checked_failure_are_atomic() {
        let mut row = v3::RightOfWay::new();
        row.delivery_chain = 4;
        row.accumulator = i64::MIN + 9;
        let before = row;
        assert!(row.animal_hit().is_err());
        assert_eq!(row, before);

        row.accumulator = 10;
        row.animal_hit().unwrap();
        assert_eq!(row.accumulator, 0);
        assert_eq!(row.premium_bps, 9_000);
        assert_eq!(row.delivery_chain, 0);
        assert_eq!(row.guilt_remaining_ms, 5_000);
        row.tick_guilt(4_999);
        assert_eq!(row.guilt_remaining_ms, 1);
        row.tick_guilt(1);
        assert_eq!(row.guilt_remaining_ms, 0);
    }
}
