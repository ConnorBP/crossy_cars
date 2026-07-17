use bevy::prelude::*;

/// Tunable gameplay constants (moved out of `const` so they can be adjusted at runtime).
#[derive(Resource)]
pub struct GameConfig {
    pub max_speed: f32,
    pub turn_rate: f32,
    pub cam_offset: Vec3,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            max_speed: 12.0,
            turn_rate: 2.5,
            cam_offset: Vec3::new(12.0, 12.0, 12.0),
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

/// Persistent local pond-outcome latch. It remains active across Paused so
/// simulation and presentation resume at the exact same elapsed time.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct Drowning {
    pub active: bool,
    pub elapsed: f32,
    /// Player pose after the previous frame's complete collision resolution.
    pub previous_resolved_center: Vec2,
    pub previous_resolved_heading: f32,
    /// Player pose immediately after motion, before this frame's pushout.
    pub motion_end_center: Vec2,
    pub motion_end_heading: f32,
    /// Whether the previous resolved pose has been seeded for a real sweep.
    pub initialized: bool,
    /// Lets the camera consume the final resolved entry pose exactly once.
    pub camera_capture_pending: bool,
    pub entry_position: Vec3,
}

/// Run condition for ordinary interactions that stop at pond entry.
pub fn not_drowning(drowning: Res<Drowning>) -> bool {
    !drowning.active
}

/// Why the round ended — drives the GameOver screen title. Defaults to
/// `TimeUp`; `health.rs` sets `Wrecked` when the car is destroyed. `Drowned`
/// maps to the stable rules-v3 terminal discriminant for either conduct.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameOverReason {
    #[default]
    TimeUp,
    Wrecked,
    Drowned,
}
