use bevy::prelude::*;

/// Tunable gameplay constants (moved out of `const` so they can be adjusted at runtime).
#[derive(Resource)]
pub struct GameConfig {
    pub accel: f32,
    pub max_speed: f32,
    pub turn_rate: f32,
    pub drag: f32,
    pub cam_offset: Vec3,
    pub arena_min: f32,
    pub arena_max: f32,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            accel: 8.0,
            max_speed: 12.0,
            turn_rate: 2.5,
            drag: 0.8,
            cam_offset: Vec3::new(12.0, 12.0, 12.0),
            arena_min: -49.0,
            arena_max: 49.0,
        }
    }
}

/// Score for the current run: chickens hit + coins collected.
/// Total score = chickens + coins.
#[derive(Resource, Default)]
pub struct Score {
    pub chickens: u32,
    pub coins: u32,
}

/// Seconds remaining in the current timed round (counts down from 60.0).
#[derive(Resource)]
pub struct TimeLeft(pub f32);

impl Default for TimeLeft {
    fn default() -> Self {
        Self(60.0)
    }
}

/// True while a round is in progress (including while paused). Keeps
/// `reset_run` + `spawn_coins`/`spawn_chickens` from re-firing when resuming
/// from `Paused`, so pausing doesn't wipe the current run.
#[derive(Resource, Default)]
pub struct RoundActive(pub bool);

/// Why the round ended — drives the GameOver screen title. Defaults to
/// `TimeUp`; `health.rs` sets `Wrecked` when the car is destroyed.
#[derive(Resource, Default, Clone, Copy)]
pub enum GameOverReason {
    #[default]
    TimeUp,
    Wrecked,
}
