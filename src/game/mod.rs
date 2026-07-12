pub mod events;
pub mod resources;
pub mod state;

use bevy::prelude::*;

use crate::car::{Car, InputFrozen};
use crate::game::events::{ChickenHit, CoinCollected, ObstacleHit};
use crate::game::resources::{GameConfig, RoundActive, Score, TimeLeft};
use crate::game::state::GameState;

/// System set grouping all fresh-round spawn systems that run on
/// `OnEnter(GameState::Playing)` and must execute BEFORE `reset_run` flips
/// `RoundActive` on. Later waves (T3 `spawn_chickens`, T6 `start_countdown`)
/// add their `OnEnter(Playing)` systems to this set so resume-from-Paused /
/// fresh-round gating stays correct (risk E11).
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
            .add_message::<ChickenHit>()
            .add_message::<CoinCollected>()
            .add_message::<ObstacleHit>()
            // Register the spawn-ordering set in the OnEnter(Playing) schedule
            // so fresh-round spawn systems (added by later waves into
            // SpawnSet) run before reset_run flips RoundActive.
            .configure_sets(OnEnter(GameState::Playing), SpawnSet)
            // Start a fresh round on entering Playing ONLY when coming from
            // Menu/GameOver (round inactive). On resume from Paused the round
            // is still active, so reset is skipped. reset_run runs AFTER all
            // SpawnSet systems so it can flip RoundActive on for the round.
            .add_systems(
                OnEnter(GameState::Playing),
                reset_run.after(SpawnSet),
            )
            // End the round (clear the active flag) when leaving for GameOver
            // or Menu; the world plugin despawns round entities on these too.
            .add_systems(OnEnter(GameState::GameOver), end_round)
            .add_systems(OnEnter(GameState::Menu), end_round)
            .add_systems(Update, tick_timeleft.run_if(in_state(GameState::Playing)))
            .add_systems(
                Update,
                menu_input.run_if(in_state(GameState::Menu)),
            )
            .add_systems(
                Update,
                pause_to_paused.run_if(in_state(GameState::Playing)),
            )
            .add_systems(
                Update,
                pause_to_playing.run_if(in_state(GameState::Paused)),
            )
            .add_systems(
                Update,
                gameover_input.run_if(in_state(GameState::GameOver)),
            );
    }
}

fn reset_run(
    mut score: ResMut<Score>,
    mut timeleft: ResMut<TimeLeft>,
    mut round_active: ResMut<RoundActive>,
    mut car: Query<(&mut Car, &mut Transform)>,
) {
    // Resuming from Paused: round already active -> keep score/time/coins.
    if round_active.0 {
        return;
    }
    *score = Score::default();
    timeleft.0 = 60.0;
    if let Ok((mut car, mut tf)) = car.single_mut() {
        car.speed = 0.0;
        car.heading = 0.0;
        tf.translation = Vec3::ZERO;
        tf.rotation = Quat::IDENTITY;
    }
    // Mark the round active so a later Paused->Playing resume won't reset.
    round_active.0 = true;
}

fn end_round(mut round_active: ResMut<RoundActive>) {
    round_active.0 = false;
}

fn tick_timeleft(
    mut t: ResMut<TimeLeft>,
    time: Res<Time>,
    mut next: ResMut<NextState<GameState>>,
    input_frozen: Res<InputFrozen>,
) {
    // Don't burn the 60s round timer while a countdown overlay is active.
    if input_frozen.0 {
        return;
    }
    t.0 -= time.delta_secs();
    if t.0 <= 0.0 {
        t.0 = 0.0;
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

fn pause_to_playing(keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<GameState>>) {
    if keys.just_pressed(KeyCode::Escape) {
        next.set(GameState::Playing);
    }
}

fn gameover_input(keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<GameState>>) {
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        next.set(GameState::Playing);
    } else if keys.just_pressed(KeyCode::Escape) {
        next.set(GameState::Menu);
    }
}
