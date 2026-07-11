use bevy::prelude::*;

#[derive(States, Clone, Copy, Default, Eq, PartialEq, Hash, Debug)]
pub enum GameState {
    #[default]
    Menu,
    Playing,
    Paused,
    GameOver,
}
