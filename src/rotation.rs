//! Deterministic v3 schedule, phase, wave, Frenzy and population budgets.

use bevy::prelude::*;
use roady_score_rules::v3::{self, Effect, FrenzyOpportunity, RotationWindow, ScheduledEvent};

use crate::game::resources::{RoundActive, Score};
use crate::game::state::GameState;
use crate::game::{RoundClockSet, SpawnSet};
use crate::game_modes::{ActivePlayClock, ActiveRunRules, Conduct, GameModeSetupSet};
use crate::ledger::{CanonicalEventQueue, PendingCanonicalEvent};
use crate::modifiers::{ActiveModifier, effect_modifier};
use crate::right_of_way::{PendingRightOfWayActions, RightOfWayRun};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RotationPhase {
    #[default]
    Grace,
    Telegraph {
        index: u8,
        effect: Effect,
    },
    Active {
        index: u8,
        effect: Effect,
    },
    Cooldown {
        index: u8,
    },
    Complete,
}

#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct RotationState {
    pub windows: Option<[RotationWindow; v3::SCHEDULE_SEGMENTS]>,
    pub events: Option<[ScheduledEvent; 2]>,
    pub phase: RotationPhase,
    pub awarded_waves: u32,
    pub last_emitted_ms: u64,
}

impl Default for RotationState {
    fn default() -> Self {
        Self {
            windows: None,
            events: None,
            phase: RotationPhase::Grace,
            awarded_waves: 0,
            last_emitted_ms: 0,
        }
    }
}

impl RotationState {
    pub fn effect_at(&self, active_ms: u64) -> Option<Effect> {
        self.windows
            .as_ref()
            .and_then(|schedule| v3::active_effect_at(schedule, active_ms))
    }

    pub fn event_at(&self, active_ms: u64) -> Option<ScheduledEvent> {
        let events = self.events?;
        v3::EVENT_WINDOWS
            .iter()
            .enumerate()
            .find_map(|(index, &(start, end))| {
                (active_ms >= start && active_ms < end).then_some(events[index])
            })
    }
}

pub fn phase_at(schedule: &[RotationWindow; v3::SCHEDULE_SEGMENTS], now: u64) -> RotationPhase {
    if now < v3::INITIAL_GRACE_MS {
        return RotationPhase::Grace;
    }
    for (index, window) in schedule.iter().enumerate() {
        let index = index as u8;
        if now < window.active_start_ms {
            return RotationPhase::Telegraph {
                index,
                effect: window.effect,
            };
        }
        if now < window.active_end_ms {
            return RotationPhase::Active {
                index,
                effect: window.effect,
            };
        }
        if now < window.cooldown_end_ms {
            return RotationPhase::Cooldown { index };
        }
    }
    RotationPhase::Complete
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrenzyRuntimePhase {
    #[default]
    Waiting,
    Orb {
        spawned_ms: u64,
        approached: bool,
        relocation_checked: bool,
    },
    Telegraph {
        start_ms: u64,
        end_ms: u64,
    },
    Active {
        start_ms: u64,
        end_ms: u64,
    },
    Expired,
}

#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct FrenzyState {
    pub opportunities: Vec<FrenzyOpportunity>,
    pub next_opportunity: usize,
    pub phase: FrenzyRuntimePhase,
}

impl FrenzyState {
    pub fn activation_active(&self, now: u64) -> bool {
        matches!(self.phase, FrenzyRuntimePhase::Active { start_ms, end_ms } if now >= start_ms && now < end_ms)
    }

    /// Expiry is resolved before collection at the exact lifetime boundary.
    pub fn collect(&mut self, now: u64) -> bool {
        match self.phase {
            FrenzyRuntimePhase::Orb { spawned_ms, .. } if v3::frenzy_orb_alive(spawned_ms, now) => {
                self.phase = FrenzyRuntimePhase::Telegraph {
                    start_ms: now,
                    end_ms: now.saturating_add(v3::FRENZY_TELEGRAPH_MS),
                };
                true
            }
            FrenzyRuntimePhase::Orb { .. } => {
                self.phase = FrenzyRuntimePhase::Expired;
                false
            }
            _ => false,
        }
    }

    pub fn tick(&mut self, now: u64) {
        match self.phase {
            FrenzyRuntimePhase::Waiting => {
                if let Some(opportunity) = self.opportunities.get(self.next_opportunity).copied() {
                    if now >= opportunity.at_ms {
                        self.next_opportunity += 1;
                        if opportunity.spawn {
                            self.phase = FrenzyRuntimePhase::Orb {
                                spawned_ms: opportunity.at_ms,
                                approached: false,
                                relocation_checked: false,
                            };
                        }
                    }
                }
            }
            FrenzyRuntimePhase::Orb { spawned_ms, .. }
                if !v3::frenzy_orb_alive(spawned_ms, now) =>
            {
                self.phase = FrenzyRuntimePhase::Expired;
            }
            FrenzyRuntimePhase::Telegraph { end_ms, .. } if now >= end_ms => {
                self.phase = FrenzyRuntimePhase::Active {
                    start_ms: end_ms,
                    end_ms: end_ms.saturating_add(v3::FRENZY_ACTIVE_MS),
                };
            }
            FrenzyRuntimePhase::Active { end_ms, .. } if now >= end_ms => {
                self.phase = FrenzyRuntimePhase::Expired;
            }
            _ => {}
        }
    }
}

/// Active-time token budget. Fractions are retained and no RNG is consumed by
/// this type; callers consume RNG only for the returned spawn count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PopulationBudget {
    spawn_milli_tokens: u64,
    retire_milli_tokens: u64,
}

impl PopulationBudget {
    pub fn reconcile(&mut self, elapsed_ms: u64, current: usize, target: usize) -> (usize, usize) {
        if current < target {
            self.spawn_milli_tokens = self.spawn_milli_tokens.saturating_add(elapsed_ms * 12);
            let available = (self.spawn_milli_tokens / 1_000) as usize;
            let spawn = available.min(target - current);
            self.spawn_milli_tokens -= spawn as u64 * 1_000;
            (spawn, 0)
        } else if current > target {
            self.retire_milli_tokens = self.retire_milli_tokens.saturating_add(elapsed_ms * 18);
            let available = (self.retire_milli_tokens / 1_000) as usize;
            let retire = available.min(current - target);
            self.retire_milli_tokens -= retire as u64 * 1_000;
            (0, retire)
        } else {
            (0, 0)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RetirementCandidate {
    pub entity_bits: u64,
    pub effect_extra: bool,
    pub outside_camera: bool,
    pub behind_car: bool,
    pub distance_sq: f32,
}

pub fn deterministic_retirement_order(
    a: &RetirementCandidate,
    b: &RetirementCandidate,
) -> std::cmp::Ordering {
    // Effect extras first, then outside camera, behind, farthest, entity bits.
    b.effect_extra
        .cmp(&a.effect_extra)
        .then_with(|| b.outside_camera.cmp(&a.outside_camera))
        .then_with(|| b.behind_car.cmp(&a.behind_car))
        .then_with(|| b.distance_sq.total_cmp(&a.distance_sq))
        .then_with(|| a.entity_bits.cmp(&b.entity_bits))
}

fn emit_segment_edges(
    previous_ms: u64,
    now_ms: u64,
    schedule: &[RotationWindow; v3::SCHEDULE_SEGMENTS],
    events: Option<[ScheduledEvent; 2]>,
    queue: &mut CanonicalEventQueue,
) {
    if now_ms < previous_ms {
        return;
    }
    for window in schedule {
        for (at, active) in [
            (window.active_end_ms, false),
            (window.active_start_ms, true),
        ] {
            if at > previous_ms && at <= now_ms {
                queue.push(PendingCanonicalEvent {
                    active_ms: at,
                    stable_id: u64::from(window.effect as u8),
                    payload: v3::canonical::EventPayload::SegmentChanged {
                        segment_kind: 0,
                        effect_or_event: window.effect as u8,
                        active,
                        start_ms: window.active_start_ms,
                        end_ms: window.active_end_ms,
                    },
                });
            }
        }
    }
    if let Some(events) = events {
        for (index, &(start_ms, end_ms)) in v3::EVENT_WINDOWS.iter().enumerate() {
            for (at, active) in [(end_ms, false), (start_ms, true)] {
                if at > previous_ms && at <= now_ms {
                    queue.push(PendingCanonicalEvent {
                        active_ms: at,
                        stable_id: index as u64,
                        payload: v3::canonical::EventPayload::SegmentChanged {
                            segment_kind: 1,
                            effect_or_event: events[index] as u8,
                            active,
                            start_ms,
                            end_ms,
                        },
                    });
                }
            }
        }
    }
}

pub struct RotationPlugin;
impl Plugin for RotationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RotationState>()
            .init_resource::<FrenzyState>()
            .add_systems(
                OnEnter(GameState::Playing),
                setup_rotation.in_set(SpawnSet).after(GameModeSetupSet),
            )
            .add_systems(
                Update,
                tick_rotation
                    .after(RoundClockSet)
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

fn setup_rotation(
    round_active: Res<RoundActive>,
    rules: Res<ActiveRunRules>,
    mut rotation: ResMut<RotationState>,
    mut frenzy: ResMut<FrenzyState>,
) {
    if round_active.0 {
        return;
    }
    *rotation = RotationState::default();
    *frenzy = FrenzyState::default();
    if let Some(receipt) = rules.ranked_receipt() {
        let windows = receipt.schedule;
        rotation.events = Some(v3::scheduled_events(&receipt.seed, &windows));
        rotation.windows = Some(windows);
        frenzy.opportunities = v3::frenzy_opportunities(&receipt.seed, v3::FRENZY_PITY_MS + 12_001);
    }
}

fn tick_rotation(
    clock: Res<ActivePlayClock>,
    mut rotation: ResMut<RotationState>,
    mut frenzy: ResMut<FrenzyState>,
    mut modifier: ResMut<ActiveModifier>,
    rules: Res<ActiveRunRules>,
    mut score: ResMut<Score>,
    right_of_way: Option<Res<RightOfWayRun>>,
    mut pending_right_of_way: Option<ResMut<PendingRightOfWayActions>>,
    mut canonical_queue: Option<ResMut<CanonicalEventQueue>>,
) {
    let now = clock.milliseconds();
    if let Some(schedule) = rotation.windows {
        if let Some(queue) = canonical_queue.as_mut() {
            emit_segment_edges(
                rotation.last_emitted_ms,
                now,
                &schedule,
                rotation.events,
                queue,
            );
        }
        rotation.last_emitted_ms = now;
        rotation.phase = phase_at(&schedule, now);
        modifier.0 = rotation
            .effect_at(now)
            .map(effect_modifier)
            .unwrap_or(crate::modifiers::ModifierKind::Standard);
        let completed = v3::completed_waves(now).min(v3::SCHEDULE_SEGMENTS as u32);
        if rules.is_ranked() {
            while rotation.awarded_waves < completed {
                match rules.conduct {
                    Conduct::CluckHunt => {
                        score.chickens = v3::cluck_wave_award(score.chickens, true);
                    }
                    Conduct::RightOfWay => {
                        if right_of_way.as_ref().is_none_or(|run| run.failed) {
                            break;
                        }
                        let Some(pending) = pending_right_of_way.as_mut() else {
                            break;
                        };
                        pending.wave(now, rotation.awarded_waves);
                    }
                }
                rotation.awarded_waves += 1;
            }
        }
    }
    frenzy.tick(now);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed() -> [u8; 32] {
        core::array::from_fn(|i| i as u8 + 1)
    }

    #[test]
    fn every_phase_boundary_is_half_open_and_never_shifts() {
        let schedule = v3::rotation_schedule(&seed());
        assert_eq!(phase_at(&schedule, 7_999), RotationPhase::Grace);
        assert!(matches!(
            phase_at(&schedule, 8_000),
            RotationPhase::Telegraph { index: 0, .. }
        ));
        assert!(matches!(
            phase_at(&schedule, 10_999),
            RotationPhase::Telegraph { index: 0, .. }
        ));
        assert!(matches!(
            phase_at(&schedule, 11_000),
            RotationPhase::Active { index: 0, .. }
        ));
        assert!(matches!(
            phase_at(&schedule, 28_999),
            RotationPhase::Active { index: 0, .. }
        ));
        assert_eq!(
            phase_at(&schedule, 29_000),
            RotationPhase::Cooldown { index: 0 }
        );
        assert!(matches!(
            phase_at(&schedule, 36_000),
            RotationPhase::Telegraph { index: 1, .. }
        ));
        assert_eq!(schedule[15].telegraph_start_ms, 8_000 + 15 * 28_000);
    }

    #[test]
    fn population_budget_is_frame_rate_independent_and_bounded() {
        for step in [1_000, 100, 10] {
            let mut budget = PopulationBudget::default();
            let mut count = 0;
            for _ in 0..1_000 / step {
                let (spawn, retire) = budget.reconcile(step, count, 40);
                count = count + spawn - retire;
            }
            assert_eq!(count, 12);
        }
        let mut budget = PopulationBudget::default();
        assert_eq!(budget.reconcile(1_000, 30, 5), (0, 18));
    }

    #[test]
    fn active_frenzy_is_half_open() {
        let frenzy = FrenzyState {
            opportunities: vec![],
            next_opportunity: 0,
            phase: FrenzyRuntimePhase::Active {
                start_ms: 10,
                end_ms: 20,
            },
        };
        assert!(!frenzy.activation_active(9));
        assert!(frenzy.activation_active(10));
        assert!(frenzy.activation_active(19));
        assert!(!frenzy.activation_active(20));
    }

    #[test]
    fn scheduled_events_exclude_the_matching_active_effect() {
        let schedule = v3::rotation_schedule(&seed());
        let events = v3::scheduled_events(&seed(), &schedule);
        for (index, &(start, _)) in v3::EVENT_WINDOWS.iter().enumerate() {
            let forbidden = match v3::active_effect_at(&schedule, start) {
                Some(Effect::RushHour) => Some(ScheduledEvent::TrafficSurge),
                Some(Effect::Stampede) => Some(ScheduledEvent::CritterBurst),
                Some(Effect::GlassCannon) => Some(ScheduledEvent::ComboFrenzy),
                _ => None,
            };
            assert_ne!(Some(events[index]), forbidden);
        }
    }

    #[test]
    fn frenzy_stream_draw_counts_and_pity_are_deterministic() {
        let a = v3::frenzy_opportunities(&seed(), 100_000);
        let b = v3::frenzy_opportunities(&seed(), 100_000);
        assert_eq!(a, b);
        assert_eq!(a.iter().filter(|op| op.spawn).count(), 1);
        assert!(a.last().unwrap().spawn);
        assert!(a.len() <= 7, "one interval and roll draw per opportunity");
        assert_eq!(v3::frenzy_relocation_candidates(&seed()).len(), 8);
    }

    #[test]
    fn completed_wave_boundaries_are_exact() {
        assert_eq!(v3::completed_waves(35_999), 0);
        assert_eq!(v3::completed_waves(36_000), 1);
        assert_eq!(v3::completed_waves(63_999), 1);
        assert_eq!(v3::completed_waves(64_000), 2);
    }

    #[test]
    fn frenzy_expiry_precedes_same_ms_collection() {
        let mut frenzy = FrenzyState {
            opportunities: vec![],
            next_opportunity: 0,
            phase: FrenzyRuntimePhase::Orb {
                spawned_ms: 100,
                approached: false,
                relocation_checked: false,
            },
        };
        assert!(frenzy.collect(12_099));
        frenzy.phase = FrenzyRuntimePhase::Orb {
            spawned_ms: 100,
            approached: false,
            relocation_checked: false,
        };
        assert!(!frenzy.collect(12_100));
        assert_eq!(frenzy.phase, FrenzyRuntimePhase::Expired);
    }

    #[test]
    fn segment_edges_emit_every_crossed_boundary_once_and_in_order() {
        let schedule = v3::rotation_schedule(&seed());
        let events = v3::scheduled_events(&seed(), &schedule);
        let mut queue = CanonicalEventQueue::default();
        emit_segment_edges(10_999, 29_000, &schedule, Some(events), &mut queue);
        queue.0.sort_by(crate::ledger::canonical_event_order);
        let points: Vec<_> = queue
            .0
            .iter()
            .map(|event| (event.active_ms, event.payload.kind()))
            .collect();
        assert_eq!(points.len(), 4); // rotation start/end + E0 start/end
        assert_eq!(points[0].0, 11_000);
        assert_eq!(points[1].0, 15_000);
        assert_eq!(points[2].0, 23_000);
        assert_eq!(points[3].0, 29_000);

        let mut none = CanonicalEventQueue::default();
        emit_segment_edges(29_000, 29_000, &schedule, Some(events), &mut none);
        assert!(none.0.is_empty());
    }

    #[test]
    fn retirement_order_is_total_and_stable() {
        let mut candidates = [
            RetirementCandidate {
                entity_bits: 9,
                effect_extra: false,
                outside_camera: true,
                behind_car: true,
                distance_sq: 9.0,
            },
            RetirementCandidate {
                entity_bits: 8,
                effect_extra: true,
                outside_camera: false,
                behind_car: false,
                distance_sq: 1.0,
            },
            RetirementCandidate {
                entity_bits: 2,
                effect_extra: false,
                outside_camera: true,
                behind_car: true,
                distance_sq: 9.0,
            },
        ];
        candidates.sort_by(deterministic_retirement_order);
        assert_eq!(candidates.map(|c| c.entity_bits), [8, 2, 9]);
    }
}
