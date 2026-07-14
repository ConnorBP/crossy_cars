use roady_score_rules::*;
use std::{collections::BTreeSet, fs, path::PathBuf};

fn rules_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../rules")
}

#[test]
fn checked_in_manifest_is_byte_for_byte_canonical() {
    let checked_in = fs::read(rules_dir().join("roady-rules.v1.json")).unwrap();
    assert_eq!(checked_in, manifest_pretty_json().as_bytes());
}

#[test]
fn checked_in_schema_is_byte_for_byte_canonical() {
    let checked_in = fs::read(rules_dir().join("roady-rules.v1.schema.json")).unwrap();
    assert_eq!(checked_in, schema_pretty_json().as_bytes());
}

#[test]
fn combo_boundary_vectors_cover_every_tier_edge() {
    for (count, expected) in [
        (0, 1),
        (1, 1),
        (4, 1),
        (5, 2),
        (6, 2),
        (9, 2),
        (10, 3),
        (11, 3),
        (14, 3),
        (15, 4),
        (16, 4),
        (19, 4),
        (20, 5),
        (21, 5),
        (u32::MAX, 5),
    ] {
        assert_eq!(combo_multiplier(count), expected, "count {count}");
    }
    assert_eq!(combo_bonus(4, ConditionId::Standard, None), 3);
    assert_eq!(combo_bonus(4, ConditionId::GlassCannon, None), 6);
    assert_eq!(
        combo_bonus(4, ConditionId::Standard, Some(EventId::ComboFrenzy)),
        6
    );
    assert_eq!(
        combo_bonus(4, ConditionId::GlassCannon, Some(EventId::ComboFrenzy)),
        12
    );
    assert_eq!(
        combo_bonus(
            u32::MAX,
            ConditionId::GlassCannon,
            Some(EventId::ComboFrenzy)
        ),
        u32::MAX
    );
}

#[test]
fn condition_and_reachable_event_vectors_are_complete_and_stable() {
    let expected = [
        (
            ConditionId::Standard,
            "standard",
            0,
            [EventId::TrafficSurge, EventId::CritterBurst],
        ),
        (
            ConditionId::RushHour,
            "rush_hour",
            1,
            [EventId::ChickenBurst, EventId::ComboFrenzy],
        ),
        (
            ConditionId::ChickenFrenzy,
            "chicken_frenzy",
            2,
            [EventId::CritterBurst, EventId::TrafficSurge],
        ),
        (
            ConditionId::Stampede,
            "stampede",
            3,
            [EventId::ComboFrenzy, EventId::ChickenBurst],
        ),
        (
            ConditionId::GlassCannon,
            "glass_cannon",
            4,
            [EventId::CritterBurst, EventId::TrafficSurge],
        ),
    ];
    let mut reached = BTreeSet::new();
    for (condition, id, index, events) in expected {
        assert_eq!(condition.as_str(), id);
        assert_eq!(condition.storage_index(), index);
        assert_eq!(reachable_events(condition), events);
        reached.extend(events.map(EventId::as_str));
    }
    assert_eq!(reached, EVENTS.map(EventId::as_str).into_iter().collect());
}

#[test]
fn coin_and_time_boundary_vectors_preserve_transitions() {
    for (current, expected) in [
        (0.0, 1.5),
        (60.0, 61.5),
        (88.5, 90.0),
        (89.5, 90.0),
        (90.0, 90.0),
        (120.0, 90.0),
        (-10.0, 1.5),
        (f32::NEG_INFINITY, 1.5),
        (f32::INFINITY, 90.0),
        (f32::MAX, 90.0),
    ] {
        assert_eq!(
            coin_time_after_collect(current),
            expected,
            "current {current}"
        );
    }
    assert_eq!(coin_time_after_collect(f32::NAN), 1.5);

    for (current, expected) in [
        (0.0, 5.0),
        (93.0, 98.0),
        (94.0, 99.0),
        (99.0, 99.0),
        (120.0, 99.0),
    ] {
        assert_eq!(time_after_pickup(current), expected, "current {current}");
    }
    assert_eq!(time_after_pickup(f32::NAN), 99.0);
    assert_eq!(time_after_pickup(f32::INFINITY), 99.0);
    assert_eq!(time_after_pickup(f32::NEG_INFINITY), f32::NEG_INFINITY);
}

#[test]
fn scoring_and_terminal_overflow_vectors_are_explicit() {
    assert_eq!(chicken_direct_award(ConditionId::Standard, None), 1);
    assert_eq!(chicken_direct_award(ConditionId::ChickenFrenzy, None), 2);
    assert_eq!(
        chicken_direct_award(ConditionId::Standard, Some(EventId::ChickenBurst)),
        2
    );
    assert_eq!(
        chicken_direct_award(ConditionId::ChickenFrenzy, Some(EventId::ChickenBurst)),
        3
    );
    assert_eq!(award_objective(u32::MAX - 5), u32::MAX);
    assert_eq!(apply_critter_penalty(1), 0);
    assert_eq!(terminal_total_checked(20, 22), Ok(42));
    assert_eq!(
        terminal_total_checked(u32::MAX, 1),
        Err(TerminalScoreOverflow)
    );
    assert_eq!(terminal_total_saturating(u32::MAX, 1), u32::MAX);
}
