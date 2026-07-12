//! Best-score persistence: `localStorage` on web, a tiny file on native.
//!
//! Loads the saved best at startup, updates the in-memory value whenever the
//! current run beats it, and persists a new record once when the round ends.
//! It also renders a small "BEST: N" UI node in the bottom-left corner (away
//! from the HUD, which occupies the top-left panel and top-right timer).

use bevy::{prelude::*, text::FontSize};

use crate::game::resources::Score;
use crate::game::state::GameState;
use crate::modifiers::{ActiveModifier, ModifierKind};
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

/// Best score for each road condition, indexed by [`ModifierKind::index`].
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConditionBests {
    pub by_kind: [u32; 5],
}

/// Per-condition records as they stood when the current round began. The
/// live `ConditionBests` resource updates during play, while this snapshot
/// lets Game Over report a strict condition record and medal upgrade.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConditionBestsAtRoundStart {
    pub by_kind: [u32; 5],
}

/// The persisted best as it stood when the current round began.
///
/// This snapshot lets the game-over UI identify a record even though
/// [`BestScore`] is updated continuously while the round is being played.
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
            .add_systems(Startup, (load_best_score, spawn_best_score_ui).chain())
            // Keep the live value current for the persistent BEST UI, but do
            // not touch storage in the per-frame path.
            .add_systems(Update, update_best.run_if(in_state(GameState::Playing)))
            .add_systems(Update, update_best_score_text)
            // Either terminal destination ends a round. Persisted snapshots
            // make GameOver -> Menu idempotent; paused restarts route via Menu.
            .add_systems(OnEnter(GameState::GameOver), persist_best_on_round_end)
            .add_systems(OnEnter(GameState::Menu), persist_best_on_round_end);
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

/// Apply a round total to both the global and active-condition records.
/// Returns whether either value strictly improved.
fn apply_total(snapshot: &mut BestsSnapshot, kind: ModifierKind, total: u32) -> bool {
    let previous = *snapshot;
    snapshot.global = snapshot.global.max(total);
    let condition = &mut snapshot.by_kind[kind.index()];
    *condition = (*condition).max(total);
    *snapshot != previous
}

fn current_snapshot(best: &BestScore, conditions: &ConditionBests) -> BestsSnapshot {
    BestsSnapshot {
        global: best.0,
        by_kind: conditions.by_kind,
    }
}

fn apply_to_resources(
    best: &mut BestScore,
    conditions: &mut ConditionBests,
    kind: ModifierKind,
    total: u32,
) {
    let mut snapshot = current_snapshot(best, conditions);
    apply_total(&mut snapshot, kind, total);
    best.0 = snapshot.global;
    conditions.by_kind = snapshot.by_kind;
}

/// If the current run's total (chickens + coins) beats either record, update
/// only memory so live UI remains current without per-hit I/O.
fn update_best(
    score: Res<Score>,
    active: Res<ActiveModifier>,
    mut best: ResMut<BestScore>,
    mut conditions: ResMut<ConditionBests>,
) {
    apply_to_resources(
        &mut best,
        &mut conditions,
        active.0,
        score.chickens + score.coins,
    );
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

/// Capture any final score update and persist a new record at a terminal round
/// boundary. Updating `persisted` only after a successful write permits a
/// failed GameOver write to retry on Menu while suppressing duplicate writes.
fn persist_best_on_round_end(
    score: Res<Score>,
    active: Res<ActiveModifier>,
    mut best: ResMut<BestScore>,
    mut conditions: ResMut<ConditionBests>,
    mut persisted_best: ResMut<PersistedBest>,
    mut persisted_conditions: ResMut<PersistedConditionBests>,
) {
    apply_to_resources(
        &mut best,
        &mut conditions,
        active.0,
        score.chickens + score.coins,
    );
    let current = current_snapshot(&best, &conditions);
    let persisted = BestsSnapshot {
        global: persisted_best.0,
        by_kind: persisted_conditions.by_kind,
    };
    if should_persist_bests(current, persisted) && save_bests(current) {
        persisted_best.0 = current.global;
        persisted_conditions.by_kind = current.by_kind;
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
    fn update_math_tracks_global_and_only_the_active_condition() {
        let mut bests = BestsSnapshot {
            global: 50,
            by_kind: [10, 20, 30, 40, 5],
        };
        assert!(apply_total(&mut bests, ModifierKind::GlassCannon, 60));
        assert_eq!(bests.global, 60);
        assert_eq!(bests.by_kind, [10, 20, 30, 40, 60]);

        // A condition may improve even when the all-condition global does not.
        assert!(apply_total(&mut bests, ModifierKind::Standard, 45));
        assert_eq!(bests.global, 60);
        assert_eq!(bests.by_kind, [45, 20, 30, 40, 60]);
        assert!(!apply_total(&mut bests, ModifierKind::Standard, 44));
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
