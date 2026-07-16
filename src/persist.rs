//! Best-score persistence: `localStorage` on web, a tiny file on native.
//!
//! Loads the saved best at startup and records only terminal scores from
//! completed rounds. In-progress peaks and abandoned rounds never become
//! records. It also renders a small "BEST: N" UI node in the bottom-left
//! corner (away from the HUD, which occupies the top-left panel and top-right
//! timer).

use bevy::{prelude::*, text::FontSize};

use crate::game::resources::Score;
use crate::game::state::GameState;
use crate::modifiers::{ActiveModifier, ModifierKind};
use crate::palette;
use crate::touch::TouchControlsActive;

/// localStorage key (web).
#[cfg(target_arch = "wasm32")]
const STORAGE_KEY: &str = "car_game_best";
/// Native best-score file written next to the executable.
#[cfg(not(target_arch = "wasm32"))]
const FILE_PATH: &str = "best_score.txt";

/// Best terminal score from completed rounds. Loaded from storage at startup
/// and changed only after a completed-round snapshot is successfully saved.
#[derive(Resource, Default)]
pub struct BestScore(pub u32);

/// Best completed-round terminal score for each road condition, indexed by
/// [`ModifierKind::index`].
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConditionBests {
    pub by_kind: [u32; 5],
}

/// Per-condition records as they stood when the current round began. This
/// lets Game Over report a strict condition record and medal upgrade without
/// depending on persistence-system ordering.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConditionBestsAtRoundStart {
    pub by_kind: [u32; 5],
}

/// The persisted best as it stood when the current round began.
///
/// [`BestScore`] remains unchanged during play; this explicit snapshot also
/// makes the game-over presentation independent of OnEnter system ordering.
#[derive(Resource, Default)]
pub struct BestAtRoundStart(pub u32);

/// Complete representation stored under the legacy best-score key/file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BestsSnapshot {
    global: u32,
    by_kind: [u32; 5],
}

/// Last global value successfully loaded from or written to storage. Keeping
/// this separate from `BestAtRoundStart` avoids a duplicate write on
/// GameOver -> Menu without changing the snapshot used by the game-over UI.
#[derive(Resource, Default)]
struct PersistedBest(u32);

/// Last condition values successfully loaded from or written to storage.
#[derive(Resource, Default)]
struct PersistedConditionBests {
    by_kind: [u32; 5],
}

/// A completed-round snapshot whose write failed. Menu entry may retry this
/// exact snapshot, but never derives a record from the current `Score`; this
/// keeps startup and paused/abandoned Menu transitions harmless.
#[derive(Resource, Default)]
struct PendingBests(Option<BestsSnapshot>);

/// Medal earned for a condition-specific best score.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Medal {
    #[default]
    None,
    Bronze,
    Silver,
    Gold,
}

impl Medal {
    /// Stable player-facing medal label.
    pub const fn display_label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Bronze => "Bronze",
            Self::Silver => "Silver",
            Self::Gold => "Gold",
        }
    }

    /// Concise alias for presentation callers.
    pub const fn label(self) -> &'static str {
        self.display_label()
    }
}

impl std::fmt::Display for Medal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.display_label())
    }
}

/// Calculate the medal for a road condition's best score.
pub const fn medal_for(kind: ModifierKind, best: u32) -> Medal {
    let (bronze, silver, gold) = match kind {
        ModifierKind::Standard => (20, 40, 70),
        ModifierKind::RushHour => (15, 30, 55),
        ModifierKind::ChickenFrenzy => (35, 65, 100),
        ModifierKind::Stampede => (15, 25, 45),
        ModifierKind::GlassCannon => (25, 50, 80),
    };
    if best >= gold {
        Medal::Gold
    } else if best >= silver {
        Medal::Silver
    } else if best >= bronze {
        Medal::Bronze
    } else {
        Medal::None
    }
}

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
            .init_resource::<ConditionBests>()
            .init_resource::<ConditionBestsAtRoundStart>()
            .init_resource::<BestAtRoundStart>()
            .init_resource::<PersistedBest>()
            .init_resource::<PersistedConditionBests>()
            .init_resource::<PendingBests>()
            .add_systems(Startup, (load_best_score, spawn_best_score_ui).chain())
            .add_systems(
                Update,
                (update_best_score_text, update_best_score_visibility),
            )
            // Only GameOver completes a round. Menu entry (including startup
            // and paused restart/quit) may retry a failed completed-round
            // write, but never applies an in-progress score.
            .add_systems(OnEnter(GameState::GameOver), persist_best_on_round_end)
            .add_systems(OnEnter(GameState::Menu), retry_pending_best_write);
    }
}

/// Overwrite the in-memory best resources with the values from disk/web.
fn load_best_score(
    mut best: ResMut<BestScore>,
    mut conditions: ResMut<ConditionBests>,
    mut persisted_best: ResMut<PersistedBest>,
    mut persisted_conditions: ResMut<PersistedConditionBests>,
) {
    let loaded = load_bests();
    best.0 = loaded.global;
    conditions.by_kind = loaded.by_kind;
    persisted_best.0 = loaded.global;
    persisted_conditions.by_kind = loaded.by_kind;
}

/// Spawn the "BEST: N" UI node (bottom-left, absolute). Lives for the whole
/// app lifetime; the number span is refreshed each frame by
/// `update_best_score_text`.
const fn best_score_visible(state: GameState, touch_active: bool) -> bool {
    !matches!(state, GameState::Menu)
        && !(touch_active && matches!(state, GameState::Playing | GameState::Paused))
}

fn update_best_score_visibility(
    state: Res<State<GameState>>,
    touch: Res<TouchControlsActive>,
    mut roots: Query<&mut Visibility, With<BestScoreRoot>>,
) {
    let visibility = if best_score_visible(*state.get(), touch.0) {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    for mut current in &mut roots {
        *current = visibility;
    }
}

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

/// Return the records produced by one completed round's terminal total.
/// Gameplay peaks are intentionally not an input: only the final total can
/// improve the global and active-condition records.
fn with_terminal_total(
    mut snapshot: BestsSnapshot,
    kind: ModifierKind,
    terminal_total: u32,
) -> BestsSnapshot {
    snapshot.global = snapshot.global.max(terminal_total);
    let condition = &mut snapshot.by_kind[kind.index()];
    *condition = (*condition).max(terminal_total);
    snapshot
}

fn current_snapshot(best: &BestScore, conditions: &ConditionBests) -> BestsSnapshot {
    BestsSnapshot {
        global: best.0,
        by_kind: conditions.by_kind,
    }
}

/// A round writes if any component strictly improves over the last snapshot
/// successfully loaded from or written to storage.
fn should_persist_bests(current: BestsSnapshot, persisted: BestsSnapshot) -> bool {
    current.global > persisted.global
        || current
            .by_kind
            .iter()
            .zip(persisted.by_kind)
            .any(|(current, persisted)| *current > persisted)
}

/// Try the pending completed-round write. The persisted mirrors and public
/// record resources advance only after success, making retries idempotent and
/// preserving a failed write for a later Menu transition.
fn flush_pending_best_write(
    pending: &mut PendingBests,
    best: &mut BestScore,
    conditions: &mut ConditionBests,
    persisted_best: &mut PersistedBest,
    persisted_conditions: &mut PersistedConditionBests,
) {
    let Some(snapshot) = pending.0 else {
        return;
    };
    let persisted = BestsSnapshot {
        global: persisted_best.0,
        by_kind: persisted_conditions.by_kind,
    };
    if !should_persist_bests(snapshot, persisted) {
        pending.0 = None;
        return;
    }
    if save_bests(snapshot) {
        persisted_best.0 = snapshot.global;
        persisted_conditions.by_kind = snapshot.by_kind;
        best.0 = snapshot.global;
        conditions.by_kind = snapshot.by_kind;
        pending.0 = None;
    }
}

/// Apply the terminal score of a completed round exactly once to the pending
/// snapshot, then persist the resulting versioned schema. This system is
/// registered only for GameOver; entering Menu abandons an active round and
/// cannot create records.
fn persist_best_on_round_end(
    score: Res<Score>,
    reason: Res<crate::game::resources::GameOverReason>,
    active: Res<ActiveModifier>,
    mut best: ResMut<BestScore>,
    mut conditions: ResMut<ConditionBests>,
    mut pending: ResMut<PendingBests>,
    mut persisted_best: ResMut<PersistedBest>,
    mut persisted_conditions: ResMut<PersistedConditionBests>,
) {
    // Local pond outcomes are explicitly ineligible for records just as they
    // are ineligible for leaderboard submission.
    if *reason == crate::game::resources::GameOverReason::Drowned {
        return;
    }
    // Preserve an earlier failed completed-round result when a player starts
    // another round directly from Game Over. A later terminal score can only
    // add to that pending snapshot, never replace it with a lower record.
    let base = pending
        .0
        .unwrap_or_else(|| current_snapshot(&best, &conditions));
    let completed = with_terminal_total(base, active.0, score.chickens + score.coins);
    let persisted = BestsSnapshot {
        global: persisted_best.0,
        by_kind: persisted_conditions.by_kind,
    };
    if should_persist_bests(completed, persisted) {
        pending.0 = Some(completed);
    }
    flush_pending_best_write(
        &mut pending,
        &mut best,
        &mut conditions,
        &mut persisted_best,
        &mut persisted_conditions,
    );
}

/// Menu is not a completed-round boundary. It can only retry a snapshot that
/// was already computed at GameOver and failed to reach storage.
fn retry_pending_best_write(
    mut pending: ResMut<PendingBests>,
    mut best: ResMut<BestScore>,
    mut conditions: ResMut<ConditionBests>,
    mut persisted_best: ResMut<PersistedBest>,
    mut persisted_conditions: ResMut<PersistedConditionBests>,
) {
    flush_pending_best_write(
        &mut pending,
        &mut best,
        &mut conditions,
        &mut persisted_best,
        &mut persisted_conditions,
    );
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

fn encode_bests(snapshot: BestsSnapshot) -> String {
    format!(
        "v1:{}:{}:{}:{}:{}:{}",
        snapshot.global,
        snapshot.by_kind[0],
        snapshot.by_kind[1],
        snapshot.by_kind[2],
        snapshot.by_kind[3],
        snapshot.by_kind[4],
    )
}

/// Parse the current schema, or migrate the old bare global-best value.
fn decode_bests(value: &str) -> BestsSnapshot {
    let value = value.trim();
    if let Ok(global) = value.parse::<u32>() {
        return BestsSnapshot {
            global,
            ..default()
        };
    }

    let mut fields = value.split(':');
    if fields.next() != Some("v1") {
        return BestsSnapshot::default();
    }
    let Some(global) = fields.next().and_then(|field| field.parse().ok()) else {
        return BestsSnapshot::default();
    };
    let mut by_kind = [0; 5];
    for condition in &mut by_kind {
        let Some(value) = fields.next().and_then(|field| field.parse().ok()) else {
            return BestsSnapshot::default();
        };
        *condition = value;
    }
    if fields.next().is_some() {
        return BestsSnapshot::default();
    }
    BestsSnapshot { global, by_kind }
}

/// Read all persisted bests. Missing, unreadable, or invalid data is defaulted.
fn load_bests() -> BestsSnapshot {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return BestsSnapshot::default();
        };
        let Ok(Some(storage)) = window.local_storage() else {
            return BestsSnapshot::default();
        };
        storage
            .get_item(STORAGE_KEY)
            .ok()
            .flatten()
            .map(|value| decode_bests(&value))
            .unwrap_or_default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::read_to_string(FILE_PATH)
            .ok()
            .map(|value| decode_bests(&value))
            .unwrap_or_default()
    }
}

/// Persist the complete schema. Returns whether storage accepted the write.
fn save_bests(snapshot: BestsSnapshot) -> bool {
    let encoded = encode_bests(snapshot);
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return false;
        };
        let Ok(Some(storage)) = window.local_storage() else {
            return false;
        };
        storage.set_item(STORAGE_KEY, &encoded).is_ok()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        atomic_write_native(FILE_PATH, encoded.as_bytes())
    }
}

/// Write beside the destination, then replace it. Some platforms do not let
/// `rename` replace an existing file, so retry after removing the destination.
/// Every failure path makes a best effort to clean up the temporary file.
#[cfg(not(target_arch = "wasm32"))]
fn atomic_write_native(path: &str, bytes: &[u8]) -> bool {
    use std::io::Write;
    use std::path::Path;

    let destination = Path::new(path);
    let temporary = destination.with_extension("tmp");
    let write_result = std::fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return false;
    }

    if std::fs::rename(&temporary, destination).is_ok() {
        return true;
    }

    // Safe fallback for platforms where rename cannot replace an existing
    // destination. Move the known-good save aside rather than deleting it,
    // and restore it if installing the temporary file fails.
    if !destination.exists() {
        let _ = std::fs::remove_file(&temporary);
        return false;
    }
    let backup = destination.with_extension("bak");
    if backup.exists() && std::fs::remove_file(&backup).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return false;
    }
    if std::fs::rename(destination, &backup).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return false;
    }
    if std::fs::rename(&temporary, destination).is_ok() {
        let _ = std::fs::remove_file(&backup);
        true
    } else {
        // The old data remains valid in `backup` even if restoration itself
        // encounters an exceptional filesystem error.
        let _ = std::fs::rename(&backup, destination);
        let _ = std::fs::remove_file(&temporary);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ModifierKind; 5] = [
        ModifierKind::Standard,
        ModifierKind::RushHour,
        ModifierKind::ChickenFrenzy,
        ModifierKind::Stampede,
        ModifierKind::GlassCannon,
    ];

    #[test]
    fn legacy_global_best_migrates_without_condition_records() {
        assert_eq!(
            decode_bests(" 42\n"),
            BestsSnapshot {
                global: 42,
                by_kind: [0; 5],
            }
        );
    }

    #[test]
    fn v1_format_has_exact_order_and_round_trips() {
        let snapshot = BestsSnapshot {
            global: 99,
            by_kind: [10, 20, 30, 40, 50],
        };
        let encoded = encode_bests(snapshot);
        assert_eq!(encoded, "v1:99:10:20:30:40:50");
        assert_eq!(decode_bests(&encoded), snapshot);
    }

    #[test]
    fn corrupt_and_unsupported_values_default_completely() {
        for value in [
            "",
            "garbage",
            "v2:1:2:3:4:5:6",
            "v1:1:2:3:4:5",
            "v1:1:2:3:4:5:6:7",
            "v1:1:2:x:4:5:6",
            "-1",
        ] {
            assert_eq!(decode_bests(value), BestsSnapshot::default(), "{value}");
        }
    }

    #[test]
    fn medals_cover_every_threshold_boundary() {
        let thresholds = [
            (20, 40, 70),
            (15, 30, 55),
            (35, 65, 100),
            (15, 25, 45),
            (25, 50, 80),
        ];
        for (kind, (bronze, silver, gold)) in ALL.into_iter().zip(thresholds) {
            assert_eq!(medal_for(kind, bronze - 1), Medal::None);
            assert_eq!(medal_for(kind, bronze), Medal::Bronze);
            assert_eq!(medal_for(kind, silver - 1), Medal::Bronze);
            assert_eq!(medal_for(kind, silver), Medal::Silver);
            assert_eq!(medal_for(kind, gold - 1), Medal::Silver);
            assert_eq!(medal_for(kind, gold), Medal::Gold);
            assert_eq!(medal_for(kind, gold + 1), Medal::Gold);
        }
        assert_eq!(Medal::None.display_label(), "None");
        assert_eq!(Medal::Bronze.to_string(), "Bronze");
        assert_eq!(Medal::Silver.display_label(), "Silver");
        assert_eq!(Medal::Gold.to_string(), "Gold");
    }

    #[test]
    fn terminal_update_tracks_global_and_only_the_active_condition() {
        let bests = BestsSnapshot {
            global: 50,
            by_kind: [10, 20, 30, 40, 5],
        };
        let bests = with_terminal_total(bests, ModifierKind::GlassCannon, 60);
        assert_eq!(bests.global, 60);
        assert_eq!(bests.by_kind, [10, 20, 30, 40, 60]);

        // A condition may improve even when the all-condition global does not.
        let bests = with_terminal_total(bests, ModifierKind::Standard, 45);
        assert_eq!(bests.global, 60);
        assert_eq!(bests.by_kind, [45, 20, 30, 40, 60]);

        // Lower and equal terminal totals are not record improvements.
        assert_eq!(
            with_terminal_total(bests, ModifierKind::Standard, 44),
            bests
        );
        assert_eq!(
            with_terminal_total(bests, ModifierKind::Standard, 45),
            bests
        );
    }

    #[test]
    fn peak_during_play_is_irrelevant_and_terminal_total_is_recorded() {
        let start = BestsSnapshot::default();
        let peak_during_play = 20;
        let terminal_total = 18;

        // Only terminal_total is passed to record calculation.
        let completed = with_terminal_total(start, ModifierKind::Standard, terminal_total);
        assert_eq!(completed.global, 18);
        assert_eq!(completed.by_kind[ModifierKind::Standard.index()], 18);
        assert_ne!(completed.global, peak_during_play);
    }

    #[test]
    fn touch_best_score_visibility_avoids_playing_and_pause_controls() {
        assert!(!best_score_visible(GameState::Menu, false));
        for state in [GameState::Playing, GameState::Paused, GameState::GameOver] {
            assert!(best_score_visible(state, false), "desktop {state:?}");
        }
        assert!(!best_score_visible(GameState::Menu, true));
        assert!(!best_score_visible(GameState::Playing, true));
        assert!(!best_score_visible(GameState::Paused, true));
        assert!(best_score_visible(GameState::GameOver, true));
    }

    #[test]
    fn persistence_requires_improvement_and_is_idempotent_after_success() {
        let original = BestsSnapshot {
            global: 70,
            by_kind: [70, 20, 30, 40, 50],
        };
        assert!(!should_persist_bests(original, original));

        let mut current = original;
        current.by_kind[ModifierKind::RushHour.index()] = 21;
        assert!(should_persist_bests(current, original));

        // Mirroring a successful write suppresses GameOver -> Menu retries.
        let persisted_after_success = current;
        assert!(!should_persist_bests(current, persisted_after_success));

        // A failed write leaves the old mirror and therefore remains retryable.
        assert!(should_persist_bests(current, original));
    }
}
