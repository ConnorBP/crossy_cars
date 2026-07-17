//! Deterministic, language-neutral v3 rules artifacts.
//!
//! These documents are generated from the typed rules implementation. They are
//! intentionally data-only so Rust, TypeScript, WASM, and workerd consumers can
//! share exactly the same immutable inputs and outputs.

use super::canonical::{
    CluckTerminal, ConductTerminal, Event, EventPayload, RightOfWayTerminal, SessionHeader,
    chain_event, score_hmac_input, sha256, started_session_header, unstarted_session_header,
    worker_proof_input,
};
use super::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const STREAM_DOMAINS: [&str; 7] = [
    ROTATION_DOMAIN,
    SCHEDULED_EVENTS_DOMAIN,
    FRENZY_INTERVAL_DOMAIN,
    FRENZY_ROLL_DOMAIN,
    FRENZY_KIND_DOMAIN,
    FRENZY_POSITION_DOMAIN,
    FRENZY_RELOCATION_DOMAIN,
];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn effect_name(value: Effect) -> &'static str {
    match value {
        Effect::Standard => "standard",
        Effect::RushHour => "rush_hour",
        Effect::ChickenFrenzy => "chicken_frenzy",
        Effect::Stampede => "stampede",
        Effect::GlassCannon => "glass_cannon",
    }
}

fn scheduled_event_name(value: ScheduledEvent) -> &'static str {
    match value {
        ScheduledEvent::TrafficSurge => "traffic_surge",
        ScheduledEvent::ChickenBurst => "chicken_burst",
        ScheduledEvent::ComboFrenzy => "combo_frenzy",
        ScheduledEvent::CritterBurst => "critter_burst",
    }
}

fn objective_name(value: Objective) -> &'static str {
    match value {
        Objective::HitChickens => "hit_chickens",
        Objective::CollectCoins => "collect_coins",
        Objective::ReachCombo => "reach_combo",
        Objective::DeliverPackages => "deliver_packages",
        Objective::CourtesyAwards => "courtesy_awards",
    }
}

/// The immutable v3 mechanics and canonical-protocol manifest.
pub fn manifest_document() -> Value {
    json!({
        "rules_version": RULES_VERSION,
        "rules_id": RULES_VERSION_ID,
        "protocol_version": PROTOCOL_VERSION,
        "protocol_id": PROTOCOL_ID,
        "policy_version": POLICY_VERSION,
        "policy_id": POLICY_ID,
        "mode": MODE,
        "categories": [
            { "conduct": "cluck_hunt", "ordinal": Conduct::CluckHunt as u8, "key": CLUCK_HUNT_CATEGORY },
            { "conduct": "right_of_way", "ordinal": Conduct::RightOfWay as u8, "key": RIGHT_OF_WAY_CATEGORY }
        ],
        "ordinals": {
            "effects": { "standard": 0, "rush_hour": 1, "chicken_frenzy": 2, "stampede": 3, "glass_cannon": 4 },
            "scheduled_events": { "traffic_surge": 0, "chicken_burst": 1, "combo_frenzy": 2, "critter_burst": 3 },
            "event_kinds": {
                "chicken_hit": 1, "coin_collected": 2, "time_pickup": 3,
                "objective_completed": 4, "critter_penalty": 5, "segment_changed": 6,
                "terminal": 7, "package_pickup": 8, "package_delivery": 9,
                "courtesy_award": 10, "animal_hit": 11, "wave_award": 12,
                "coin_award": 13, "frenzy_changed": 14
            },
            "terminal_reasons": { "time_up": 1, "wrecked": 2, "drowned": 3 },
            "platforms": { "web": 1, "native": 2 },
            "objectives": {
                "hit_chickens": 1, "collect_coins": 2, "reach_combo": 3,
                "deliver_packages": 4, "courtesy_awards": 5
            },
            "frenzy_phases": { "spawned": 1, "telegraph": 2, "active": 3, "expired": 4 }
        },
        "limits": {
            "schedule_segments": SCHEDULE_SEGMENTS,
            "max_events": canonical::MAX_EVENTS,
            "max_event_record_bytes": canonical::MAX_EVENT_RECORD_BYTES,
            "max_ledger_bytes": canonical::MAX_LEDGER_BYTES,
            "max_evidence_bytes": canonical::MAX_EVIDENCE_BYTES,
            "max_lp1_bytes": 255,
            "max_lp4_bytes": canonical::MAX_LP4_BYTES,
            "max_build_bytes": canonical::MAX_BUILD_BYTES,
            "max_remaining_ms": TIME_PICKUP_CAP_MS
        },
        "rotation": {
            "initial_grace_ms": INITIAL_GRACE_MS,
            "telegraph_ms": TELEGRAPH_MS,
            "active_ms": ACTIVE_MS,
            "cooldown_ms": COOLDOWN_MS,
            "cadence_ms": CADENCE_MS,
            "pool": [Effect::RushHour as u8, Effect::Stampede as u8, Effect::GlassCannon as u8],
            "anti_repeat": "one_retry_then_cyclic_successor",
            "scheduled_event_windows_ms": EVENT_WINDOWS
        },
        "prng": {
            "algorithm": "splitmix64",
            "range_mapping": "next_u64_mod_n",
            "increment_hex": "9e3779b97f4a7c15",
            "multiplier_1_hex": "bf58476d1ce4e5b9",
            "multiplier_2_hex": "94d049bb133111eb",
            "stream_derivation": "fnv1a64_seed_domain_length_le_domain_then_splitmix_finalizer",
            "fnv_offset_hex": "cbf29ce484222325",
            "fnv_prime_hex": "00000100000001b3",
            "domains": STREAM_DOMAINS
        },
        "cluck_hunt": {
            "combo_window_ms": COMBO_WINDOW_MS,
            "combo_tiers": [
                { "minimum_count": 0, "multiplier": 1 },
                { "minimum_count": 5, "multiplier": 2 },
                { "minimum_count": 10, "multiplier": 3 },
                { "minimum_count": 15, "multiplier": 4 },
                { "minimum_count": 20, "multiplier": 5 }
            ],
            "chicken_base_award": 1,
            "objective_award": OBJECTIVE_AWARD,
            "ranked_wave_award": RANKED_WAVE_AWARD,
            "coin_award": COIN_AWARD,
            "mega_coin_award": MEGA_COIN_AWARD,
            "critter_penalty": CRITTER_PENALTY,
            "terminal_policy": "checked_chickens_plus_coins",
            "live_bucket_policy": "saturating"
        },
        "time_and_health": {
            "coin_bonus_ms": COIN_TIME_BONUS_MS,
            "package_bonus_ms": PACKAGE_TIME_BONUS_MS,
            "time_pickup_bonus_ms": TIME_PICKUP_BONUS_MS,
            "coin_and_package_cap_ms": COIN_AND_PACKAGE_TIME_CAP_MS,
            "time_pickup_cap_ms": TIME_PICKUP_CAP_MS,
            "health_pickup": 35,
            "health_cap": 100
        },
        "right_of_way": {
            "package_capacity": PACKAGE_CAPACITY,
            "package_base_award": PACKAGE_BASE_AWARD,
            "courtesy_base_award": COURTESY_BASE_AWARD,
            "animal_hit_delta": ANIMAL_HIT_DELTA,
            "initial_premium_bps": INITIAL_PREMIUM_BPS,
            "premium_decay_bps": PREMIUM_DECAY_BPS,
            "guilt_multiplier_bps": GUILT_MULTIPLIER_BPS,
            "guilt_ms": GUILT_MS,
            "courtesy_cooldown_ms": COURTESY_COOLDOWN_MS,
            "courtesy_min_speed": COURTESY_MIN_SPEED,
            "chicken_hit_radius": CHICKEN_HIT_RADIUS,
            "courtesy_outer_radius": COURTESY_OUTER_RADIUS,
            "positive_award_policy": "floor_base_times_premium_then_floor_guilt",
            "accumulator_policy": "checked_i64_terminal_max_zero_checked_u32"
        },
        "frenzy": {
            "eligible_ms": FRENZY_ELIGIBLE_MS,
            "interval_span": FRENZY_INTERVAL_SPAN,
            "roll_range": FRENZY_ROLL_RANGE,
            "success_range": FRENZY_SUCCESS_RANGE,
            "pity_ms": FRENZY_PITY_MS,
            "orb_lifetime_ms": FRENZY_ORB_LIFETIME_MS,
            "relocation_age_ms": FRENZY_RELOCATION_AGE_MS,
            "telegraph_ms": FRENZY_TELEGRAPH_MS,
            "active_ms": FRENZY_ACTIVE_MS,
            "relocation_candidates": 8,
            "relocation_draws": 16
        },
        "population": {
            "traffic_target": "min(min(1+floor(level/2),8)*rush*surge,8)",
            "traffic_speed": "min((5+level*0.7)*(0.85+roll*0.30)*rush_speed*surge_speed,11.5)",
            "chicken_target": "min(14*chicken_burst*frenzy,40)",
            "critter_target": "min(5*stampede+critter_burst_extra,16)"
        },
        "objectives": {
            "cluck_hunt": [
                { "objective": "hit_chickens", "target": 10 },
                { "objective": "collect_coins", "target": 6 },
                { "objective": "reach_combo", "target": 3 }
            ],
            "right_of_way": [
                { "objective": "deliver_packages", "target": 3 },
                { "objective": "courtesy_awards", "target": 3 },
                { "objective": "collect_coins", "target": 6 }
            ]
        },
        "canonical_encoding": {
            "integer_byte_order": "big_endian",
            "hash": "sha256",
            "domains": {
                "session": "roady.v3.session", "score": "roady.v3.score",
                "event": "roady.v3.event", "root": "roady.v3.root",
                "schedule": "roady.v3.schedule", "seed": "roady.v3.seed",
                "proof": "roady.v3.proof", "evidence": "roady.v3.evidence"
            },
            "event_record": "lp1(event_domain)||u32(seq)||u64(active_ms)||u8(kind)||payload",
            "event_hash": "sha256(previous_hash32||event_record)",
            "stored_event": "event_record||event_hash32",
            "evidence": "lp1(evidence_domain)||lp1(session_id)||u32(event_count)||lp4(stored_events)",
            "final_root": "sha256(lp1(root_domain)||h0||hN||conduct_aggregates)",
            "same_ms_order": [
                "expirations", "segment_ends", "segment_starts", "activation_spawns",
                "collections", "activations", "gameplay_kind_then_stable_id", "final_objective_reward",
                "terminal", "final_root", "game_over_snapshot"
            ],
            "payload_layouts": {
                "chicken_hit": "u32,u32,u32,u8,u8,u32,u32",
                "coin_collected": "u8,u32,u8,u8,u32,u32,u64,u64",
                "time_pickup": "u64,u64",
                "objective_completed_cluck": "u8,u32,u32,u32,u32",
                "critter_penalty": "u32,u32,u32,u64",
                "segment_changed": "u8,u8,u8,u64,u64",
                "terminal_cluck": "u8,u8,u32,u32,u32,u8,u8,u64,u64,lp1,u8",
                "package_pickup": "u8,u8",
                "package_delivery": "u8,u32,u32,u32,u8,u32,i64,i64,u64,u64",
                "courtesy_award": "u32,u32,u8,u32,i64,i64,u32",
                "animal_hit": "u8,i32,u32,u32,u64,i64,i64",
                "wave_award": "u32,u32,u8,u32,i64,i64",
                "coin_award": "u32,u32,u8,u32,i64,i64,u64,u64",
                "frenzy_changed": "u8,u64,u64",
                "objective_completed_right_of_way": "u8,u32,u32,u32,u8,u32,i64,i64",
                "terminal_right_of_way": "u8,u8,u32,i64,u32,u32,u32,u32,u32,u8,u64,u64,lp1,u8"
            }
        }
    })
}

/// A strict schema: a v3 manifest is valid only when it equals this immutable
/// generated contract. This intentionally rejects additions as well as edits.
pub fn schema_document() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://roady.game/rules/roady-rules.v3.schema.json",
        "title": "Roady immutable gameplay and canonical protocol rules v3",
        "description": "The generated v3 manifest is an exact immutable contract.",
        "type": "object",
        "const": manifest_document()
    })
}

fn seed(number: u8) -> [u8; 32] {
    core::array::from_fn(|index| number.wrapping_add(index as u8))
}

fn schedule_vector(number: u8) -> Value {
    let seed = seed(number);
    let windows = rotation_schedule(&seed);
    let events = scheduled_events(&seed, &windows);
    json!({
        "number": number,
        "input": { "seed_hex": hex(&seed) },
        "output": {
            "windows": windows.iter().map(|window| json!({
                "effect": effect_name(window.effect),
                "effect_ordinal": window.effect as u8,
                "telegraph_start_ms": window.telegraph_start_ms,
                "active_start_ms": window.active_start_ms,
                "active_end_ms": window.active_end_ms,
                "cooldown_end_ms": window.cooldown_end_ms
            })).collect::<Vec<_>>(),
            "scheduled_events": events.map(|event| json!({
                "name": scheduled_event_name(event), "ordinal": event as u8
            })),
            "seed_commitment_hex": hex(&seed_commitment(&seed)),
            "schedule_commitment_cluck_hunt_hex": hex(&schedule_commitment(&seed, CLUCK_HUNT_CATEGORY)),
            "schedule_commitment_right_of_way_hex": hex(&schedule_commitment(&seed, RIGHT_OF_WAY_CATEGORY))
        }
    })
}

fn stream_vectors() -> Vec<Value> {
    let seed = seed(1);
    STREAM_DOMAINS
        .into_iter()
        .map(|domain| {
            let mut rng = stream(&seed, domain);
            json!({
                "input": { "seed_hex": hex(&seed), "domain": domain },
                "output": {
                    "fnv_hex": format!("{:016x}", stream_fnv(&seed, domain)),
                    "initial_state_hex": format!("{:016x}", stream_state(&seed, domain)),
                    "first_three_u64_hex": [
                        format!("{:016x}", rng.next_u64()),
                        format!("{:016x}", rng.next_u64()),
                        format!("{:016x}", rng.next_u64())
                    ]
                }
            })
        })
        .collect()
}

fn terminal_cluck(build: &str) -> ConductTerminal {
    ConductTerminal::CluckHunt(CluckTerminal {
        reason: TerminalReason::Wrecked,
        total: 42,
        chickens: 35,
        coins: 7,
        objective_completed: true,
        max_combo: 5,
        duration_ms: 123_456,
        remaining_ms: 9_876,
        build: build.into(),
        platform: Platform::Native,
    })
}

fn terminal_row(build: &str) -> ConductTerminal {
    ConductTerminal::RightOfWay(RightOfWayTerminal {
        reason: TerminalReason::TimeUp,
        total: 17,
        accumulator: 17,
        premium_bps: 9_000,
        packages_delivered: 3,
        courtesy_count: 2,
        animal_hits: 1,
        max_delivery_chain: 3,
        objective_completed: true,
        duration_ms: 60_000,
        remaining_ms: 5_000,
        build: build.into(),
        platform: Platform::Web,
    })
}

fn all_event_payloads() -> Vec<(&'static str, EventPayload)> {
    vec![
        (
            "chicken_hit",
            EventPayload::ChickenHit {
                base: u32::MAX,
                event_bonus: u32::MAX,
                frenzy_bonus: u32::MAX,
                combo_before: u8::MAX,
                combo_after: u8::MAX,
                bucket_before: u32::MAX,
                bucket_after: u32::MAX,
            },
        ),
        (
            "coin_collected",
            EventPayload::CoinCollected {
                mega: true,
                base: u32::MAX,
                combo_before: u8::MAX,
                combo_after: u8::MAX,
                bucket_before: u32::MAX,
                bucket_after: u32::MAX,
                remaining_before_ms: u64::MAX,
                remaining_after_ms: u64::MAX,
            },
        ),
        (
            "time_pickup",
            EventPayload::TimePickup {
                remaining_before_ms: u64::MAX,
                remaining_after_ms: u64::MAX,
            },
        ),
        (
            "objective_completed_cluck",
            EventPayload::ObjectiveCompletedCluck {
                objective: Objective::ReachCombo,
                target: u32::MAX,
                base_reward: u32::MAX,
                bucket_before: u32::MAX,
                bucket_after: u32::MAX,
            },
        ),
        (
            "critter_penalty",
            EventPayload::CritterPenalty {
                penalty: u32::MAX,
                bucket_before: u32::MAX,
                bucket_after: u32::MAX,
                cooldown_after_ms: u64::MAX,
            },
        ),
        (
            "segment_changed",
            EventPayload::SegmentChanged {
                segment_kind: u8::MAX,
                effect_or_event: u8::MAX,
                active: true,
                start_ms: u64::MAX,
                end_ms: u64::MAX,
            },
        ),
        (
            "terminal_cluck",
            EventPayload::Terminal(terminal_cluck(&"x".repeat(canonical::MAX_BUILD_BYTES))),
        ),
        (
            "package_pickup",
            EventPayload::PackagePickup {
                carried_before: u8::MAX,
                carried_after: u8::MAX,
            },
        ),
        (
            "package_delivery",
            EventPayload::PackageDelivery {
                delivered_ordinal_within_dropoff: u8::MAX,
                chain_index: u32::MAX,
                base: u32::MAX,
                premium_bps: u32::MAX,
                guilt: true,
                credited: u32::MAX,
                accumulator_before: i64::MIN,
                accumulator_after: i64::MAX,
                remaining_before_ms: u64::MAX,
                remaining_after_ms: u64::MAX,
            },
        ),
        (
            "courtesy_award",
            EventPayload::CourtesyAward {
                chicken_stable_id: u32::MAX,
                premium_bps: u32::MAX,
                guilt: true,
                credited: u32::MAX,
                accumulator_before: i64::MIN,
                accumulator_after: i64::MAX,
                cooldown_after_ms: u32::MAX,
            },
        ),
        (
            "animal_hit",
            EventPayload::AnimalHit {
                animal_kind: u8::MAX,
                delta: i32::MIN,
                premium_before_bps: u32::MAX,
                premium_after_bps: u32::MAX,
                guilt_after_ms: u64::MAX,
                accumulator_before: i64::MIN,
                accumulator_after: i64::MAX,
            },
        ),
        (
            "wave_award",
            EventPayload::WaveAward {
                base: u32::MAX,
                premium_bps: u32::MAX,
                guilt: true,
                credited: u32::MAX,
                accumulator_before: i64::MIN,
                accumulator_after: i64::MAX,
            },
        ),
        (
            "coin_award",
            EventPayload::CoinAward {
                base: u32::MAX,
                premium_bps: u32::MAX,
                guilt: true,
                credited: u32::MAX,
                accumulator_before: i64::MIN,
                accumulator_after: i64::MAX,
                remaining_before_ms: u64::MAX,
                remaining_after_ms: u64::MAX,
            },
        ),
        (
            "frenzy_changed",
            EventPayload::FrenzyChanged {
                phase: FrenzyPhase::Expired,
                start_ms: u64::MAX,
                end_ms: u64::MAX,
            },
        ),
        (
            "objective_completed_right_of_way",
            EventPayload::ObjectiveCompletedRightOfWay {
                objective: Objective::CourtesyAwards,
                target: u32::MAX,
                base: u32::MAX,
                premium_bps: u32::MAX,
                guilt: true,
                credited: u32::MAX,
                accumulator_before: i64::MIN,
                accumulator_after: i64::MAX,
            },
        ),
        (
            "terminal_right_of_way",
            EventPayload::Terminal(terminal_row(&"x".repeat(canonical::MAX_BUILD_BYTES))),
        ),
    ]
}

fn event_input(payload: &EventPayload) -> Value {
    match payload {
        EventPayload::ChickenHit {
            base,
            event_bonus,
            frenzy_bonus,
            combo_before,
            combo_after,
            bucket_before,
            bucket_after,
        } => {
            json!({ "base": base, "event_bonus": event_bonus, "frenzy_bonus": frenzy_bonus, "combo_before": combo_before, "combo_after": combo_after, "bucket_before": bucket_before, "bucket_after": bucket_after })
        }
        EventPayload::CoinCollected {
            mega,
            base,
            combo_before,
            combo_after,
            bucket_before,
            bucket_after,
            remaining_before_ms,
            remaining_after_ms,
        } => {
            json!({ "mega": mega, "base": base, "combo_before": combo_before, "combo_after": combo_after, "bucket_before": bucket_before, "bucket_after": bucket_after, "remaining_before_ms": remaining_before_ms.to_string(), "remaining_after_ms": remaining_after_ms.to_string() })
        }
        EventPayload::TimePickup {
            remaining_before_ms,
            remaining_after_ms,
        } => {
            json!({ "remaining_before_ms": remaining_before_ms.to_string(), "remaining_after_ms": remaining_after_ms.to_string() })
        }
        EventPayload::ObjectiveCompletedCluck {
            objective,
            target,
            base_reward,
            bucket_before,
            bucket_after,
        } => {
            json!({ "objective": objective_name(*objective), "objective_ordinal": *objective as u8, "target": target, "base_reward": base_reward, "bucket_before": bucket_before, "bucket_after": bucket_after })
        }
        EventPayload::CritterPenalty {
            penalty,
            bucket_before,
            bucket_after,
            cooldown_after_ms,
        } => {
            json!({ "penalty": penalty, "bucket_before": bucket_before, "bucket_after": bucket_after, "cooldown_after_ms": cooldown_after_ms.to_string() })
        }
        EventPayload::SegmentChanged {
            segment_kind,
            effect_or_event,
            active,
            start_ms,
            end_ms,
        } => {
            json!({ "segment_kind": segment_kind, "effect_or_event": effect_or_event, "active": active, "start_ms": start_ms.to_string(), "end_ms": end_ms.to_string() })
        }
        EventPayload::Terminal(terminal) => terminal_input(terminal),
        EventPayload::PackagePickup {
            carried_before,
            carried_after,
        } => json!({ "carried_before": carried_before, "carried_after": carried_after }),
        EventPayload::PackageDelivery {
            delivered_ordinal_within_dropoff,
            chain_index,
            base,
            premium_bps,
            guilt,
            credited,
            accumulator_before,
            accumulator_after,
            remaining_before_ms,
            remaining_after_ms,
        } => {
            json!({ "delivered_ordinal_within_dropoff": delivered_ordinal_within_dropoff, "chain_index": chain_index, "base": base, "premium_bps": premium_bps, "guilt": guilt, "credited": credited, "accumulator_before": accumulator_before.to_string(), "accumulator_after": accumulator_after.to_string(), "remaining_before_ms": remaining_before_ms.to_string(), "remaining_after_ms": remaining_after_ms.to_string() })
        }
        EventPayload::CourtesyAward {
            chicken_stable_id,
            premium_bps,
            guilt,
            credited,
            accumulator_before,
            accumulator_after,
            cooldown_after_ms,
        } => {
            json!({ "chicken_stable_id": chicken_stable_id, "premium_bps": premium_bps, "guilt": guilt, "credited": credited, "accumulator_before": accumulator_before.to_string(), "accumulator_after": accumulator_after.to_string(), "cooldown_after_ms": cooldown_after_ms })
        }
        EventPayload::AnimalHit {
            animal_kind,
            delta,
            premium_before_bps,
            premium_after_bps,
            guilt_after_ms,
            accumulator_before,
            accumulator_after,
        } => {
            json!({ "animal_kind": animal_kind, "delta": delta, "premium_before_bps": premium_before_bps, "premium_after_bps": premium_after_bps, "guilt_after_ms": guilt_after_ms.to_string(), "accumulator_before": accumulator_before.to_string(), "accumulator_after": accumulator_after.to_string() })
        }
        EventPayload::WaveAward {
            base,
            premium_bps,
            guilt,
            credited,
            accumulator_before,
            accumulator_after,
        } => {
            json!({ "base": base, "premium_bps": premium_bps, "guilt": guilt, "credited": credited, "accumulator_before": accumulator_before.to_string(), "accumulator_after": accumulator_after.to_string() })
        }
        EventPayload::CoinAward {
            base,
            premium_bps,
            guilt,
            credited,
            accumulator_before,
            accumulator_after,
            remaining_before_ms,
            remaining_after_ms,
        } => {
            json!({ "base": base, "premium_bps": premium_bps, "guilt": guilt, "credited": credited, "accumulator_before": accumulator_before.to_string(), "accumulator_after": accumulator_after.to_string(), "remaining_before_ms": remaining_before_ms.to_string(), "remaining_after_ms": remaining_after_ms.to_string() })
        }
        EventPayload::FrenzyChanged {
            phase,
            start_ms,
            end_ms,
        } => {
            json!({ "phase": *phase as u8, "start_ms": start_ms.to_string(), "end_ms": end_ms.to_string() })
        }
        EventPayload::ObjectiveCompletedRightOfWay {
            objective,
            target,
            base,
            premium_bps,
            guilt,
            credited,
            accumulator_before,
            accumulator_after,
        } => {
            json!({ "objective": objective_name(*objective), "objective_ordinal": *objective as u8, "target": target, "base": base, "premium_bps": premium_bps, "guilt": guilt, "credited": credited, "accumulator_before": accumulator_before.to_string(), "accumulator_after": accumulator_after.to_string() })
        }
    }
}

fn terminal_input(terminal: &ConductTerminal) -> Value {
    match terminal {
        ConductTerminal::CluckHunt(value) => json!({
            "conduct": "cluck_hunt", "conduct_ordinal": 0, "reason": value.reason as u8,
            "total": value.total, "chickens": value.chickens, "coins": value.coins,
            "objective_completed": value.objective_completed, "max_combo": value.max_combo,
            "duration_ms": value.duration_ms, "remaining_ms": value.remaining_ms,
            "build": value.build, "platform": value.platform as u8
        }),
        ConductTerminal::RightOfWay(value) => json!({
            "conduct": "right_of_way", "conduct_ordinal": 1, "reason": value.reason as u8,
            "total": value.total, "accumulator": value.accumulator.to_string(),
            "premium_bps": value.premium_bps, "packages_delivered": value.packages_delivered,
            "courtesy_count": value.courtesy_count, "animal_hits": value.animal_hits,
            "max_delivery_chain": value.max_delivery_chain,
            "objective_completed": value.objective_completed, "duration_ms": value.duration_ms,
            "remaining_ms": value.remaining_ms, "build": value.build, "platform": value.platform as u8
        }),
    }
}

fn event_vectors() -> Vec<Value> {
    all_event_payloads().into_iter().enumerate().map(|(index, (name, payload))| {
        let previous_hash = [index as u8; 32];
        let event = Event { seq: index as u32, active_ms: 0x0102_0304_0506_0708, payload };
        let stored = chain_event(&previous_hash, &event).expect("fixture event is canonical");
        json!({
            "name": name,
            "input": {
                "previous_hash_hex": hex(&previous_hash), "seq": event.seq,
                "active_ms": event.active_ms.to_string(), "kind": event.payload.kind() as u8,
                "payload": event_input(&event.payload)
            },
            "output": {
                "event_record_hex": hex(&stored.record), "event_record_length": stored.record.len(),
                "event_hash_hex": hex(&stored.event_hash),
                "stored_event_hex": hex(&stored.canonical_bytes()),
                "stored_event_length": stored.record.len() + 32,
                "fits_max_event_record_bytes": stored.record.len() <= canonical::MAX_EVENT_RECORD_BYTES
            }
        })
    }).collect()
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut key_block = [0u8; 64];
    if key.len() > key_block.len() {
        key_block[..32].copy_from_slice(&sha256(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; 64];
    let mut outer_pad = [0x5cu8; 64];
    for index in 0..64 {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_hash);
    outer.finalize().into()
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        result.push(ALPHABET[((value >> 18) & 63) as usize] as char);
        result.push(ALPHABET[((value >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            result.push(ALPHABET[((value >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            result.push(ALPHABET[(value & 63) as usize] as char);
        }
    }
    result
}

fn reason_terminal(conduct: Conduct, reason: TerminalReason) -> ConductTerminal {
    match conduct {
        Conduct::CluckHunt => ConductTerminal::CluckHunt(CluckTerminal {
            reason,
            total: 42,
            chickens: 35,
            coins: 7,
            objective_completed: true,
            max_combo: 5,
            duration_ms: 60_000,
            remaining_ms: 5_000,
            build: "dev".into(),
            platform: Platform::Web,
        }),
        Conduct::RightOfWay => ConductTerminal::RightOfWay(RightOfWayTerminal {
            reason,
            total: 17,
            accumulator: 17,
            premium_bps: 9_000,
            packages_delivered: 3,
            courtesy_count: 2,
            animal_hits: 1,
            max_delivery_chain: 3,
            objective_completed: true,
            duration_ms: 60_000,
            remaining_ms: 5_000,
            build: "dev".into(),
            platform: Platform::Web,
        }),
    }
}

fn terminal_reason_vectors() -> Vec<Value> {
    [TerminalReason::TimeUp, TerminalReason::Wrecked, TerminalReason::Drowned]
        .into_iter().flat_map(|reason| [Conduct::CluckHunt, Conduct::RightOfWay].into_iter().map(move |conduct| {
            let terminal = reason_terminal(conduct, reason);
            let event = Event { seq: 0, active_ms: 60_000, payload: EventPayload::Terminal(terminal.clone()) };
            json!({
                "conduct": match conduct { Conduct::CluckHunt => "cluck_hunt", Conduct::RightOfWay => "right_of_way" },
                "reason": match reason { TerminalReason::TimeUp => "time_up", TerminalReason::Wrecked => "wrecked", TerminalReason::Drowned => "drowned" },
                "reason_ordinal": reason as u8,
                "terminal": terminal_input(&terminal),
                "conduct_aggregates_hex": hex(&terminal.canonical_bytes().unwrap()),
                "event_record_hex": hex(&canonical::event_record(&event).unwrap())
            })
        })).collect()
}

fn drowned_vectors() -> Vec<Value> {
    let seed = seed(1);
    [Conduct::CluckHunt, Conduct::RightOfWay].into_iter().map(|conduct| {
        let category = match conduct { Conduct::CluckHunt => CLUCK_HUNT_CATEGORY, Conduct::RightOfWay => RIGHT_OF_WAY_CATEGORY };
        let seed_hash = seed_commitment(&seed);
        let schedule_hash = schedule_commitment(&seed, category);
        let header = SessionHeader { category, session_id: "S03", challenge: "C03", seed_commitment: &seed_hash, schedule_hash: &schedule_hash, issued_at_ms: 1_000 };
        let unstarted = unstarted_session_header(&header, 301_000).unwrap();
        let started = started_session_header(&header, 2_000).unwrap();
        let terminal = reason_terminal(conduct, TerminalReason::Drowned);
        let event = Event { seq: 0, active_ms: 60_000, payload: EventPayload::Terminal(terminal.clone()) };
        let mut ledger = canonical::CanonicalLedger::new(&started);
        ledger.append(&event).unwrap();
        let evidence = ledger.evidence_bytes("S03").unwrap();
        let root = ledger.final_root().unwrap();
        let score_input = score_hmac_input(category, "S03", &root, &schedule_hash, &seed_hash, &terminal).unwrap();
        json!({
            "conduct": match conduct { Conduct::CluckHunt => "cluck_hunt", Conduct::RightOfWay => "right_of_way" },
            "input": {
                "category": category, "session_id": "S03", "challenge": "C03", "issued_at_ms": "1000",
                "start_by_expiry_ms": "301000", "started_at_ms": "2000", "seed_hex": hex(&seed),
                "proof_key_utf8": "roady-v3-test-proof-key", "client_key_utf8": "roady-v3-test-client-key",
                "terminal": terminal_input(&terminal)
            },
            "output": {
                "seed_commitment_hex": hex(&seed_hash), "schedule_hash_hex": hex(&schedule_hash),
                "unstarted_header_hex": hex(&unstarted), "started_header_hex": hex(&started),
                "unstarted_proof_input_hex": hex(&worker_proof_input(&unstarted)),
                "unstarted_proof_base64url": base64url(&hmac_sha256(b"roady-v3-test-proof-key", &worker_proof_input(&unstarted))),
                "started_proof_input_hex": hex(&worker_proof_input(&started)),
                "started_proof_base64url": base64url(&hmac_sha256(b"roady-v3-test-proof-key", &worker_proof_input(&started))),
                "h0_hex": hex(ledger.h0()), "terminal_event_record_hex": hex(&canonical::event_record(&event).unwrap()),
                "terminal_event_hash_hex": hex(ledger.last_hash()), "stored_ledger_hex": hex(ledger.stored_bytes()),
                "evidence_bytes_hex": hex(&evidence), "evidence_hash_hex": hex(&sha256(&evidence)),
                "conduct_aggregates_hex": hex(&terminal.canonical_bytes().unwrap()), "final_root_hex": hex(&root),
                "score_input_hex": hex(&score_input),
                "score_hmac_base64url": base64url(&hmac_sha256(b"roady-v3-test-client-key", &score_input))
            }
        })
    }).collect()
}

fn canonical_vectors() -> Value {
    let seed = seed(1);
    let seed_hash = seed_commitment(&seed);
    let schedule_hash = schedule_commitment(&seed, RIGHT_OF_WAY_CATEGORY);
    let header_input = SessionHeader {
        category: RIGHT_OF_WAY_CATEGORY,
        session_id: "S01",
        challenge: "C01",
        seed_commitment: &seed_hash,
        schedule_hash: &schedule_hash,
        issued_at_ms: 1_000,
    };
    let unstarted = unstarted_session_header(&header_input, 301_000).unwrap();
    let started = started_session_header(&header_input, 2_000).unwrap();
    let unstarted_proof_input = worker_proof_input(&unstarted);
    let started_proof_input = worker_proof_input(&started);
    let proof_key = b"roady-v3-test-proof-key";

    let mut ledger = canonical::CanonicalLedger::new(&started);
    let delivery = Event {
        seq: 0,
        active_ms: 59_000,
        payload: EventPayload::PackageDelivery {
            delivered_ordinal_within_dropoff: 0,
            chain_index: 0,
            base: 5,
            premium_bps: 9_000,
            guilt: true,
            credited: 2,
            accumulator_before: 15,
            accumulator_after: 17,
            remaining_before_ms: 2_000,
            remaining_after_ms: 5_000,
        },
    };
    ledger.append(&delivery).unwrap();
    let terminal = terminal_row("dev");
    ledger
        .append(&Event {
            seq: 1,
            active_ms: 60_000,
            payload: EventPayload::Terminal(terminal.clone()),
        })
        .unwrap();
    let evidence = ledger.evidence_bytes("S01").unwrap();
    let final_root = ledger.final_root().unwrap();

    let score_key = b"roady-v3-test-client-key";
    let row_golden_input = score_hmac_input(
        RIGHT_OF_WAY_CATEGORY,
        "S01",
        &[0x11; 32],
        &schedule_hash,
        &seed_hash,
        &terminal,
    )
    .unwrap();
    let row_golden_hmac = base64url(&hmac_sha256(score_key, &row_golden_input));

    let cluck_terminal = terminal_cluck("dev");
    let cluck_score_input = score_hmac_input(
        CLUCK_HUNT_CATEGORY,
        "S01",
        &final_root,
        &schedule_commitment(&seed, CLUCK_HUNT_CATEGORY),
        &seed_hash,
        &cluck_terminal,
    )
    .unwrap();

    json!({
        "session": {
            "input": {
                "category": RIGHT_OF_WAY_CATEGORY, "session_id": "S01", "challenge": "C01",
                "seed_commitment_hex": hex(&seed_hash), "schedule_hash_hex": hex(&schedule_hash),
                "issued_at_ms": 1000, "start_by_expiry_ms": 301000, "started_at_ms": 2000,
                "proof_key_utf8": "roady-v3-test-proof-key"
            },
            "output": {
                "unstarted_header_hex": hex(&unstarted), "unstarted_header_length": unstarted.len(),
                "started_header_hex": hex(&started), "started_header_length": started.len(),
                "unstarted_proof_input_hex": hex(&unstarted_proof_input),
                "unstarted_proof_base64url": base64url(&hmac_sha256(proof_key, &unstarted_proof_input)),
                "started_proof_input_hex": hex(&started_proof_input),
                "started_proof_base64url": base64url(&hmac_sha256(proof_key, &started_proof_input)),
                "h0_hex": hex(&sha256(&started))
            }
        },
        "schedule": {
            "input": { "seed_hex": hex(&seed), "category": RIGHT_OF_WAY_CATEGORY },
            "output": {
                "bytes_hex": hex(&canonical::schedule_bytes(&seed, RIGHT_OF_WAY_CATEGORY).unwrap()),
                "sha256_hex": hex(&schedule_hash)
            }
        },
        "events": event_vectors(),
        "ledger": {
            "input": {
                "started_header_hex": hex(&started),
                "events": [
                    { "seq": 0, "active_ms": 59000, "payload": event_input(&delivery.payload) },
                    { "seq": 1, "active_ms": 60000, "payload": terminal_input(&terminal) }
                ],
                "session_id": "S01"
            },
            "output": {
                "h0_hex": hex(ledger.h0()), "hN_hex": hex(ledger.last_hash()),
                "event_count": ledger.event_count(), "stored_ledger_hex": hex(ledger.stored_bytes()),
                "stored_ledger_length": ledger.stored_bytes().len(),
                "evidence_bytes_hex": hex(&evidence), "evidence_bytes_length": evidence.len(),
                "evidence_hash_hex": hex(&sha256(&evidence)),
                "conduct_aggregates_hex": hex(&terminal.canonical_bytes().unwrap()),
                "final_root_hex": hex(&final_root)
            }
        },
        "score_hmac_right_of_way_contract_golden": {
            "input": {
                "key_utf8": "roady-v3-test-client-key", "category": RIGHT_OF_WAY_CATEGORY,
                "session_id": "S01", "final_root_hex": hex(&[0x11; 32]),
                "schedule_hash_hex": hex(&schedule_hash), "seed_commitment_hex": hex(&seed_hash),
                "terminal": terminal_input(&terminal)
            },
            "output": {
                "score_input_hex": hex(&row_golden_input), "score_input_length": row_golden_input.len(),
                "hmac_sha256_base64url": row_golden_hmac
            }
        },
        "score_hmac_cluck_hunt": {
            "input": {
                "key_utf8": "roady-v3-test-client-key", "category": CLUCK_HUNT_CATEGORY,
                "session_id": "S01", "final_root_hex": hex(&final_root),
                "schedule_hash_hex": hex(&schedule_commitment(&seed, CLUCK_HUNT_CATEGORY)),
                "seed_commitment_hex": hex(&seed_hash), "terminal": terminal_input(&cluck_terminal)
            },
            "output": {
                "score_input_hex": hex(&cluck_score_input), "score_input_length": cluck_score_input.len(),
                "hmac_sha256_base64url": base64url(&hmac_sha256(score_key, &cluck_score_input))
            }
        }
    })
}

fn result_u32(value: Result<u32, ArithmeticError>) -> Value {
    match value {
        Ok(value) => json!({ "ok": value }),
        Err(_) => json!({ "error": "arithmetic_overflow" }),
    }
}

fn arithmetic_boundaries() -> Value {
    let combo = [0, 1, 4, 5, 6, 9, 10, 11, 14, 15, 16, 19, 20, 21, u32::MAX]
        .map(|count| json!({ "input": count, "output": combo_multiplier(count) }));
    let coin_clocks = [0, 88_499, 88_500, 89_999, 90_000, 90_001, u64::MAX].map(
        |current_ms| {
            json!({ "input_ms": current_ms.to_string(), "output_ms": coin_clock(current_ms).to_string() })
        },
    );
    let package_clocks = [0, 86_999, 87_000, 89_999, 90_000, u64::MAX].map(
        |current_ms| {
            json!({ "input_ms": current_ms.to_string(), "output_ms": package_clock(current_ms).to_string() })
        },
    );
    let pickup_clocks = [0, 93_999, 94_000, 98_999, 99_000, u64::MAX].map(
        |current_ms| {
            json!({ "input_ms": current_ms.to_string(), "output_ms": time_pickup_clock(current_ms).to_string() })
        },
    );
    let waves = [0, 35_999, 36_000, 63_999, 64_000, u64::MAX].map(|active_ms| {
        json!({ "input_ms": active_ms.to_string(), "output": completed_waves(active_ms) })
    });
    let credited = [
        (0, 10_000, false),
        (1, 10_000, false),
        (5, 9_000, false),
        (5, 9_000, true),
        (u32::MAX, u32::MAX, false),
        (u32::MAX, u32::MAX, true),
    ]
    .map(|(base, premium_bps, guilt)| {
        json!({
            "input": { "base": base, "premium_bps": premium_bps, "guilt": guilt },
            "output": result_u32(credited_positive(base, premium_bps, guilt))
        })
    });
    let objectives = [Conduct::CluckHunt, Conduct::RightOfWay].into_iter().flat_map(|conduct| {
        (0..6).map(move |round_index| {
            let (objective, target) = objective_for(conduct, round_index);
            json!({
                "input": { "conduct": conduct as u8, "round_index": round_index },
                "output": { "objective": objective_name(objective), "ordinal": objective as u8, "target": target }
            })
        })
    }).collect::<Vec<_>>();

    let health_pickups = [0, 64, 65, 99, 100, u32::MAX]
        .map(|input| json!({ "input": input, "output": health_pickup(input) }));

    json!({
        "combo_multiplier": combo,
        "cluck_awards": [
            { "input": { "chicken_burst": false, "frenzy": false }, "direct": cluck_direct_award(false, false) },
            { "input": { "chicken_burst": true, "frenzy": false }, "direct": cluck_direct_award(true, false) },
            { "input": { "chicken_burst": true, "frenzy": true }, "direct": cluck_direct_award(true, true) },
            { "input": { "multiplier": 5, "glass_cannon": true, "combo_frenzy": true }, "combo_bonus": cluck_combo_bonus(5, true, true) }
        ],
        "cluck_bucket_saturation": {
            "objective_at_max": cluck_objective(u32::MAX),
            "coin_mega_at_max": cluck_coin_award(u32::MAX, true),
            "penalty_zero": cluck_critter_penalty(0),
            "penalty_one": cluck_critter_penalty(1),
            "wave_at_max": cluck_wave_award(u32::MAX, true)
        },
        "cluck_terminal": [
            { "input": { "chickens": 20, "coins": 22 }, "output": result_u32(cluck_terminal(20, 22)) },
            { "input": { "chickens": u32::MAX, "coins": 0 }, "output": result_u32(cluck_terminal(u32::MAX, 0)) },
            { "input": { "chickens": u32::MAX, "coins": 1 }, "output": result_u32(cluck_terminal(u32::MAX, 1)) }
        ],
        "coin_clock": coin_clocks,
        "package_clock": package_clocks,
        "time_pickup_clock": pickup_clocks,
        "health_pickup": health_pickups,
        "completed_waves": waves,
        "population_caps": [
            { "input": { "level": 0, "rush": false, "surge": false }, "traffic_target": traffic_target(0, false, false) },
            { "input": { "level": u32::MAX, "rush": true, "surge": true }, "traffic_target": traffic_target(u32::MAX, true, true) },
            { "input": { "chicken_burst": true, "frenzy": true }, "chicken_target": chicken_target(true, true) },
            { "input": { "stampede": true, "critter_burst": true }, "critter_target": critter_target(true, true) }
        ],
        "courtesy_edges": [
            { "speed": 3.999, "distance": 2.0, "eligible": courtesy_eligible(3.999, 2.0) },
            { "speed": 4.0, "distance": 1.0, "eligible": courtesy_eligible(4.0, 1.0) },
            { "speed": 4.0, "distance": 1.0001, "eligible": courtesy_eligible(4.0, 1.0001) },
            { "speed": 4.0, "distance": 2.12, "eligible": courtesy_eligible(4.0, 2.12) },
            { "speed": 4.0, "distance": 2.1201, "eligible": courtesy_eligible(4.0, 2.1201) }
        ],
        "credited_positive": credited,
        "right_of_way_terminal": [
            { "accumulator": "-1", "output": result_u32(RightOfWay { accumulator: -1, ..RightOfWay::new() }.terminal_total()) },
            { "accumulator": "0", "output": result_u32(RightOfWay::new().terminal_total()) },
            { "accumulator": "4294967295", "output": result_u32(RightOfWay { accumulator: i64::from(u32::MAX), ..RightOfWay::new() }.terminal_total()) },
            { "accumulator": "4294967296", "output": result_u32(RightOfWay { accumulator: i64::from(u32::MAX) + 1, ..RightOfWay::new() }.terminal_total()) }
        ],
        "objectives": objectives,
        "frenzy_orb_lifetime": [
            { "spawn_ms": 100, "now_ms": 99, "alive": frenzy_orb_alive(100, 99) },
            { "spawn_ms": 100, "now_ms": 100, "alive": frenzy_orb_alive(100, 100) },
            { "spawn_ms": 100, "now_ms": 12099, "alive": frenzy_orb_alive(100, 12099) },
            { "spawn_ms": 100, "now_ms": 12100, "alive": frenzy_orb_alive(100, 12100) }
        ]
    })
}

/// Complete cross-language deterministic fixtures, including all event layouts
/// at maximum fixed-width values and arithmetic transition boundaries.
pub fn golden_document() -> Value {
    let seed01 = seed(1);
    json!({
        "fixture_version": 1,
        "rules_id": RULES_VERSION_ID,
        "encoding": { "hex": "lowercase", "base64url": "unpadded", "integers": "big_endian", "u64_fixture_inputs": "canonical_decimal_strings", "line_endings": "LF" },
        "stream_vectors": stream_vectors(),
        "schedule_vectors": (1..=20).map(schedule_vector).collect::<Vec<_>>(),
        "frenzy_seed01": {
            "input": { "seed_hex": hex(&seed01), "through_ms": 60000 },
            "output": frenzy_opportunities(&seed01, 60_000).iter().map(|value| json!({
                "at_ms": value.at_ms, "roll_residue": value.roll_residue,
                "spawn": value.spawn, "pity": value.pity
            })).collect::<Vec<_>>()
        },
        "relocation_seed01": {
            "input": { "seed_hex": hex(&seed01) },
            "output": { "lateral_ahead": frenzy_relocation_candidates(&seed01) }
        },
        "canonical": canonical_vectors(),
        "terminal_reason_vectors": terminal_reason_vectors(),
        "drowned_vectors": drowned_vectors(),
        "arithmetic_boundaries": arithmetic_boundaries()
    })
}

fn pretty(document: &Value) -> String {
    serde_json::to_string_pretty(document).expect("v3 artifact is serializable") + "\n"
}

pub fn manifest_pretty_json() -> String {
    pretty(&manifest_document())
}

pub fn schema_pretty_json() -> String {
    pretty(&schema_document())
}

pub fn golden_pretty_json() -> String {
    pretty(&golden_document())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_maximum_width_event_vector_fits_record_limit() {
        for (name, payload) in all_event_payloads() {
            let record = canonical::event_record(&Event {
                seq: u32::MAX,
                active_ms: u64::MAX,
                payload,
            })
            .unwrap();
            assert!(
                record.len() <= canonical::MAX_EVENT_RECORD_BYTES,
                "{name}: {}",
                record.len()
            );
        }
    }
}
