//! Competition/conduct ownership for additive rules v3.
//!
//! Classic gameplay remains the default.  Ranked state can only be armed by an
//! explicitly injected, validated Worker receipt; this chunk deliberately has
//! no capability fetch, session request, or score submission code.

use bevy::prelude::*;
use roady_score_rules::v3;

use crate::car::InputFrozen;
use crate::game::resources::{Drowning, RoundActive};
use crate::game::state::GameState;
use crate::game::{RoundClockSet, SpawnSet};
use crate::modifiers::{ModifierKind, SelectedModifier};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Competition {
    Ranked,
    #[default]
    Casual,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Conduct {
    #[default]
    CluckHunt,
    RightOfWay,
}

impl Conduct {
    pub const fn rules(self) -> v3::Conduct {
        match self {
            Self::CluckHunt => v3::Conduct::CluckHunt,
            Self::RightOfWay => v3::Conduct::RightOfWay,
        }
    }

    pub const fn category(self) -> &'static str {
        match self {
            Self::CluckHunt => v3::CLUCK_HUNT_CATEGORY,
            Self::RightOfWay => v3::RIGHT_OF_WAY_CATEGORY,
        }
    }
}

/// Frozen classic condition IDs. There is intentionally no ID 5.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ManualCondition {
    #[default]
    Standard = 0,
    RushHour = 1,
    ChickenFrenzy = 2,
    Stampede = 3,
    GlassCannon = 4,
}

impl TryFrom<u8> for ManualCondition {
    type Error = ();
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Standard),
            1 => Ok(Self::RushHour),
            2 => Ok(Self::ChickenFrenzy),
            3 => Ok(Self::Stampede),
            4 => Ok(Self::GlassCannon),
            _ => Err(()),
        }
    }
}

impl From<ModifierKind> for ManualCondition {
    fn from(value: ModifierKind) -> Self {
        match value {
            ModifierKind::Standard => Self::Standard,
            ModifierKind::RushHour => Self::RushHour,
            ModifierKind::ChickenFrenzy => Self::ChickenFrenzy,
            ModifierKind::Stampede => Self::Stampede,
            ModifierKind::GlassCannon => Self::GlassCannon,
        }
    }
}

impl From<ManualCondition> for ModifierKind {
    fn from(value: ManualCondition) -> Self {
        match value {
            ManualCondition::Standard => Self::Standard,
            ManualCondition::RushHour => Self::RushHour,
            ManualCondition::ChickenFrenzy => Self::ChickenFrenzy,
            ManualCondition::Stampede => Self::Stampede,
            ManualCondition::GlassCannon => Self::GlassCannon,
        }
    }
}

#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectedGameMode {
    pub competition: Competition,
    pub conduct: Conduct,
    pub manual_condition: ManualCondition,
}

impl Default for SelectedGameMode {
    fn default() -> Self {
        Self {
            competition: Competition::Casual,
            conduct: Conduct::CluckHunt,
            manual_condition: ManualCondition::Standard,
        }
    }
}

/// Validated material returned by a Worker start response. The receipt keeps
/// the exact started header used to derive h0; game code never fabricates it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerRankedReceipt {
    pub session_id: String,
    pub challenge: String,
    pub seed: [u8; 32],
    pub category: String,
    pub started_header: Vec<u8>,
    pub schedule_hash: [u8; 32],
    pub seed_commitment: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiptError {
    WrongCategory,
    SeedCommitment,
    ScheduleCommitment,
    EmptyStartedHeader,
}

impl WorkerRankedReceipt {
    /// Validate injected Worker material before it can become run state.
    pub fn validate(self, conduct: Conduct) -> Result<Self, ReceiptError> {
        if self.category != conduct.category() {
            return Err(ReceiptError::WrongCategory);
        }
        if self.started_header.is_empty() {
            return Err(ReceiptError::EmptyStartedHeader);
        }
        if v3::seed_commitment(&self.seed) != self.seed_commitment {
            return Err(ReceiptError::SeedCommitment);
        }
        if v3::schedule_commitment(&self.seed, &self.category) != self.schedule_hash {
            return Err(ReceiptError::ScheduleCommitment);
        }
        Ok(self)
    }
}

/// One-shot injection point. No local fallback seed exists for Ranked.
#[derive(Resource, Default, Debug)]
pub struct InjectedRankedSession(pub Option<WorkerRankedReceipt>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunCondition {
    Casual(ManualCondition),
    Ranked(WorkerRankedReceipt),
}

#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct ActiveRunRules {
    pub competition: Competition,
    pub conduct: Conduct,
    pub condition: RunCondition,
}

impl Default for ActiveRunRules {
    fn default() -> Self {
        Self {
            competition: Competition::Casual,
            conduct: Conduct::CluckHunt,
            condition: RunCondition::Casual(ManualCondition::Standard),
        }
    }
}

impl ActiveRunRules {
    pub fn ranked_receipt(&self) -> Option<&WorkerRankedReceipt> {
        match &self.condition {
            RunCondition::Ranked(receipt) => Some(receipt),
            RunCondition::Casual(_) => None,
        }
    }

    pub const fn is_ranked(&self) -> bool {
        matches!(self.condition, RunCondition::Ranked(_))
    }
}

/// Integer active-play time. The private fractional accumulator prevents frame
/// deltas from being rounded independently. Pause/countdown/drowning do not
/// mutate either field.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct ActivePlayClock {
    milliseconds: u64,
    fractional_ms: f64,
}

impl ActivePlayClock {
    pub const fn milliseconds(&self) -> u64 {
        self.milliseconds
    }

    pub fn advance_seconds(&mut self, seconds: f64) {
        if !seconds.is_finite() || seconds <= 0.0 {
            return;
        }
        self.fractional_ms += seconds * 1_000.0;
        // Tiny tolerance removes binary summation noise at exact integer-ms
        // boundaries without changing any representable sub-ms boundary.
        let whole = (self.fractional_ms + 1.0e-7).floor();
        let add = whole.min(u64::MAX as f64) as u64;
        self.milliseconds = self.milliseconds.saturating_add(add);
        self.fractional_ms -= add as f64;
        if self.fractional_ms < 0.0 {
            self.fractional_ms = 0.0;
        }
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct RunAdmissionError(pub Option<ReceiptError>);

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct GameModeSetupSet;

pub struct GameModesPlugin;

impl Plugin for GameModesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SelectedGameMode>()
            .init_resource::<InjectedRankedSession>()
            .init_resource::<ActiveRunRules>()
            .init_resource::<ActivePlayClock>()
            .init_resource::<RunAdmissionError>()
            .add_systems(
                OnEnter(GameState::Playing),
                obtain_fresh_run_rules
                    .in_set(GameModeSetupSet)
                    .before(SpawnSet),
            )
            .add_systems(
                Update,
                tick_active_play_clock
                    .in_set(RoundClockSet)
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

fn obtain_fresh_run_rules(
    round_active: Res<RoundActive>,
    selected: Res<SelectedGameMode>,
    selected_modifier: Res<SelectedModifier>,
    mut injected: ResMut<InjectedRankedSession>,
    mut active: ResMut<ActiveRunRules>,
    mut clock: ResMut<ActivePlayClock>,
    mut error: ResMut<RunAdmissionError>,
) {
    if round_active.0 {
        return;
    }
    clock.reset();
    error.0 = None;
    match selected.competition {
        Competition::Casual => {
            *active = ActiveRunRules {
                competition: Competition::Casual,
                conduct: selected.conduct,
                condition: RunCondition::Casual(selected_modifier.0.into()),
            };
            // A Casual run cannot retain a proof/capability/receipt.
            injected.0 = None;
        }
        Competition::Ranked => {
            let Some(receipt) = injected.0.take() else {
                error.0 = Some(ReceiptError::EmptyStartedHeader);
                return;
            };
            match receipt.validate(selected.conduct) {
                Ok(receipt) => {
                    *active = ActiveRunRules {
                        competition: Competition::Ranked,
                        conduct: selected.conduct,
                        condition: RunCondition::Ranked(receipt),
                    };
                }
                Err(receipt_error) => error.0 = Some(receipt_error),
            }
        }
    }
}

fn tick_active_play_clock(
    time: Res<Time>,
    frozen: Res<InputFrozen>,
    drowning: Res<Drowning>,
    mut clock: ResMut<ActivePlayClock>,
) {
    if !frozen.0 && !drowning.active {
        clock.advance_seconds(time.delta_secs_f64());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_at_fps(fps: u32, seconds: u32) -> u64 {
        let mut clock = ActivePlayClock::default();
        for _ in 0..fps * seconds {
            clock.advance_seconds(1.0 / f64::from(fps));
        }
        clock.milliseconds()
    }

    #[test]
    fn classic_ids_are_exact_and_no_sixth_id_exists() {
        for id in 0..=4 {
            assert_eq!(ManualCondition::try_from(id).unwrap() as u8, id);
        }
        assert!(ManualCondition::try_from(5).is_err());
        assert_eq!(SelectedGameMode::default().competition, Competition::Casual);
    }

    #[test]
    fn integer_clock_is_equivalent_at_30_60_and_120_fps() {
        for seconds in [1, 8, 11, 29, 60, 120] {
            let values = [30, 60, 120].map(|fps| run_at_fps(fps, seconds));
            assert_eq!(values, [u64::from(seconds) * 1_000; 3]);
        }
    }

    #[test]
    fn conduct_ids_and_categories_are_stable() {
        assert_eq!(Conduct::CluckHunt.rules(), v3::Conduct::CluckHunt);
        assert_eq!(Conduct::RightOfWay.rules(), v3::Conduct::RightOfWay);
        assert_eq!(Conduct::RightOfWay.category(), v3::RIGHT_OF_WAY_CATEGORY);
        assert_eq!(Competition::Ranked, Competition::Ranked);
    }

    #[test]
    fn invalid_receipt_cannot_arm_ranked() {
        let receipt = WorkerRankedReceipt {
            session_id: "S".into(),
            challenge: "C".into(),
            seed: [1; 32],
            category: v3::CLUCK_HUNT_CATEGORY.into(),
            started_header: vec![1],
            schedule_hash: [0; 32],
            seed_commitment: [0; 32],
        };
        assert_eq!(
            receipt.validate(Conduct::CluckHunt),
            Err(ReceiptError::SeedCommitment)
        );
    }
}
