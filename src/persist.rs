//! Best-score persistence: `localStorage` on web, a tiny file on native.
//!
//! Loads the saved best at startup, updates the in-memory value whenever the
//! current run beats it, and persists a new record once when the round ends.
//! It also renders a small "BEST: N" UI node in the bottom-left corner (away
//! from the HUD, which occupies the top-left panel and top-right timer).

use bevy::{prelude::*, text::FontSize};

use crate::game::resources::Score;
use crate::game::state::GameState;
use crate::palette;

/// localStorage key (web).
#[cfg(target_arch = "wasm32")]
const STORAGE_KEY: &str = "car_game_best";
/// Native best-score file written next to the executable.
#[cfg(not(target_arch = "wasm32"))]
const FILE_PATH: &str = "best_score.txt";

/// The best score seen across runs. Loaded from storage at startup.
#[derive(Resource, Default)]
pub struct BestScore(pub u32);

/// The persisted best as it stood when the current round began.
///
/// This snapshot lets the game-over UI identify a record even though
/// [`BestScore`] is updated continuously while the round is being played.
#[derive(Resource, Default)]
pub struct BestAtRoundStart(pub u32);

/// Last value successfully loaded from or written to storage. Keeping this
/// separate from `BestAtRoundStart` avoids a duplicate write on
/// GameOver -> Menu without changing the snapshot used by the game-over UI.
#[derive(Resource, Default)]
struct PersistedBest(u32);

/// Marker for the best-score UI root node (never despawned).
#[derive(Component)]
struct BestScoreRoot;
/// Marker for the dynamic number span inside the best-score text.
#[derive(Component)]
struct BestScoreText;

pub struct PersistPlugin;

impl Plugin for PersistPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BestScore>()
            .init_resource::<BestAtRoundStart>()
            .init_resource::<PersistedBest>()
            .add_systems(Startup, (load_best_score, spawn_best_score_ui).chain())
            // Keep the live value current for the persistent BEST UI, but do
            // not touch storage in the per-frame path.
            .add_systems(Update, update_best.run_if(in_state(GameState::Playing)))
            .add_systems(Update, update_best_score_text)
            // Either terminal destination ends a round. PersistedBest makes
            // GameOver -> Menu idempotent; paused restarts also route via Menu.
            .add_systems(OnEnter(GameState::GameOver), persist_best_on_round_end)
            .add_systems(OnEnter(GameState::Menu), persist_best_on_round_end);
    }
}

/// Overwrite the `BestScore` resource with the value persisted on disk/web.
fn load_best_score(mut best: ResMut<BestScore>, mut persisted: ResMut<PersistedBest>) {
    let loaded = load_best();
    best.0 = loaded;
    persisted.0 = loaded;
}

/// Spawn the "BEST: N" UI node (bottom-left, absolute). Lives for the whole
/// app lifetime; the number span is refreshed each frame by
/// `update_best_score_text`.
fn spawn_best_score_ui(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: px(12.0),
                left: px(14.0),
                padding: UiRect::all(px(8.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.35)),
            BestScoreRoot,
            Text::new("BEST: "),
            TextFont {
                font_size: FontSize::Px(20.0),
                ..default()
            },
            TextColor(palette::HUD_TEXT.into()),
        ))
        .with_child((
            TextSpan::default(),
            TextFont {
                font_size: FontSize::Px(20.0),
                ..default()
            },
            TextColor(palette::HUD_ACCENT.into()),
            BestScoreText,
        ));
}

/// If the current run's total (chickens + coins) beats the best, update only
/// the in-memory resource so live UI remains current without per-hit I/O.
fn update_best(score: Res<Score>, mut best: ResMut<BestScore>) {
    best.0 = best.0.max(score.chickens + score.coins);
}

/// A round should write only a strict improvement over both its starting
/// record and the value already known to be in storage.
fn should_persist_best(current_best: u32, best_at_round_start: u32, persisted_best: u32) -> bool {
    current_best > best_at_round_start && current_best > persisted_best
}

/// Capture any final score update and persist a new record at a terminal round
/// boundary. Updating `persisted` only after a successful write permits a
/// failed GameOver write to retry on Menu while suppressing duplicate writes.
fn persist_best_on_round_end(
    score: Res<Score>,
    mut best: ResMut<BestScore>,
    best_at_start: Res<BestAtRoundStart>,
    mut persisted: ResMut<PersistedBest>,
) {
    best.0 = best.0.max(score.chickens + score.coins);
    if should_persist_best(best.0, best_at_start.0, persisted.0) && save_best(best.0) {
        persisted.0 = best.0;
    }
}

/// Refresh the "BEST: N" number span each frame.
fn update_best_score_text(
    best: Res<BestScore>,
    mut query: Query<&mut TextSpan, With<BestScoreText>>,
) {
    for mut span in &mut query {
        **span = format!("{}", best.0);
    }
}

// --- platform-specific storage helpers ---
// All `web_sys::window()` usage is gated behind `target_arch = "wasm32"`; native
// uses blocking `std::fs` (tiny file, only used at startup / round end).

/// Read the persisted best score. Returns 0 if missing/unreadable.
fn load_best() -> u32 {
    #[cfg(target_arch = "wasm32")]
    {
        // `window()` -> Option<Window>; `local_storage()` -> Result<Option<Storage>, _>.
        let Some(window) = web_sys::window() else {
            return 0;
        };
        let Ok(Some(storage)) = window.local_storage() else {
            return 0;
        };
        // `get_item` -> Result<Option<String>, _>; flatten the Option<String>.
        storage
            .get_item(STORAGE_KEY)
            .ok()
            .flatten()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(0)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::read_to_string(FILE_PATH)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0)
    }
}

/// Persist the new best score. Returns whether storage accepted the write.
fn save_best(total: u32) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return false;
        };
        let Ok(Some(storage)) = window.local_storage() else {
            return false;
        };
        storage.set_item(STORAGE_KEY, &total.to_string()).is_ok()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::write(FILE_PATH, total.to_string()).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::should_persist_best;

    #[test]
    fn persistence_requires_a_strict_round_improvement() {
        assert!(should_persist_best(11, 10, 10));
        assert!(!should_persist_best(10, 10, 9));
        assert!(!should_persist_best(9, 10, 9));
    }

    #[test]
    fn already_persisted_round_best_is_not_written_again() {
        assert!(!should_persist_best(11, 10, 11));
        assert!(!should_persist_best(11, 10, 12));
    }
}
