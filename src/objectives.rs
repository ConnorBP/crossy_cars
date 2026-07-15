//! Deterministic, per-round bonus objectives.
//!
//! Objectives are deliberately process-local: they are selected for each fresh
//! round, survive a pause/resume, and are never persisted. Gameplay messages
//! are consumed in `PostUpdate`, after their `Update` producers and before the
//! next state transition is applied; Bevy messages keep each reader's cursor
//! independent, so no hit is consumed by another feature.

use bevy::prelude::*;
use bevy::text::FontSize;
use bevy::window::PrimaryWindow;

use crate::combos::Combo;
use crate::game::SpawnSet;
use crate::game::TouchStateSet;
use crate::game::events::{ChickenHit, CoinCollected};
use crate::game::resources::{RoundActive, Score};
use crate::game::state::GameState;
use crate::modifiers::{ActiveModifier, ModifierKind};
use crate::touch::{
    TOUCH_OBJECTIVE_HEIGHT, TOUCH_OBJECTIVE_TOP, TOUCH_OBJECTIVE_WIDTH, TouchControlsActive,
    is_touch_portrait, touch_objective_bounds,
};

/// Score added once when the current objective is completed.
pub const OBJECTIVE_BONUS: u32 = roady_score_rules::OBJECTIVE_BONUS;

/// The task assigned to the current round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObjectiveKind {
    HitChickens { target: u32 },
    CollectCoins { target: u32 },
    ReachCombo { target: u32 },
}

impl ObjectiveKind {
    /// Stable player-facing label, suitable for HUD and Game Over summaries.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::HitChickens { .. } => "Hit chickens",
            Self::CollectCoins { .. } => "Collect coins",
            Self::ReachCombo { .. } => "Reach combo",
        }
    }

    /// Completion threshold carried by this objective.
    pub const fn target(self) -> u32 {
        match self {
            Self::HitChickens { target }
            | Self::CollectCoins { target }
            | Self::ReachCombo { target } => target,
        }
    }

    /// Compact progress summary for presentation code.
    pub fn summary(self, progress: u32) -> String {
        format!(
            "{} {}/{}",
            self.display_name(),
            progress.min(self.target()),
            self.target()
        )
    }
}

/// Live state for the single objective assigned to a round.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveObjective {
    pub kind: ObjectiveKind,
    pub progress: u32,
    pub completed: bool,
    pub reward_awarded: bool,
}

impl ActiveObjective {
    pub const fn new(kind: ObjectiveKind) -> Self {
        Self {
            kind,
            progress: 0,
            completed: false,
            reward_awarded: false,
        }
    }

    /// Compact terminal/current summary for later UI consumers.
    pub fn summary(&self) -> String {
        let base = self.kind.summary(self.progress);
        if self.completed {
            format!("{base} | COMPLETE +{OBJECTIVE_BONUS}")
        } else {
            base
        }
    }
}

impl Default for ActiveObjective {
    fn default() -> Self {
        Self::new(ObjectiveKind::HitChickens { target: 10 })
    }
}

/// Number of fresh objective selections made in this process.
///
/// This index is intentionally independent of `modifiers::RoundIndex`: both
/// plugins may increment their own counters during entry without depending on
/// system ordering or observing the other's post-increment value.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjectiveRoundIndex(pub u64);

/// Emitted on the incomplete -> complete edge, exactly once per objective.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectiveCompleted {
    pub kind: ObjectiveKind,
}

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ObjectiveSelectionSet;

pub(crate) fn mission_action(kind: ObjectiveKind) -> String {
    match kind {
        ObjectiveKind::HitChickens { target } => format!("Hit {target} chickens"),
        ObjectiveKind::CollectCoins { target } => format!("Collect {target} coins"),
        ObjectiveKind::ReachCombo { target } => format!("Reach a {target}x combo"),
    }
}

pub(crate) fn mission_announcement(kind: ObjectiveKind) -> String {
    format!(
        "ROUND MISSION\n{}\nComplete once: +{OBJECTIVE_BONUS} bonus",
        mission_action(kind)
    )
}

#[derive(Component)]
pub(crate) struct ObjectiveHudRoot;

#[derive(Component)]
struct ObjectiveHudText;

const fn objective_kind_for_round(index: u64, modifier: ModifierKind) -> ObjectiveKind {
    match index % 3 {
        0 => ObjectiveKind::HitChickens {
            target: if matches!(modifier, ModifierKind::ChickenFrenzy) {
                20
            } else {
                10
            },
        },
        1 => ObjectiveKind::CollectCoins {
            target: if matches!(modifier, ModifierKind::RushHour) {
                8
            } else {
                6
            },
        },
        _ => ObjectiveKind::ReachCombo {
            // Glass Cannon is the combo-focused condition, so its objective
            // asks for the next meaningful multiplier tier.
            target: if matches!(modifier, ModifierKind::GlassCannon) {
                4
            } else {
                3
            },
        },
    }
}

/// Pure fresh-entry decision. `None` is a pause resume and preserves all state.
fn fresh_objective_selection(
    round_active: bool,
    index: u64,
    modifier: ModifierKind,
) -> Option<(ActiveObjective, u64)> {
    if round_active {
        None
    } else {
        Some((
            ActiveObjective::new(objective_kind_for_round(index, modifier)),
            index
                .checked_add(1)
                .expect("objective round index exhausted its u64 range"),
        ))
    }
}

/// Inputs collected by the Bevy-facing message readers for one update.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ObjectiveProgressInput {
    chicken_hits: u32,
    coins_collected: u32,
    combo_multiplier: u32,
}

/// Pure objective transition. The boolean is true only on the completion edge.
fn apply_progress(
    current: ActiveObjective,
    input: ObjectiveProgressInput,
) -> (ActiveObjective, bool) {
    if current.completed {
        return (current, false);
    }

    let mut next = current;
    let target = next.kind.target();
    next.progress = match next.kind {
        ObjectiveKind::HitChickens { .. } => {
            next.progress.saturating_add(input.chicken_hits).min(target)
        }
        ObjectiveKind::CollectCoins { .. } => next
            .progress
            .saturating_add(input.coins_collected)
            .min(target),
        ObjectiveKind::ReachCombo { .. } => next.progress.max(input.combo_multiplier).min(target),
    };
    next.completed = next.progress >= target;
    (next, next.completed && !current.completed)
}

/// Pure one-time reward transition. A matching completion message is required
/// in addition to completed state, preventing unrelated/stale messages from
/// awarding the current objective.
fn apply_reward(
    current: ActiveObjective,
    chicken_score: u32,
    completed_kind: Option<ObjectiveKind>,
) -> (ActiveObjective, u32, bool) {
    if current.completed && !current.reward_awarded && completed_kind == Some(current.kind) {
        let mut next = current;
        next.reward_awarded = true;
        (
            next,
            roady_score_rules::award_objective(chicken_score),
            true,
        )
    } else {
        (current, chicken_score, false)
    }
}

pub struct ObjectivesPlugin;

impl Plugin for ObjectivesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveObjective>()
            .init_resource::<ObjectiveRoundIndex>()
            .add_message::<ObjectiveCompleted>()
            // RoundActive remains false throughout SpawnSet on a fresh entry.
            .add_systems(
                OnEnter(GameState::Playing),
                select_fresh_objective
                    .in_set(SpawnSet)
                    .in_set(ObjectiveSelectionSet),
            )
            // Recreate presentation after selection. On pause resume this uses
            // the preserved objective rather than resetting it.
            .add_systems(
                OnEnter(GameState::Playing),
                spawn_objective_hud.after(SpawnSet),
            )
            .add_systems(
                OnExit(GameState::Playing),
                despawn_marker::<ObjectiveHudRoot>,
            )
            .add_systems(
                PostUpdate,
                // Update owns the chicken/coin producers and may also request
                // GameOver. Consume those messages and award the completion
                // bonus before the state transition and its OnEnter snapshot.
                (tick_objective, award_objective_bonus)
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            )
            .add_systems(
                Update,
                update_objective_hud
                    .run_if(in_state(GameState::Playing))
                    .run_if(resource_changed::<ActiveObjective>),
            )
            .add_systems(
                Update,
                update_objective_layout
                    .after(TouchStateSet)
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

fn select_fresh_objective(
    round_active: Res<RoundActive>,
    modifier: Res<ActiveModifier>,
    mut objective: ResMut<ActiveObjective>,
    mut index: ResMut<ObjectiveRoundIndex>,
) {
    let Some((selected, next_index)) =
        fresh_objective_selection(round_active.0, index.0, modifier.0)
    else {
        return;
    };
    *objective = selected;
    index.0 = next_index;
}

fn tick_objective(
    mut chicken_hits: MessageReader<ChickenHit>,
    mut coin_hits: MessageReader<CoinCollected>,
    combo: Res<Combo>,
    mut objective: ResMut<ActiveObjective>,
    mut completions: MessageWriter<ObjectiveCompleted>,
) {
    // These are independent readers: consuming messages here does not affect
    // audio, combo scoring, or any other MessageReader.
    let input = ObjectiveProgressInput {
        chicken_hits: u32::try_from(chicken_hits.read().count()).unwrap_or(u32::MAX),
        coins_collected: u32::try_from(coin_hits.read().count()).unwrap_or(u32::MAX),
        combo_multiplier: combo.multiplier,
    };
    let (next, newly_completed) = apply_progress(*objective, input);
    if next != *objective {
        *objective = next;
    }
    if newly_completed {
        completions.write(ObjectiveCompleted { kind: next.kind });
    }
}

fn award_objective_bonus(
    mut completions: MessageReader<ObjectiveCompleted>,
    mut objective: ResMut<ActiveObjective>,
    mut score: ResMut<Score>,
) {
    // There is one active objective. If duplicate matching messages ever
    // arrive, applying the pure transition repeatedly still awards only once.
    for completion in completions.read() {
        let (next, chicken_score, awarded) =
            apply_reward(*objective, score.chickens, Some(completion.kind));
        if awarded {
            *objective = next;
            score.chickens = chicken_score;
        }
    }
}

fn objective_hud_copy(objective: &ActiveObjective, portrait: bool) -> String {
    if portrait && objective.completed {
        // `summary` already contains `COMPLETE +10`; repeating `BONUS +10`
        // overflows the narrow portrait panel without adding information.
        format!("MISSION | {}", objective.summary())
    } else {
        format!(
            "MISSION | {} | BONUS +{OBJECTIVE_BONUS}",
            objective.summary()
        )
    }
}

fn spawn_objective_hud(
    mut commands: Commands,
    objective: Res<ActiveObjective>,
    touch: Res<TouchControlsActive>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let viewport = windows
        .single()
        .ok()
        .map(|window| Vec2::new(window.width(), window.height()));
    let portrait = touch.0 && viewport.is_some_and(is_touch_portrait);
    commands
        .spawn((objective_root_node(touch.0, viewport), ObjectiveHudRoot))
        .with_child((
            Text::new(objective_hud_copy(&objective, portrait)),
            objective_font(touch.0, viewport),
            TextColor(Color::srgb(1.0, 0.86, 0.22)),
            objective_panel_node(touch.0, viewport),
            BackgroundColor(Color::srgba(0.015, 0.02, 0.035, 0.72)),
            ObjectiveHudText,
        ));
}

fn objective_root_node(touch_active: bool, viewport: Option<Vec2>) -> Node {
    let portrait = touch_active
        .then_some(viewport)
        .flatten()
        .filter(|viewport| is_touch_portrait(*viewport));
    Node {
        position_type: PositionType::Absolute,
        top: px(if let Some(viewport) = portrait {
            touch_objective_bounds(viewport).top
        } else if touch_active {
            TOUCH_OBJECTIVE_TOP
        } else {
            54.0
        }),
        left: px(0.0),
        width: Val::Percent(100.0),
        justify_content: JustifyContent::Center,
        ..default()
    }
}

fn objective_panel_node(touch_active: bool, viewport: Option<Vec2>) -> Node {
    if touch_active {
        let bounds = viewport
            .filter(|viewport| is_touch_portrait(*viewport))
            .map(touch_objective_bounds);
        Node {
            width: px(bounds.map_or(TOUCH_OBJECTIVE_WIDTH, |bounds| bounds.width())),
            height: px(bounds.map_or(TOUCH_OBJECTIVE_HEIGHT, |bounds| bounds.height())),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: UiRect::axes(px(6.0), px(3.0)),
            ..default()
        }
    } else {
        Node {
            padding: UiRect::axes(px(9.0), px(4.0)),
            ..default()
        }
    }
}

fn objective_font(touch_active: bool, viewport: Option<Vec2>) -> TextFont {
    let portrait = touch_active && viewport.is_some_and(is_touch_portrait);
    TextFont {
        // The completed mission is the longest form. Ten pixels keeps it on
        // one line inside the 304px narrow-portrait panel; landscape touch
        // retains its established 12px typography.
        font_size: FontSize::Px(if portrait {
            10.0
        } else if touch_active {
            12.0
        } else {
            17.0
        }),
        ..default()
    }
}

fn update_objective_layout(
    touch: Res<TouchControlsActive>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut roots: Query<&mut Node, (With<ObjectiveHudRoot>, Without<ObjectiveHudText>)>,
    mut labels: Query<(&mut Node, &mut TextFont), With<ObjectiveHudText>>,
) {
    if !touch.0 {
        return;
    }
    let viewport = windows
        .single()
        .ok()
        .map(|window| Vec2::new(window.width(), window.height()));
    for mut node in &mut roots {
        *node = objective_root_node(true, viewport);
    }
    for (mut node, mut font) in &mut labels {
        *node = objective_panel_node(true, viewport);
        *font = objective_font(true, viewport);
    }
}

/// Change-filtered HUD refresh; no UI entity is rebuilt as progress advances.
fn update_objective_hud(
    objective: Res<ActiveObjective>,
    touch: Res<TouchControlsActive>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut labels: Query<&mut Text, With<ObjectiveHudText>>,
) {
    let portrait = touch.0
        && windows
            .single()
            .ok()
            .is_some_and(|window| is_touch_portrait(Vec2::new(window.width(), window.height())));
    let summary = objective_hud_copy(&objective, portrait);
    for mut label in &mut labels {
        **label = summary.clone();
    }
}

fn despawn_marker<M: Component>(mut commands: Commands, query: Query<Entity, With<M>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_MODIFIERS: [ModifierKind; 5] = [
        ModifierKind::Standard,
        ModifierKind::RushHour,
        ModifierKind::ChickenFrenzy,
        ModifierKind::Stampede,
        ModifierKind::GlassCannon,
    ];

    fn input(chickens: u32, coins: u32, combo: u32) -> ObjectiveProgressInput {
        ObjectiveProgressInput {
            chicken_hits: chickens,
            coins_collected: coins,
            combo_multiplier: combo,
        }
    }

    #[derive(Resource, Debug, Default, PartialEq, Eq)]
    struct TerminalSnapshot {
        captures: u32,
        objective: Option<ActiveObjective>,
        chicken_score: u32,
    }

    fn produce_timeout_chicken_hit(
        mut produced: Local<bool>,
        mut score: ResMut<Score>,
        mut hits: MessageWriter<ChickenHit>,
        mut reason: ResMut<crate::game::resources::GameOverReason>,
        mut next: ResMut<NextState<GameState>>,
    ) {
        if *produced {
            return;
        }
        *produced = true;
        score.chickens = score.chickens.saturating_add(1);
        hits.write(ChickenHit);
        *reason = crate::game::resources::GameOverReason::TimeUp;
        next.set(GameState::GameOver);
    }

    fn capture_terminal_snapshot(
        objective: Res<ActiveObjective>,
        score: Res<Score>,
        mut snapshot: ResMut<TerminalSnapshot>,
    ) {
        snapshot.captures += 1;
        snapshot.objective = Some(*objective);
        snapshot.chicken_score = score.chickens;
    }

    #[test]
    fn terminal_update_completes_and_rewards_before_gameover_snapshot() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .init_state::<GameState>()
            .init_resource::<Score>()
            .init_resource::<Combo>()
            .init_resource::<RoundActive>()
            .init_resource::<crate::game::resources::GameOverReason>()
            .init_resource::<ActiveModifier>()
            .init_resource::<TerminalSnapshot>()
            .init_resource::<TouchControlsActive>()
            .add_message::<ChickenHit>()
            .add_message::<CoinCollected>()
            .add_plugins(ObjectivesPlugin)
            .add_systems(
                Update,
                produce_timeout_chicken_hit.run_if(in_state(GameState::Playing)),
            )
            .add_systems(OnEnter(GameState::GameOver), capture_terminal_snapshot);

        app.world_mut()
            .insert_resource(ActiveObjective::new(ObjectiveKind::HitChickens {
                target: 1,
            }));
        app.world_mut().resource_mut::<RoundActive>().0 = true;
        app.world_mut()
            .resource_mut::<NextState<GameState>>()
            .set(GameState::Playing);
        app.update();
        app.update();

        let snapshot = app.world().resource::<TerminalSnapshot>();
        let objective = snapshot
            .objective
            .expect("GameOver entry must capture the terminal objective");
        assert_eq!(snapshot.captures, 1);
        assert_eq!(objective.progress, 1);
        assert!(objective.completed);
        assert!(objective.reward_awarded);
        assert_eq!(snapshot.chicken_score, 1 + OBJECTIVE_BONUS);
        assert_eq!(
            app.world().resource::<Score>().chickens,
            1 + OBJECTIVE_BONUS
        );

        app.update();
        let snapshot = app.world().resource::<TerminalSnapshot>();
        assert_eq!(snapshot.captures, 1);
        assert_eq!(snapshot.chicken_score, 1 + OBJECTIVE_BONUS);
        assert_eq!(
            app.world().resource::<Score>().chickens,
            1 + OBJECTIVE_BONUS
        );
    }

    #[test]
    fn first_round_is_hit_ten_and_selection_is_deterministic() {
        let expected = ObjectiveKind::HitChickens { target: 10 };
        assert_eq!(
            objective_kind_for_round(0, ModifierKind::Standard),
            expected
        );
        assert_eq!(
            fresh_objective_selection(false, 0, ModifierKind::Standard),
            Some((ActiveObjective::new(expected), 1))
        );
        for index in 0..100 {
            for modifier in ALL_MODIFIERS {
                assert_eq!(
                    objective_kind_for_round(index, modifier),
                    objective_kind_for_round(index, modifier)
                );
            }
        }
    }

    #[test]
    fn deterministic_cycle_reaches_every_kind() {
        let kinds: Vec<_> = (0..6)
            .map(|index| objective_kind_for_round(index, ModifierKind::Standard))
            .collect();
        assert!(matches!(kinds[0], ObjectiveKind::HitChickens { .. }));
        assert!(matches!(kinds[1], ObjectiveKind::CollectCoins { .. }));
        assert!(matches!(kinds[2], ObjectiveKind::ReachCombo { .. }));
        assert_eq!(kinds[0], kinds[3]);
        assert_eq!(kinds[1], kinds[4]);
        assert_eq!(kinds[2], kinds[5]);
    }

    #[test]
    fn pause_resume_neither_resets_nor_increments_index() {
        for index in [0, 1, 19, u64::MAX] {
            assert_eq!(
                fresh_objective_selection(true, index, ModifierKind::ChickenFrenzy),
                None
            );
        }
    }

    #[test]
    fn fresh_selection_resets_all_live_objective_state() {
        let mut old = ActiveObjective::new(ObjectiveKind::CollectCoins { target: 6 });
        old.progress = 6;
        old.completed = true;
        old.reward_awarded = true;

        let (fresh, next_index) =
            fresh_objective_selection(false, 3, ModifierKind::Standard).unwrap();
        assert_ne!(fresh, old);
        assert_eq!(fresh.kind, ObjectiveKind::HitChickens { target: 10 });
        assert_eq!(fresh.progress, 0);
        assert!(!fresh.completed);
        assert!(!fresh.reward_awarded);
        assert_eq!(next_index, 4);
    }

    #[test]
    fn modifier_targets_are_adjusted_only_for_matching_flavors() {
        assert_eq!(
            objective_kind_for_round(0, ModifierKind::ChickenFrenzy),
            ObjectiveKind::HitChickens { target: 20 }
        );
        assert_eq!(
            objective_kind_for_round(1, ModifierKind::RushHour),
            ObjectiveKind::CollectCoins { target: 8 }
        );
        assert_eq!(
            objective_kind_for_round(2, ModifierKind::GlassCannon),
            ObjectiveKind::ReachCombo { target: 4 }
        );
        assert_eq!(
            objective_kind_for_round(0, ModifierKind::RushHour),
            ObjectiveKind::HitChickens { target: 10 }
        );
        assert_eq!(
            objective_kind_for_round(1, ModifierKind::ChickenFrenzy),
            ObjectiveKind::CollectCoins { target: 6 }
        );
        assert_eq!(
            objective_kind_for_round(2, ModifierKind::Standard),
            ObjectiveKind::ReachCombo { target: 3 }
        );
    }

    #[test]
    fn chicken_progress_counts_only_chickens_and_clamps_at_boundary() {
        let start = ActiveObjective::new(ObjectiveKind::HitChickens { target: 10 });
        let (at_nine, completed) = apply_progress(start, input(9, 99, 5));
        assert_eq!(at_nine.progress, 9);
        assert!(!completed);
        assert!(!at_nine.completed);

        let (at_ten, completed) = apply_progress(at_nine, input(1, 0, 1));
        assert_eq!(at_ten.progress, 10);
        assert!(completed);
        assert!(at_ten.completed);

        let (still_ten, completed_again) = apply_progress(at_ten, input(u32::MAX, 0, 1));
        assert_eq!(still_ten.progress, 10);
        assert!(!completed_again);
    }

    #[test]
    fn coin_progress_counts_only_coin_messages_and_saturates_safely() {
        let mut start = ActiveObjective::new(ObjectiveKind::CollectCoins { target: 6 });
        start.progress = 5;
        let (unchanged, completed) = apply_progress(start, input(100, 0, 5));
        assert_eq!(unchanged.progress, 5);
        assert!(!completed);

        let (done, completed) = apply_progress(unchanged, input(0, u32::MAX, 1));
        assert_eq!(done.progress, 6);
        assert!(completed);
    }

    #[test]
    fn combo_uses_highest_current_multiplier_and_completes_at_target() {
        let start = ActiveObjective::new(ObjectiveKind::ReachCombo { target: 4 });
        let (at_three, completed) = apply_progress(start, input(50, 50, 3));
        assert_eq!(at_three.progress, 3);
        assert!(!completed);

        // A later lower current multiplier cannot erase achieved progress.
        let (still_three, completed) = apply_progress(at_three, input(0, 0, 1));
        assert_eq!(still_three.progress, 3);
        assert!(!completed);

        let (done, completed) = apply_progress(still_three, input(0, 0, 4));
        assert_eq!(done.progress, 4);
        assert!(completed);
    }

    #[test]
    fn completion_and_reward_cannot_fire_twice() {
        let start = ActiveObjective::new(ObjectiveKind::HitChickens { target: 1 });
        let (completed, edge) = apply_progress(start, input(1, 0, 1));
        assert!(edge);
        let (same, second_edge) = apply_progress(completed, input(1, 0, 5));
        assert_eq!(same, completed);
        assert!(!second_edge);

        let (awarded, score, did_award) = apply_reward(completed, 7, Some(completed.kind));
        assert!(did_award);
        assert_eq!(score, 17);
        assert!(awarded.reward_awarded);

        let (unchanged, score, did_award) = apply_reward(awarded, score, Some(awarded.kind));
        assert_eq!(unchanged, awarded);
        assert_eq!(score, 17);
        assert!(!did_award);
    }

    #[test]
    fn reward_requires_matching_completion_and_score_addition_saturates() {
        let mut completed = ActiveObjective::new(ObjectiveKind::CollectCoins { target: 6 });
        completed.progress = 6;
        completed.completed = true;

        let (unchanged, score, awarded) = apply_reward(
            completed,
            20,
            Some(ObjectiveKind::HitChickens { target: 10 }),
        );
        assert_eq!(unchanged, completed);
        assert_eq!(score, 20);
        assert!(!awarded);

        let (rewarded, score, awarded) =
            apply_reward(completed, u32::MAX - 2, Some(completed.kind));
        assert!(awarded);
        assert!(rewarded.reward_awarded);
        assert_eq!(score, u32::MAX);
    }

    #[test]
    fn display_and_summary_accessors_are_stable() {
        let mut objective = ActiveObjective::new(ObjectiveKind::ReachCombo { target: 3 });
        objective.progress = 2;
        assert_eq!(objective.kind.display_name(), "Reach combo");
        assert_eq!(objective.kind.target(), 3);
        assert_eq!(objective.summary(), "Reach combo 2/3");
        objective.progress = 3;
        objective.completed = true;
        assert_eq!(objective.summary(), "Reach combo 3/3 | COMPLETE +10");
        assert!(objective.summary().is_ascii());
        assert!(format!("OBJECTIVE | {}", objective.summary()).is_ascii());
    }
}
