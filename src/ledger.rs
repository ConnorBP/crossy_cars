//! Game-owned canonical v3 event collection and terminal snapshot ordering.

use bevy::prelude::*;
use roady_score_rules::v3::{self, canonical};

use crate::combos::Combo;
use crate::game::resources::{GameOverReason, RoundActive, Score, TimeLeft};
use crate::game::state::GameState;
use crate::game::{SpawnSet, TerminalFinalizeSet, finalize_pending_terminal};
use crate::game_modes::{ActivePlayClock, ActiveRunRules, Conduct, GameModeSetupSet};
use crate::objectives::{ActiveObjective, ObjectiveFinalizeSet};
use crate::right_of_way::RightOfWayRun;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingCanonicalEvent {
    pub active_ms: u64,
    pub stable_id: u64,
    pub payload: canonical::EventPayload,
}

fn same_ms_order(payload: &canonical::EventPayload) -> u8 {
    use canonical::EventPayload::*;
    match payload {
        FrenzyChanged {
            phase: v3::FrenzyPhase::Expired,
            ..
        } => 0,
        SegmentChanged { active: false, .. } => 1,
        SegmentChanged { active: true, .. } => 2,
        FrenzyChanged {
            phase: v3::FrenzyPhase::Spawned,
            ..
        } => 3,
        FrenzyChanged {
            phase: v3::FrenzyPhase::Telegraph,
            ..
        } => 5,
        Terminal(_) => u8::MAX,
        _ => 6u8.saturating_add(payload.kind() as u8),
    }
}

pub fn canonical_event_order(
    a: &PendingCanonicalEvent,
    b: &PendingCanonicalEvent,
) -> std::cmp::Ordering {
    a.active_ms
        .cmp(&b.active_ms)
        .then_with(|| same_ms_order(&a.payload).cmp(&same_ms_order(&b.payload)))
        .then_with(|| (a.payload.kind() as u8).cmp(&(b.payload.kind() as u8)))
        .then_with(|| a.stable_id.cmp(&b.stable_id))
}

#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct CanonicalEventQueue(pub Vec<PendingCanonicalEvent>);

impl CanonicalEventQueue {
    pub fn push(&mut self, event: PendingCanonicalEvent) {
        self.0.push(event);
    }
}

#[derive(Resource, Debug, Default)]
pub struct V3LedgerState {
    pub ledger: Option<canonical::CanonicalLedger>,
    pub session_id: Option<String>,
    pub terminal_queued: bool,
    pub final_root: Option<[u8; 32]>,
    pub failure: Option<canonical::CanonicalError>,
}

#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct FinalGameOverSnapshot {
    pub reason: Option<v3::TerminalReason>,
    pub terminal: Option<canonical::ConductTerminal>,
    pub final_root: Option<[u8; 32]>,
    pub event_count: u32,
}

pub const fn terminal_reason(reason: GameOverReason) -> v3::TerminalReason {
    match reason {
        GameOverReason::TimeUp => v3::TerminalReason::TimeUp,
        GameOverReason::Wrecked => v3::TerminalReason::Wrecked,
        GameOverReason::Drowned => v3::TerminalReason::Drowned,
    }
}

pub struct LedgerPlugin;
impl Plugin for LedgerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CanonicalEventQueue>()
            .init_resource::<V3LedgerState>()
            .init_resource::<FinalGameOverSnapshot>()
            .add_systems(
                OnEnter(GameState::Playing),
                setup_fresh_ledger.in_set(SpawnSet).after(GameModeSetupSet),
            )
            // Objective processing is in PostUpdate. Drain non-terminal events
            // after it, append Terminal last, finalize root, then transition.
            .add_systems(
                PostUpdate,
                (flush_events, finalize_pending_terminal)
                    .chain()
                    .in_set(TerminalFinalizeSet)
                    .after(ObjectiveFinalizeSet)
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(OnEnter(GameState::GameOver), capture_final_snapshot);
    }
}

fn setup_fresh_ledger(
    round_active: Res<RoundActive>,
    rules: Res<ActiveRunRules>,
    mut state: ResMut<V3LedgerState>,
    mut queue: ResMut<CanonicalEventQueue>,
    mut snapshot: ResMut<FinalGameOverSnapshot>,
) {
    if round_active.0 {
        return;
    }
    *state = V3LedgerState::default();
    queue.0.clear();
    *snapshot = FinalGameOverSnapshot::default();
    if let Some(receipt) = rules.ranked_receipt() {
        state.ledger = Some(canonical::CanonicalLedger::new(&receipt.started_header));
        state.session_id = Some(receipt.session_id.clone());
    }
}

fn flush_events(mut queue: ResMut<CanonicalEventQueue>, mut state: ResMut<V3LedgerState>) {
    let Some(mut ledger) = state.ledger.take() else {
        queue.0.clear();
        return;
    };
    queue.0.sort_by(canonical_event_order);
    for pending in queue.0.drain(..) {
        let event = canonical::Event {
            seq: ledger.event_count(),
            active_ms: pending.active_ms,
            payload: pending.payload,
        };
        if let Err(error) = ledger.append(&event) {
            state.failure = Some(error);
            break;
        }
    }
    state.ledger = Some(ledger);
}

/// Append the exactly-one terminal after final objective reward and before the
/// immutable OnEnter(GameOver) snapshot. Calling twice is harmless.
pub fn finalize_terminal(
    rules: &ActiveRunRules,
    clock: &ActivePlayClock,
    reason: GameOverReason,
    score: &Score,
    time_left: &TimeLeft,
    objective: &ActiveObjective,
    combo: &Combo,
    right_of_way: Option<&RightOfWayRun>,
    state: &mut V3LedgerState,
) -> Result<(), canonical::CanonicalError> {
    if state.terminal_queued || state.ledger.is_none() {
        return Ok(());
    }
    let reason = terminal_reason(reason);
    let duration_ms = clock.milliseconds();
    let remaining_ms =
        ((time_left.0.max(0.0) as f64 * 1_000.0).round() as u64).min(canonical::MAX_REMAINING_MS);
    let platform = if cfg!(target_arch = "wasm32") {
        v3::Platform::Web
    } else {
        v3::Platform::Native
    };
    let total = score.chickens.checked_add(score.coins).unwrap_or(u32::MAX);
    let terminal = match rules.conduct {
        Conduct::CluckHunt => canonical::ConductTerminal::CluckHunt(canonical::CluckTerminal {
            reason,
            total,
            chickens: score.chickens,
            coins: score.coins,
            objective_completed: objective.completed,
            max_combo: combo.multiplier.clamp(1, 5) as u8,
            duration_ms,
            remaining_ms,
            build: env!("CARGO_PKG_VERSION").into(),
            platform,
        }),
        Conduct::RightOfWay => {
            let row = right_of_way.ok_or(canonical::CanonicalError::MissingTerminal)?;
            let total = row
                .score
                .terminal_total()
                .map_err(|_| canonical::CanonicalError::MissingTerminal)?;
            canonical::ConductTerminal::RightOfWay(canonical::RightOfWayTerminal {
                reason,
                total,
                accumulator: row.score.accumulator,
                premium_bps: row.score.premium_bps,
                packages_delivered: row.score.packages_delivered,
                courtesy_count: row.score.courtesy_count,
                animal_hits: row.score.animal_hits,
                max_delivery_chain: row.score.max_delivery_chain,
                objective_completed: row.score.objective_completed,
                duration_ms,
                remaining_ms,
                build: env!("CARGO_PKG_VERSION").into(),
                platform,
            })
        }
    };
    let ledger = state.ledger.as_mut().expect("checked above");
    ledger.append(&canonical::Event {
        seq: ledger.event_count(),
        active_ms: duration_ms,
        payload: canonical::EventPayload::Terminal(terminal),
    })?;
    state.final_root = Some(ledger.final_root()?);
    state.terminal_queued = true;
    Ok(())
}

fn capture_final_snapshot(mut snapshot: ResMut<FinalGameOverSnapshot>, state: Res<V3LedgerState>) {
    let Some(ledger) = state.ledger.as_ref() else {
        return;
    };
    let Ok(terminal) = ledger.terminal() else {
        return;
    };
    *snapshot = FinalGameOverSnapshot {
        reason: Some(match terminal {
            canonical::ConductTerminal::CluckHunt(value) => value.reason,
            canonical::ConductTerminal::RightOfWay(value) => value.reason,
        }),
        terminal: Some(terminal.clone()),
        final_root: state.final_root,
        event_count: ledger.event_count(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drowned_maps_to_stable_v3_reason() {
        assert_eq!(
            terminal_reason(GameOverReason::Drowned),
            v3::TerminalReason::Drowned
        );
        assert_eq!(terminal_reason(GameOverReason::TimeUp) as u8, 1);
        assert_eq!(terminal_reason(GameOverReason::Wrecked) as u8, 2);
    }

    #[test]
    fn same_ms_objective_precedes_terminal_and_terminal_sorts_last() {
        let terminal = canonical::ConductTerminal::CluckHunt(canonical::CluckTerminal {
            reason: v3::TerminalReason::Drowned,
            total: 10,
            chickens: 10,
            coins: 0,
            objective_completed: true,
            max_combo: 1,
            duration_ms: 1,
            remaining_ms: 1,
            build: "dev".into(),
            platform: v3::Platform::Web,
        });
        let mut events = [
            PendingCanonicalEvent {
                active_ms: 1,
                stable_id: 0,
                payload: canonical::EventPayload::Terminal(terminal),
            },
            PendingCanonicalEvent {
                active_ms: 1,
                stable_id: 0,
                payload: canonical::EventPayload::ObjectiveCompletedCluck {
                    objective: v3::Objective::HitChickens,
                    target: 10,
                    base_reward: 10,
                    bucket_before: 0,
                    bucket_after: 10,
                },
            },
        ];
        events.sort_by(canonical_event_order);
        assert_eq!(events[0].payload.kind(), v3::EventKind::ObjectiveCompleted);
        assert_eq!(events[1].payload.kind(), v3::EventKind::Terminal);
    }
}
