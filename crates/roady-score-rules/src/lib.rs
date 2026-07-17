#![recursion_limit = "512"]

//! Engine-independent, versioned scoring rules for Roady.
//!
//! This crate deliberately has no Bevy dependency. The typed API is the source
//! of truth; [`manifest_pretty_json`] and [`schema_pretty_json`] expose its
//! language-neutral contract.

use serde::{Deserialize, Serialize};

/// Deterministic rotation-v2 gameplay and commitment rules.
pub mod v2;
/// Additive deterministic rotation-v3 gameplay and canonical protocol rules.
pub mod v3;

/// Typed rules protocol version. This is not the game package version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RulesVersion(pub u32);

pub const RULES_VERSION: RulesVersion = RulesVersion(1);
pub const RULES_VERSION_ID: &str = "roady-rules.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionId {
    Standard,
    RushHour,
    ChickenFrenzy,
    Stampede,
    GlassCannon,
}

pub const CONDITIONS: [ConditionId; 5] = [
    ConditionId::Standard,
    ConditionId::RushHour,
    ConditionId::ChickenFrenzy,
    ConditionId::Stampede,
    ConditionId::GlassCannon,
];

impl ConditionId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::RushHour => "rush_hour",
            Self::ChickenFrenzy => "chicken_frenzy",
            Self::Stampede => "stampede",
            Self::GlassCannon => "glass_cannon",
        }
    }

    pub const fn storage_index(self) -> usize {
        match self {
            Self::Standard => 0,
            Self::RushHour => 1,
            Self::ChickenFrenzy => 2,
            Self::Stampede => 3,
            Self::GlassCannon => 4,
        }
    }

    pub const fn chicken_score_bonus(self) -> u32 {
        if matches!(self, Self::ChickenFrenzy) {
            1
        } else {
            0
        }
    }

    pub const fn combo_bonus_multiplier(self) -> u32 {
        if matches!(self, Self::GlassCannon) {
            2
        } else {
            1
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventId {
    TrafficSurge,
    ChickenBurst,
    ComboFrenzy,
    CritterBurst,
}

pub const EVENTS: [EventId; 4] = [
    EventId::TrafficSurge,
    EventId::ChickenBurst,
    EventId::ComboFrenzy,
    EventId::CritterBurst,
];

impl EventId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TrafficSurge => "traffic_surge",
            Self::ChickenBurst => "chicken_burst",
            Self::ComboFrenzy => "combo_frenzy",
            Self::CritterBurst => "critter_burst",
        }
    }

    pub const fn chicken_score_bonus(self) -> u32 {
        if matches!(self, Self::ChickenBurst) {
            1
        } else {
            0
        }
    }

    pub const fn combo_bonus_multiplier(self) -> u32 {
        if matches!(self, Self::ComboFrenzy) {
            2
        } else {
            1
        }
    }
}

/// Every event reachable during a round with this condition, in schedule order.
pub const fn reachable_events(condition: ConditionId) -> [EventId; 2] {
    match condition {
        ConditionId::Standard => [EventId::TrafficSurge, EventId::CritterBurst],
        ConditionId::RushHour => [EventId::ChickenBurst, EventId::ComboFrenzy],
        ConditionId::ChickenFrenzy => [EventId::CritterBurst, EventId::TrafficSurge],
        ConditionId::Stampede => [EventId::ComboFrenzy, EventId::ChickenBurst],
        ConditionId::GlassCannon => [EventId::CritterBurst, EventId::TrafficSurge],
    }
}

pub const COMBO_WINDOW_SECONDS: f32 = 2.5;
pub const COMBO_MAX_MULTIPLIER: u32 = 5;
pub const COMBO_TIERS: [ComboTier; 5] = [
    ComboTier {
        minimum_count: 0,
        multiplier: 1,
    },
    ComboTier {
        minimum_count: 5,
        multiplier: 2,
    },
    ComboTier {
        minimum_count: 10,
        multiplier: 3,
    },
    ComboTier {
        minimum_count: 15,
        multiplier: 4,
    },
    ComboTier {
        minimum_count: 20,
        multiplier: 5,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ComboTier {
    pub minimum_count: u32,
    pub multiplier: u32,
}

pub const fn combo_multiplier(count: u32) -> u32 {
    match count {
        0..=4 => 1,
        5..=9 => 2,
        10..=14 => 3,
        15..=19 => 4,
        _ => 5,
    }
}

/// Bonus added above the hit's separately awarded base point.
pub const fn combo_bonus(multiplier: u32, condition: ConditionId, event: Option<EventId>) -> u32 {
    let event_multiplier = match event {
        Some(event) => event.combo_bonus_multiplier(),
        None => 1,
    };
    multiplier
        .saturating_sub(1)
        .saturating_mul(condition.combo_bonus_multiplier())
        .saturating_mul(event_multiplier)
}

pub const CHICKEN_BASE_AWARD: u32 = 1;

/// Direct award for a chicken hit; combo bonus is intentionally separate.
pub const fn chicken_direct_award(condition: ConditionId, event: Option<EventId>) -> u32 {
    let event_bonus = match event {
        Some(event) => event.chicken_score_bonus(),
        None => 0,
    };
    CHICKEN_BASE_AWARD
        .saturating_add(condition.chicken_score_bonus())
        .saturating_add(event_bonus)
}

pub const OBJECTIVE_BONUS: u32 = 10;
pub const fn award_objective(score: u32) -> u32 {
    score.saturating_add(OBJECTIVE_BONUS)
}

pub const COIN_SCORE_AWARD: u32 = 1;
pub const COIN_TIME_BONUS_SECONDS: f32 = 1.5;
pub const COIN_TIME_CAP_SECONDS: f32 = 90.0;

/// Ordinary coin transition. NaN and negative infinity sanitize to zero;
/// positive infinity and high values sanitize to the cap.
pub fn coin_time_after_collect(current: f32) -> f32 {
    let current = if current.is_nan() {
        0.0
    } else {
        current.clamp(0.0, COIN_TIME_CAP_SECONDS)
    };
    (current + COIN_TIME_BONUS_SECONDS).min(COIN_TIME_CAP_SECONDS)
}

pub const TIME_PICKUP_BONUS_SECONDS: f32 = 5.0;
pub const TIME_PICKUP_CAP_SECONDS: f32 = 99.0;

/// Preserve the game's current pickup behavior exactly: add then cap. Rust's
/// `f32::min` makes a NaN sum resolve to the cap; negative infinity remains.
pub fn time_after_pickup(current: f32) -> f32 {
    (current + TIME_PICKUP_BONUS_SECONDS).min(TIME_PICKUP_CAP_SECONDS)
}

pub const MEGA_COIN_POINTS: u32 = 5;
pub const CRITTER_SCORE_PENALTY: u32 = 2;
pub const fn apply_critter_penalty(chicken_score: u32) -> u32 {
    chicken_score.saturating_sub(CRITTER_SCORE_PENALTY)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalAggregatePolicy {
    CheckedRejectOverflow,
    Saturating,
}

/// Canonical terminal aggregate policy for protocol and leaderboard values.
pub const TERMINAL_AGGREGATE_POLICY: TerminalAggregatePolicy =
    TerminalAggregatePolicy::CheckedRejectOverflow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalScoreOverflow;

pub const fn terminal_total_checked(
    chickens: u32,
    coins: u32,
) -> Result<u32, TerminalScoreOverflow> {
    match chickens.checked_add(coins) {
        Some(total) => Ok(total),
        None => Err(TerminalScoreOverflow),
    }
}

/// Explicit fallback for non-protocol UI/display paths that cannot return an error.
pub const fn terminal_total_saturating(chickens: u32, coins: u32) -> u32 {
    chickens.saturating_add(coins)
}

#[derive(Serialize)]
struct Manifest {
    rules_version: RulesVersion,
    rules_id: &'static str,
    conditions: Vec<ManifestCondition>,
    events: Vec<ManifestEvent>,
    combo: ManifestCombo,
    scoring: ManifestScoring,
    time: ManifestTime,
    terminal_aggregate: ManifestTerminalAggregate,
}

#[derive(Serialize)]
struct ManifestCondition {
    id: &'static str,
    storage_index: usize,
    chicken_score_bonus: u32,
    combo_bonus_multiplier: u32,
    reachable_events: [&'static str; 2],
}

#[derive(Serialize)]
struct ManifestEvent {
    id: &'static str,
    chicken_score_bonus: u32,
    combo_bonus_multiplier: u32,
}

#[derive(Serialize)]
struct ManifestCombo {
    window_seconds: f32,
    tiers: [ComboTier; 5],
    bonus_policy: &'static str,
}

#[derive(Serialize)]
struct ManifestScoring {
    chicken_base_award: u32,
    objective_bonus: u32,
    coin_score_award: u32,
    mega_coin_points: u32,
    critter_score_penalty: u32,
}

#[derive(Serialize)]
struct ManifestTime {
    coin_bonus_seconds: f32,
    coin_cap_seconds: f32,
    time_pickup_bonus_seconds: f32,
    time_pickup_cap_seconds: f32,
}

#[derive(Serialize)]
struct ManifestTerminalAggregate {
    policy: TerminalAggregatePolicy,
    overflow_fallback: &'static str,
}

fn manifest() -> Manifest {
    Manifest {
        rules_version: RULES_VERSION,
        rules_id: RULES_VERSION_ID,
        conditions: CONDITIONS
            .into_iter()
            .map(|condition| {
                let events = reachable_events(condition);
                ManifestCondition {
                    id: condition.as_str(),
                    storage_index: condition.storage_index(),
                    chicken_score_bonus: condition.chicken_score_bonus(),
                    combo_bonus_multiplier: condition.combo_bonus_multiplier(),
                    reachable_events: [events[0].as_str(), events[1].as_str()],
                }
            })
            .collect(),
        events: EVENTS
            .into_iter()
            .map(|event| ManifestEvent {
                id: event.as_str(),
                chicken_score_bonus: event.chicken_score_bonus(),
                combo_bonus_multiplier: event.combo_bonus_multiplier(),
            })
            .collect(),
        combo: ManifestCombo {
            window_seconds: COMBO_WINDOW_SECONDS,
            tiers: COMBO_TIERS,
            bonus_policy: "base_point_unscaled_bonus_saturating",
        },
        scoring: ManifestScoring {
            chicken_base_award: CHICKEN_BASE_AWARD,
            objective_bonus: OBJECTIVE_BONUS,
            coin_score_award: COIN_SCORE_AWARD,
            mega_coin_points: MEGA_COIN_POINTS,
            critter_score_penalty: CRITTER_SCORE_PENALTY,
        },
        time: ManifestTime {
            coin_bonus_seconds: COIN_TIME_BONUS_SECONDS,
            coin_cap_seconds: COIN_TIME_CAP_SECONDS,
            time_pickup_bonus_seconds: TIME_PICKUP_BONUS_SECONDS,
            time_pickup_cap_seconds: TIME_PICKUP_CAP_SECONDS,
        },
        terminal_aggregate: ManifestTerminalAggregate {
            policy: TERMINAL_AGGREGATE_POLICY,
            overflow_fallback: "saturating_for_non_protocol_display_only",
        },
    }
}

/// Canonical pretty JSON (two-space indentation and one trailing newline).
pub fn manifest_pretty_json() -> String {
    serde_json::to_string_pretty(&manifest()).expect("rules manifest is serializable") + "\n"
}

/// JSON Schema for the canonical manifest document.
pub fn schema_pretty_json() -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://roady.game/rules/roady-rules.v1.schema.json",
        "title": "Roady score rules v1",
        "type": "object",
        "additionalProperties": false,
        "required": ["rules_version", "rules_id", "conditions", "events", "combo", "scoring", "time", "terminal_aggregate"],
        "properties": {
            "rules_version": { "const": 1 },
            "rules_id": { "const": RULES_VERSION_ID },
            "conditions": { "type": "array", "minItems": 5, "maxItems": 5, "items": { "$ref": "#/$defs/condition" } },
            "events": { "type": "array", "minItems": 4, "maxItems": 4, "items": { "$ref": "#/$defs/event" } },
            "combo": { "$ref": "#/$defs/combo" },
            "scoring": { "$ref": "#/$defs/scoring" },
            "time": { "$ref": "#/$defs/time" },
            "terminal_aggregate": { "$ref": "#/$defs/terminalAggregate" }
        },
        "$defs": {
            "conditionId": { "enum": (CONDITIONS.map(ConditionId::as_str)) },
            "eventId": { "enum": (EVENTS.map(EventId::as_str)) },
            "condition": {
                "type": "object", "additionalProperties": false,
                "required": ["id", "storage_index", "chicken_score_bonus", "combo_bonus_multiplier", "reachable_events"],
                "properties": {
                    "id": { "$ref": "#/$defs/conditionId" }, "storage_index": { "type": "integer", "minimum": 0, "maximum": 4 },
                    "chicken_score_bonus": { "type": "integer", "minimum": 0 }, "combo_bonus_multiplier": { "type": "integer", "minimum": 1 },
                    "reachable_events": { "type": "array", "minItems": 2, "maxItems": 2, "items": { "$ref": "#/$defs/eventId" } }
                }
            },
            "event": {
                "type": "object", "additionalProperties": false,
                "required": ["id", "chicken_score_bonus", "combo_bonus_multiplier"],
                "properties": { "id": { "$ref": "#/$defs/eventId" }, "chicken_score_bonus": { "type": "integer", "minimum": 0 }, "combo_bonus_multiplier": { "type": "integer", "minimum": 1 } }
            },
            "combo": {
                "type": "object", "additionalProperties": false, "required": ["window_seconds", "tiers", "bonus_policy"],
                "properties": {
                    "window_seconds": { "type": "number", "exclusiveMinimum": 0 },
                    "tiers": { "type": "array", "minItems": 5, "maxItems": 5, "items": { "type": "object", "additionalProperties": false, "required": ["minimum_count", "multiplier"], "properties": { "minimum_count": { "type": "integer", "minimum": 0 }, "multiplier": { "type": "integer", "minimum": 1, "maximum": 5 } } } },
                    "bonus_policy": { "const": "base_point_unscaled_bonus_saturating" }
                }
            },
            "scoring": { "type": "object", "additionalProperties": false, "required": ["chicken_base_award", "objective_bonus", "coin_score_award", "mega_coin_points", "critter_score_penalty"], "properties": { "chicken_base_award": { "type": "integer", "minimum": 0 }, "objective_bonus": { "type": "integer", "minimum": 0 }, "coin_score_award": { "type": "integer", "minimum": 0 }, "mega_coin_points": { "type": "integer", "minimum": 0 }, "critter_score_penalty": { "type": "integer", "minimum": 0 } } },
            "time": { "type": "object", "additionalProperties": false, "required": ["coin_bonus_seconds", "coin_cap_seconds", "time_pickup_bonus_seconds", "time_pickup_cap_seconds"], "properties": { "coin_bonus_seconds": { "type": "number", "minimum": 0 }, "coin_cap_seconds": { "type": "number", "minimum": 0 }, "time_pickup_bonus_seconds": { "type": "number", "minimum": 0 }, "time_pickup_cap_seconds": { "type": "number", "minimum": 0 } } },
            "terminalAggregate": { "type": "object", "additionalProperties": false, "required": ["policy", "overflow_fallback"], "properties": { "policy": { "const": "checked_reject_overflow" }, "overflow_fallback": { "const": "saturating_for_non_protocol_display_only" } } }
        }
    })).expect("rules schema is serializable") + "\n"
}
