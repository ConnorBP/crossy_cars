pub mod events;
pub mod resources;
pub mod state;

use bevy::prelude::*;

use crate::car::{Car, InputFrozen};
use crate::game::events::{ChickenHit, CoinCollected, ObstacleHit};
use crate::game::resources::{GameConfig, GameOverReason, RoundActive, Score, TimeLeft};
use crate::game::state::GameState;
use crate::persist::{BestAtRoundStart, BestScore, ConditionBests, ConditionBestsAtRoundStart};

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

pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<GameState>()
            .init_resource::<GameConfig>()
            .init_resource::<Score>()
            .init_resource::<TimeLeft>()
            .init_resource::<RoundActive>()
            .init_resource::<InputFrozen>()
            .init_resource::<GameOverReason>()
            .init_resource::<RestartRequested>()
            .add_message::<ChickenHit>()
            .add_message::<CoinCollected>()
            .add_message::<ObstacleHit>()
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
            .add_systems(OnEnter(GameState::GameOver), end_round)
            // A paused restart deliberately visits Menu so all existing
            // end-round and cleanup systems run before a fresh Playing enter.
            .add_systems(
                OnEnter(GameState::Menu),
                (end_round, consume_restart_request).chain(),
            )
            .add_systems(Update, tick_timeleft.run_if(in_state(GameState::Playing)))
            .add_systems(Update, menu_input.run_if(in_state(GameState::Menu)))
            .add_systems(Update, pause_to_paused.run_if(in_state(GameState::Playing)))
            .add_systems(Update, pause_to_playing.run_if(in_state(GameState::Paused)))
            .add_systems(Update, gameover_input.run_if(in_state(GameState::GameOver)));
    }
}

fn reset_car_and_resources(
    mut score: ResMut<Score>,
    mut timeleft: ResMut<TimeLeft>,
    round_active: Res<RoundActive>,
    best: Res<BestScore>,
    condition_bests: Res<ConditionBests>,
    mut best_at_start: ResMut<BestAtRoundStart>,
    mut condition_bests_at_start: ResMut<ConditionBestsAtRoundStart>,
    mut car: Query<(&mut Car, &mut Transform)>,
) {
    // Resuming from Paused preserves the entire run, including both original
    // best-score snapshots and the car's exact state.
    if round_entry_decision(round_active.0) == RoundEntryDecision::Resume {
        return;
    }

    best_at_start.0 = best.0;
    condition_bests_at_start.by_kind = condition_bests.by_kind;
    *score = Score::default();
    *timeleft = TimeLeft::default();
    if let Ok((mut car, mut tf)) = car.single_mut() {
        car.speed = 0.0;
        car.heading = 0.0;
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
    mut reason: ResMut<GameOverReason>,
) {
    // Don't burn the 60s round timer while a countdown overlay is active.
    if input_frozen.0 {
        return;
    }
    t.0 -= time.delta_secs();
    if t.0 <= 0.0 {
        t.0 = 0.0;
        *reason = GameOverReason::TimeUp;
        next.set(GameState::GameOver);
    }
}

fn menu_input(keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<GameState>>) {
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        next.set(GameState::Playing);
    }
}

fn pause_to_paused(keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<GameState>>) {
    if keys.just_pressed(KeyCode::Escape) {
        next.set(GameState::Paused);
    }
}

fn pause_to_playing(
    keys: Res<ButtonInput<KeyCode>>,
    mut restart: ResMut<RestartRequested>,
    mut next: ResMut<NextState<GameState>>,
) {
    match pause_decision(
        keys.just_pressed(KeyCode::Escape),
        keys.just_pressed(KeyCode::KeyR),
        keys.just_pressed(KeyCode::KeyQ),
    ) {
        PauseDecision::Resume => next.set(GameState::Playing),
        PauseDecision::Restart => {
            restart.0 = true;
            next.set(GameState::Menu);
        }
        PauseDecision::Menu => next.set(GameState::Menu),
        PauseDecision::None => {}
    }
}

fn gameover_input(keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<GameState>>) {
    if keys.just_pressed(KeyCode::Enter)
        || keys.just_pressed(KeyCode::Space)
        || keys.just_pressed(KeyCode::KeyR)
    {
        next.set(GameState::Playing);
    } else if keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::KeyQ) {
        next.set(GameState::Menu);
    }
}

#[cfg(test)]
mod tests {
    use super::{PauseDecision, RoundEntryDecision, pause_decision, round_entry_decision};

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
}
