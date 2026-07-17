pub mod events;
pub mod resources;
pub mod state;

use bevy::prelude::*;

use crate::car::{Car, DriftLatch, InputFrozen};
use crate::combos::Combo;
use crate::game::events::{ChickenHit, CoinCollected, ObstacleHit, PondEntered};
use crate::game::resources::{Drowning, GameConfig, GameOverReason, RoundActive, Score, TimeLeft};
use crate::game::state::GameState;
use crate::game_modes::{ActivePlayClock, ActiveRunRules};
use crate::ledger::{V3LedgerState, finalize_terminal};
use crate::objectives::ActiveObjective;
use crate::persist::{BestAtRoundStart, BestScore, ConditionBests, ConditionBestsAtRoundStart};
use crate::right_of_way::RightOfWayRun;
use crate::settings::SettingsOpen;

/// Set while a paused run is being routed through Menu for a safe restart.
#[derive(Resource, Default)]
pub struct RestartRequested(pub bool);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PauseDecision {
    None,
    Resume,
    Restart,
    Menu,
}

/// State action shared by touch and keyboard state controls. Every non-`None`
/// action also carries an explicit restart-latch result, preventing an older
/// restart request from surviving a later resume/menu/play action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StateAction {
    None,
    Playing,
    Paused,
    Menu,
    Restart,
}

fn state_transitions_enabled(settings_open: bool) -> bool {
    !settings_open
}

/// Run condition preventing gameplay state input from acting through the
/// settings modal.
pub(crate) fn settings_closed(settings_open: Res<SettingsOpen>) -> bool {
    state_transitions_enabled(settings_open.0)
}

fn state_action_transition(action: StateAction) -> Option<(GameState, bool)> {
    match action {
        StateAction::None => None,
        StateAction::Playing => Some((GameState::Playing, false)),
        StateAction::Paused => Some((GameState::Paused, false)),
        StateAction::Menu => Some((GameState::Menu, false)),
        StateAction::Restart => Some((GameState::Menu, true)),
    }
}

/// Apply one resolved action. Update ordering determines which input source is
/// applied last; a later non-empty action deterministically replaces both the
/// target state and restart latch.
pub(crate) fn apply_state_action(
    action: StateAction,
    restart: &mut RestartRequested,
    next: &mut NextState<GameState>,
) {
    if let Some((target, requests_restart)) = state_action_transition(action) {
        restart.0 = requests_restart;
        next.set(target);
    }
}

/// Pure test model of the chained input sets: an available keyboard action is
/// the later writer and therefore wins over a simultaneous touch action.
#[cfg(test)]
fn resolve_ordered_actions(touch: StateAction, keyboard: StateAction) -> StateAction {
    if keyboard != StateAction::None {
        keyboard
    } else {
        touch
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RoundEntryDecision {
    Fresh,
    Resume,
}

/// Decide whether entering Playing starts a fresh round or resumes the active one.
///
/// Keeping this decision pure makes it explicit that `RoundActive` remains true
/// while paused and that every fresh-round system must leave resume state intact.
fn round_entry_decision(round_active: bool) -> RoundEntryDecision {
    if round_active {
        RoundEntryDecision::Resume
    } else {
        RoundEntryDecision::Fresh
    }
}

/// Resolve simultaneous pause-screen key presses with destructive actions first.
fn pause_decision(escape: bool, restart: bool, menu: bool) -> PauseDecision {
    if restart {
        PauseDecision::Restart
    } else if menu {
        PauseDecision::Menu
    } else if escape {
        PauseDecision::Resume
    } else {
        PauseDecision::None
    }
}

/// System set grouping all fresh-round spawn systems that run on
/// `OnEnter(GameState::Playing)`. Resources and the car are reset before this
/// set, while `RoundActive` stays false until every spawn system has finished.
/// Later waves (T3 `spawn_chickens`, T6 `start_countdown`) add their systems to
/// this set so resume-from-Paused / fresh-round gating stays correct (risk E11).
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SpawnSet;

/// Touch-driven game-state transitions run before keyboard transitions.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TouchStateSet;

/// Keyboard state transitions resolve after touch and before the round clock.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct KeyboardStateSet;

/// Round-clock work runs after state input and before driving. Exported so
/// gameplay plugins can share the one authoritative frame-order contract.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RoundClockSet;

/// Same-frame terminal finalization after objective processing.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TerminalFinalizeSet;

pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<GameState>()
            .init_resource::<GameConfig>()
            .init_resource::<Score>()
            .init_resource::<TimeLeft>()
            .init_resource::<RoundActive>()
            .init_resource::<Drowning>()
            .init_resource::<InputFrozen>()
            .init_resource::<GameOverReason>()
            .init_resource::<RestartRequested>()
            // Competition/conduct runtime is registered by GameModesPlugin;
            // optional consumers below keep focused legacy harnesses valid.
            .add_message::<ChickenHit>()
            .add_message::<CoinCollected>()
            .add_message::<ObstacleHit>()
            .add_message::<PondEntered>()
            .configure_sets(
                Update,
                (TouchStateSet, KeyboardStateSet, RoundClockSet).chain(),
            )
            // Keep RoundActive false across SpawnSet so all fresh-only spawn
            // systems can distinguish a new round from a pause resume. Reset
            // state first, spawn second, then activate the completed round.
            .configure_sets(OnEnter(GameState::Playing), SpawnSet)
            .add_systems(
                OnEnter(GameState::Playing),
                reset_car_and_resources.before(SpawnSet),
            )
            .add_systems(OnEnter(GameState::Playing), activate_round.after(SpawnSet))
            // End the round (clear the active flag) when leaving for GameOver
            // or Menu; the world plugin despawns round entities on these too.
            .add_systems(
                OnEnter(GameState::GameOver),
                (end_round, clear_drowning).chain(),
            )
            // A paused restart deliberately visits Menu so all existing
            // end-round and cleanup systems run before a fresh Playing enter.
            .add_systems(
                OnEnter(GameState::Menu),
                (end_round, clear_drowning, consume_restart_request).chain(),
            )
            .add_systems(
                Update,
                tick_timeleft
                    .in_set(RoundClockSet)
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(
                Update,
                pause_to_paused
                    .in_set(KeyboardStateSet)
                    .run_if(in_state(GameState::Playing))
                    .run_if(settings_closed),
            )
            .add_systems(
                Update,
                pause_to_playing
                    .in_set(KeyboardStateSet)
                    .run_if(in_state(GameState::Paused))
                    .run_if(settings_closed),
            )
            .add_systems(
                Update,
                gameover_input
                    .in_set(KeyboardStateSet)
                    .run_if(in_state(GameState::GameOver))
                    .run_if(settings_closed),
            );
    }
}

fn reset_car_and_resources(
    mut score: ResMut<Score>,
    mut timeleft: ResMut<TimeLeft>,
    round_active: Res<RoundActive>,
    mut drowning: ResMut<Drowning>,
    best: Res<BestScore>,
    condition_bests: Res<ConditionBests>,
    mut best_at_start: ResMut<BestAtRoundStart>,
    mut condition_bests_at_start: ResMut<ConditionBestsAtRoundStart>,
    mut car: Query<(&mut Car, &mut Transform, &mut DriftLatch)>,
) {
    // Resuming from Paused preserves the entire run, including both original
    // best-score snapshots and the car's exact state.
    if round_entry_decision(round_active.0) == RoundEntryDecision::Resume {
        return;
    }

    *drowning = Drowning::default();
    // The reset pose is authoritative before the very first movement frame,
    // allowing that frame to use the same high-speed sweep as every other.
    drowning.previous_resolved_center = Vec2::ZERO;
    drowning.previous_resolved_heading = 0.0;
    drowning.motion_end_center = Vec2::ZERO;
    drowning.motion_end_heading = 0.0;
    drowning.initialized = true;
    best_at_start.0 = best.0;
    condition_bests_at_start.by_kind = condition_bests.by_kind;
    *score = Score::default();
    *timeleft = TimeLeft::default();
    if let Ok((mut car, mut tf, mut drift_latch)) = car.single_mut() {
        car.speed = 0.0;
        car.heading = 0.0;
        car.drift = 0.0;
        *drift_latch = DriftLatch::default();
        *tf = Transform::default();
    }
}

fn activate_round(mut round_active: ResMut<RoundActive>) {
    // A pause resume is already active and must remain completely untouched.
    if round_entry_decision(round_active.0) == RoundEntryDecision::Resume {
        return;
    }

    round_active.0 = true;
}

fn end_round(mut round_active: ResMut<RoundActive>) {
    round_active.0 = false;
}

fn clear_drowning(mut drowning: ResMut<Drowning>) {
    *drowning = Drowning::default();
}

fn consume_restart_request(
    mut restart: ResMut<RestartRequested>,
    mut next: ResMut<NextState<GameState>>,
) {
    if restart.0 {
        restart.0 = false;
        next.set(GameState::Playing);
    }
}

fn tick_timeleft(
    mut t: ResMut<TimeLeft>,
    time: Res<Time>,
    mut next: ResMut<NextState<GameState>>,
    input_frozen: Res<InputFrozen>,
    drowning: Res<Drowning>,
    mut reason: ResMut<GameOverReason>,
) {
    // Don't burn the 60s round timer while a countdown overlay is active.
    if input_frozen.0 || drowning.active {
        return;
    }
    t.0 -= time.delta_secs();
    if t.0 <= 0.0 {
        t.0 = 0.0;
        *reason = GameOverReason::TimeUp;
        next.set(GameState::GameOver);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn finalize_pending_terminal(
    mut next: ResMut<NextState<GameState>>,
    rules: Option<Res<ActiveRunRules>>,
    clock: Option<Res<ActivePlayClock>>,
    reason: Res<GameOverReason>,
    score: Res<Score>,
    timeleft: Res<TimeLeft>,
    objective: Option<Res<ActiveObjective>>,
    combo: Option<Res<Combo>>,
    right_of_way: Option<Res<RightOfWayRun>>,
    ledger: Option<ResMut<V3LedgerState>>,
) {
    if !matches!(
        &*next,
        NextState::Pending(GameState::GameOver) | NextState::PendingIfNeq(GameState::GameOver)
    ) {
        return;
    }
    let (Some(rules), Some(clock), Some(objective), Some(combo), Some(mut ledger)) =
        (rules, clock, objective, combo, ledger)
    else {
        return;
    };
    if let Err(error) = finalize_terminal(
        &rules,
        &clock,
        *reason,
        &score,
        &timeleft,
        &objective,
        &combo,
        right_of_way.as_deref(),
        &mut ledger,
    ) {
        // Protocol arithmetic/size failure invalidates the run. Never enter a
        // GameOver state whose immutable snapshot lacks a canonical Terminal.
        ledger.failure = Some(error);
        next.set(GameState::Menu);
    }
}

fn pause_to_paused(
    keys: Res<ButtonInput<KeyCode>>,
    mut restart: ResMut<RestartRequested>,
    mut next: ResMut<NextState<GameState>>,
) {
    let action = if keys.just_pressed(KeyCode::Escape) {
        StateAction::Paused
    } else {
        StateAction::None
    };
    apply_state_action(action, &mut restart, &mut next);
}

fn pause_to_playing(
    keys: Res<ButtonInput<KeyCode>>,
    mut restart: ResMut<RestartRequested>,
    mut next: ResMut<NextState<GameState>>,
) {
    let action = match pause_decision(
        keys.just_pressed(KeyCode::Escape),
        keys.just_pressed(KeyCode::KeyR),
        keys.just_pressed(KeyCode::KeyQ),
    ) {
        PauseDecision::Resume => StateAction::Playing,
        PauseDecision::Restart => StateAction::Restart,
        PauseDecision::Menu => StateAction::Menu,
        PauseDecision::None => StateAction::None,
    };
    apply_state_action(action, &mut restart, &mut next);
}

fn gameover_input(
    keys: Res<ButtonInput<KeyCode>>,
    rules: Option<Res<ActiveRunRules>>,
    mut restart: ResMut<RestartRequested>,
    mut next: ResMut<NextState<GameState>>,
) {
    let action = if keys.just_pressed(KeyCode::Enter)
        || keys.just_pressed(KeyCode::Space)
        || keys.just_pressed(KeyCode::KeyR)
    {
        // A Ranked restart must discard the consumed receipt and return to
        // the menu boundary for a fresh capability/session/start chain.
        if rules.as_deref().is_some_and(ActiveRunRules::is_ranked) {
            StateAction::Menu
        } else {
            StateAction::Playing
        }
    } else if keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::KeyQ) {
        StateAction::Menu
    } else {
        StateAction::None
    };
    apply_state_action(action, &mut restart, &mut next);
}

#[cfg(test)]
mod tests {
    use super::{
        GameState, PauseDecision, RoundEntryDecision, StateAction, pause_decision,
        resolve_ordered_actions, round_entry_decision, state_action_transition,
        state_transitions_enabled,
    };

    #[test]
    fn settings_gate_allows_closed_and_blocks_open() {
        assert!(state_transitions_enabled(false));
        assert!(!state_transitions_enabled(true));
    }

    #[test]
    fn round_entry_is_fresh_only_while_inactive() {
        assert_eq!(round_entry_decision(false), RoundEntryDecision::Fresh);
        assert_eq!(round_entry_decision(true), RoundEntryDecision::Resume);
    }

    #[test]
    fn pause_keys_choose_resume_restart_or_menu() {
        assert_eq!(pause_decision(true, false, false), PauseDecision::Resume);
        assert_eq!(pause_decision(false, true, false), PauseDecision::Restart);
        assert_eq!(pause_decision(false, false, true), PauseDecision::Menu);
        assert_eq!(pause_decision(false, false, false), PauseDecision::None);
    }

    #[test]
    fn restart_wins_if_pause_keys_arrive_together() {
        assert_eq!(pause_decision(true, true, true), PauseDecision::Restart);
    }

    #[test]
    fn keyboard_restart_wins_over_simultaneous_touch_resume() {
        assert_eq!(
            resolve_ordered_actions(StateAction::Playing, StateAction::Restart),
            StateAction::Restart
        );
        assert_eq!(
            state_action_transition(StateAction::Restart),
            Some((GameState::Menu, true))
        );
        // Ordering, not cross-device action priority, is authoritative.
        assert_eq!(
            resolve_ordered_actions(StateAction::Restart, StateAction::Playing),
            StateAction::Playing
        );
    }

    #[test]
    fn later_non_restart_actions_clear_the_restart_latch() {
        assert_eq!(
            state_action_transition(StateAction::Playing),
            Some((GameState::Playing, false))
        );
        assert_eq!(
            state_action_transition(StateAction::Menu),
            Some((GameState::Menu, false))
        );
        assert_eq!(state_action_transition(StateAction::None), None);
    }
}
