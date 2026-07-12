//! Best-score persistence: `localStorage` on web, a tiny file on native.
//!
//! Loads the saved best at startup, updates it whenever the current run's
//! score beats it (persisting the new record immediately), and renders a
//! small "BEST: N" UI node in the bottom-left corner (away from the HUD,
//! which occupies the top-left panel and top-right timer).

use bevy::{prelude::*, text::FontSize};

use crate::game::resources::Score;
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
            .add_systems(
                Startup,
                (load_best_score, spawn_best_score_ui).chain(),
            )
            // Runs every frame in every state; cheap (one u32 add + compare)
            // and only writes to storage when a new record is set, so it
            // reliably catches the final score at its peak.
            .add_systems(Update, (update_best, update_best_score_text));
    }
}

/// Overwrite the `BestScore` resource with the value persisted on disk/web.
fn load_best_score(mut best: ResMut<BestScore>) {
    best.0 = load_best();
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

/// If the current run's total (chickens + coins) beats the stored best,
/// update the resource and persist the new record.
fn update_best(score: Res<Score>, mut best: ResMut<BestScore>) {
    let total = score.chickens + score.coins;
    if total > best.0 {
        best.0 = total;
        save_best(total);
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
// uses blocking `std::fs` (tiny file, fine at startup / on record updates).

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

/// Persist the new best score. Best-effort: silently ignores storage errors.
fn save_best(total: u32) {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(Some(storage)) = window.local_storage() else {
            return;
        };
        let _ = storage.set_item(STORAGE_KEY, &total.to_string());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = std::fs::write(FILE_PATH, total.to_string());
    }
}
