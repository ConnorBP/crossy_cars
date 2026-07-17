//! Deterministic per-round gameplay modifiers.
//!
//! This module only owns selection and tuning data. Gameplay systems consume
//! [`ActiveModifier`] through the pure accessors below; persistence is
//! intentionally not involved because modifiers belong to one fresh round.

use bevy::prelude::*;

use crate::game::SpawnSet;
use crate::game::resources::RoundActive;
use crate::game::state::GameState;
use crate::game_modes::{ActiveRunRules, GameModeSetupSet, RunCondition};
use crate::rotation::RotationState;

/// The rule set applied to a round.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ModifierKind {
    /// Baseline rules, always used for the first round of a process.
    #[default]
    Standard,
    /// More, faster oncoming traffic.
    RushHour,
    /// A much larger flock with an extra point for each chicken.
    ChickenFrenzy,
    /// More penalty critters on the road.
    Stampede,
    /// Larger incoming damage, balanced by larger combo bonuses.
    GlassCannon,
}

impl ModifierKind {
    /// Stable storage index. Do not reorder these values: persisted condition
    /// bests use this exact Standard-through-Glass-Cannon layout.
    pub const fn index(self) -> usize {
        self.rules_id().storage_index()
    }

    /// Engine-independent stable ID from the shared scoring-rules crate.
    pub(crate) const fn rules_id(self) -> roady_score_rules::ConditionId {
        match self {
            Self::Standard => roady_score_rules::ConditionId::Standard,
            Self::RushHour => roady_score_rules::ConditionId::RushHour,
            Self::ChickenFrenzy => roady_score_rules::ConditionId::ChickenFrenzy,
            Self::Stampede => roady_score_rules::ConditionId::Stampede,
            Self::GlassCannon => roady_score_rules::ConditionId::GlassCannon,
        }
    }

    /// Stable player-facing label for HUDs and round-intro screens.
    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::RushHour => "Rush Hour",
            Self::ChickenFrenzy => "Chicken Frenzy",
            Self::Stampede => "Stampede",
            Self::GlassCannon => "Glass Cannon",
        }
    }

    /// Stable accent color for presentation. This has no gameplay effect.
    pub(crate) const fn color(self) -> Color {
        match self {
            Self::Standard => Color::srgb(0.85, 0.88, 0.92),
            Self::RushHour => Color::srgb(1.0, 0.25, 0.10),
            Self::ChickenFrenzy => Color::srgb(1.0, 0.80, 0.05),
            Self::Stampede => Color::srgb(0.72, 0.42, 0.18),
            Self::GlassCannon => Color::srgb(0.35, 0.90, 1.0),
        }
    }

    /// Multiplier for the target oncoming-traffic population.
    pub(crate) const fn traffic_count_multiplier(self) -> usize {
        match self {
            Self::RushHour => 2,
            _ => 1,
        }
    }

    /// Multiplier for each oncoming vehicle's speed.
    pub(crate) const fn traffic_speed_multiplier(self) -> f32 {
        match self {
            Self::RushHour => 1.35,
            _ => 1.0,
        }
    }

    /// Target chicken population for a supplied baseline.
    ///
    /// Frenzy uses integer arithmetic and rounds half-chickens upward, making
    /// the result approximately 2.5x without introducing float conversions.
    pub(crate) const fn chicken_target(self, base: usize) -> usize {
        match self {
            Self::ChickenFrenzy => base.saturating_mul(2).saturating_add(base.div_ceil(2)),
            _ => base,
        }
    }

    /// Regular roadside coin population for a supplied baseline.
    /// Chicken Frenzy gives up one coin per road block to offset its denser,
    /// higher-value flock without changing coin scoring or time rules.
    pub(crate) const fn coin_target(self, base: usize) -> usize {
        match self {
            Self::ChickenFrenzy => base.saturating_sub(1),
            _ => base,
        }
    }

    /// Multiplier for the target penalty-critter population.
    pub(crate) const fn critter_count_multiplier(self) -> usize {
        match self {
            Self::Stampede => 2,
            _ => 1,
        }
    }

    /// Multiplier applied to damage received by the player.
    pub(crate) const fn damage_multiplier(self) -> f32 {
        match self {
            Self::GlassCannon => 2.0,
            _ => 1.0,
        }
    }

    /// Multiplier applied only to score awarded above a combo's base point.
    #[cfg(test)]
    pub(crate) const fn combo_bonus_multiplier(self) -> u32 {
        self.rules_id().combo_bonus_multiplier()
    }

    /// Extra chicken-score points awarded in addition to normal scoring.
    #[cfg(test)]
    pub(crate) const fn chicken_score_bonus(self) -> u32 {
        self.rules_id().chicken_score_bonus()
    }
}

/// Modifier selected for the current (or next) round.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActiveModifier(pub ModifierKind);

/// Player-selected condition for the next fresh round. Pause/resume never
/// consumes or overwrites this choice.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectedModifier(pub ModifierKind);

// Delegate the tuning API from the resource to its value. Systems can read a
// `Res<ActiveModifier>` directly without coupling themselves to tuple layout;
// the same pure API remains available on `ModifierKind` for value-level code.
impl ActiveModifier {
    pub(crate) const fn display_name(&self) -> &'static str {
        self.0.display_name()
    }

    pub(crate) const fn color(&self) -> Color {
        self.0.color()
    }

    pub(crate) const fn traffic_count_multiplier(&self) -> usize {
        self.0.traffic_count_multiplier()
    }

    pub(crate) const fn traffic_speed_multiplier(&self) -> f32 {
        self.0.traffic_speed_multiplier()
    }

    pub(crate) const fn chicken_target(&self, base: usize) -> usize {
        self.0.chicken_target(base)
    }

    pub(crate) const fn critter_count_multiplier(&self) -> usize {
        self.0.critter_count_multiplier()
    }

    pub(crate) const fn damage_multiplier(&self) -> f32 {
        self.0.damage_multiplier()
    }

    #[cfg(test)]
    pub(crate) const fn combo_bonus_multiplier(&self) -> u32 {
        self.0.combo_bonus_multiplier()
    }

    #[cfg(test)]
    pub(crate) const fn chicken_score_bonus(&self) -> u32 {
        self.0.chicken_score_bonus()
    }
}

/// Pure transition: a fresh round consumes the explicit Menu selection,
/// while a pause resume leaves the current active condition untouched.
fn fresh_round_selection(round_active: bool, selected: ModifierKind) -> Option<ModifierKind> {
    (!round_active).then_some(selected)
}

pub struct ModifiersPlugin;

impl Plugin for ModifiersPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveModifier>()
            .init_resource::<SelectedModifier>()
            // Modifier consumers in SpawnSet must observe the new selection.
            // RoundActive is still false here; reset_run flips it after that
            // set has completed.
            .add_systems(
                OnEnter(GameState::Playing),
                select_modifier.after(GameModeSetupSet).before(SpawnSet),
            );
    }
}

/// Select and count exactly one modifier for a fresh round. Entering Playing
/// to resume from Paused leaves both resources untouched.
fn select_modifier(
    round_active: Res<RoundActive>,
    selected: Res<SelectedModifier>,
    rules: Option<Res<ActiveRunRules>>,
    rotation: Option<Res<RotationState>>,
    mut active: ResMut<ActiveModifier>,
) {
    if round_active.0 {
        return;
    }
    if let Some(rules) = rules {
        active.0 = match &rules.condition {
            RunCondition::Casual(condition) => (*condition).into(),
            RunCondition::Ranked(_) => rotation
                .as_ref()
                .and_then(|rotation| rotation.effect_at(0))
                .map(effect_modifier)
                .unwrap_or(ModifierKind::Standard),
        };
    } else if let Some(kind) = fresh_round_selection(false, selected.0) {
        active.0 = kind;
    }
}

pub(crate) const fn effect_modifier(effect: roady_score_rules::v3::Effect) -> ModifierKind {
    match effect {
        roady_score_rules::v3::Effect::Standard => ModifierKind::Standard,
        roady_score_rules::v3::Effect::RushHour => ModifierKind::RushHour,
        roady_score_rules::v3::Effect::ChickenFrenzy => ModifierKind::ChickenFrenzy,
        roady_score_rules::v3::Effect::Stampede => ModifierKind::Stampede,
        roady_score_rules::v3::Effect::GlassCannon => ModifierKind::GlassCannon,
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
    fn chicken_frenzy_reduces_only_regular_coin_population_by_one() {
        assert_eq!(ALL.map(|kind| kind.coin_target(4)), [4, 4, 3, 4, 4]);
        assert_eq!(ModifierKind::ChickenFrenzy.coin_target(0), 0);
        assert_eq!(ModifierKind::ChickenFrenzy.coin_target(1), 0);
        assert_eq!(
            ModifierKind::ChickenFrenzy.coin_target(usize::MAX),
            usize::MAX - 1
        );
    }

    #[test]
    fn modifier_storage_indices_are_stable() {
        for (expected, kind) in ALL.into_iter().enumerate() {
            assert_eq!(kind.index(), expected);
        }
    }

    #[test]
    fn resources_and_kind_have_safe_defaults() {
        assert_eq!(ModifierKind::default(), ModifierKind::Standard);
        let active = ActiveModifier::default();
        assert_eq!(active.0, ModifierKind::Standard);
        assert_eq!(active.display_name(), "Standard");
        assert_eq!(active.color(), ModifierKind::Standard.color());
        assert_eq!(active.traffic_count_multiplier(), 1);
        assert_eq!(active.traffic_speed_multiplier(), 1.0);
        assert_eq!(active.chicken_target(14), 14);
        assert_eq!(active.0.coin_target(4), 4);
        assert_eq!(active.critter_count_multiplier(), 1);
        assert_eq!(active.damage_multiplier(), 1.0);
        assert_eq!(active.combo_bonus_multiplier(), 1);
        assert_eq!(active.chicken_score_bonus(), 0);
        assert_eq!(SelectedModifier::default().0, ModifierKind::Standard);
    }

    #[test]
    fn fresh_round_uses_explicit_selection_and_resume_preserves_active() {
        for selected in ALL {
            assert_eq!(fresh_round_selection(false, selected), Some(selected));
            assert_eq!(fresh_round_selection(true, selected), None);
        }
    }

    #[test]
    fn labels_and_colors_are_distinct_and_stable() {
        let expected_names = [
            "Standard",
            "Rush Hour",
            "Chicken Frenzy",
            "Stampede",
            "Glass Cannon",
        ];
        for (kind, expected) in ALL.into_iter().zip(expected_names) {
            assert_eq!(kind.display_name(), expected);
        }

        for (i, left) in ALL.into_iter().enumerate() {
            for right in ALL.into_iter().skip(i + 1) {
                assert_ne!(left.color(), right.color());
            }
        }
    }

    #[test]
    fn neutral_modifiers_leave_unrelated_tuning_unchanged() {
        for kind in ALL {
            if kind != ModifierKind::RushHour {
                assert_eq!(kind.traffic_count_multiplier(), 1);
                assert_eq!(kind.traffic_speed_multiplier(), 1.0);
            }
            if kind != ModifierKind::ChickenFrenzy {
                assert_eq!(kind.chicken_target(14), 14);
                assert_eq!(kind.chicken_score_bonus(), 0);
            }
            if kind != ModifierKind::Stampede {
                assert_eq!(kind.critter_count_multiplier(), 1);
            }
            if kind != ModifierKind::GlassCannon {
                assert_eq!(kind.damage_multiplier(), 1.0);
                assert_eq!(kind.combo_bonus_multiplier(), 1);
            }
        }
    }

    #[test]
    fn rush_hour_has_exact_traffic_multipliers() {
        let rush = ModifierKind::RushHour;
        assert_eq!(rush.traffic_count_multiplier(), 2);
        assert_eq!(rush.traffic_speed_multiplier(), 1.35);
        assert!(rush.traffic_speed_multiplier().is_finite());
        assert!(rush.traffic_speed_multiplier() > 0.0);
    }

    #[test]
    fn chicken_frenzy_target_is_roughly_two_and_a_half_times_base() {
        let frenzy = ModifierKind::ChickenFrenzy;
        assert_eq!(frenzy.chicken_target(0), 0);
        assert_eq!(frenzy.chicken_target(1), 3);
        assert_eq!(frenzy.chicken_target(2), 5);
        assert_eq!(frenzy.chicken_target(3), 8);
        assert_eq!(frenzy.chicken_target(14), 35);
        assert_eq!(frenzy.chicken_score_bonus(), 1);
    }

    #[test]
    fn stampede_and_glass_cannon_have_exact_tradeoffs() {
        assert_eq!(ModifierKind::Stampede.critter_count_multiplier(), 2);
        assert_eq!(ModifierKind::GlassCannon.damage_multiplier(), 2.0);
        assert_eq!(ModifierKind::GlassCannon.combo_bonus_multiplier(), 2);
    }
}
