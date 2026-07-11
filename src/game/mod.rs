pub mod events;
pub mod resources;
pub mod state;

use bevy::prelude::*;

use crate::car::Car;
use crate::game::events::{ChickenHit, CoinCollected};
use crate::game::resources::{GameConfig, Score, TimeLeft};
use crate::game::state::GameState;

pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<GameState>()
            .init_resource::<GameConfig>()
            .init_resource::<Score>()
            .init_resource::<TimeLeft>()
            .add_message::<ChickenHit>()
            .add_message::<CoinCollected>()
            // Reset the run whenever (re)entering Playing.
            .add_systems(OnEnter(GameState::Playing), reset_run)
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
    mut car: Query<(&mut Car, &mut Transform)>,
) {
    *score = Score::default();
    timeleft.0 = 60.0;
    if let Ok((mut car, mut tf)) = car.single_mut() {
        car.speed = 0.0;
        car.heading = 0.0;
        tf.translation = Vec3::ZERO;
        tf.rotation = Quat::IDENTITY;
    }
}

fn tick_timeleft(
    mut t: ResMut<TimeLeft>,
    time: Res<Time>,
    mut next: ResMut<NextState<GameState>>,
) {
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
