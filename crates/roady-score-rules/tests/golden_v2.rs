use roady_score_rules::v2::artifacts;
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

fn rules_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../rules")
}

fn checked_in(name: &str) -> Vec<u8> {
    fs::read(rules_dir().join(name)).unwrap()
}

#[test]
fn checked_in_v2_artifacts_are_byte_for_byte_canonical_lf() {
    for (name, generated) in [
        ("roady-rules.v2.json", artifacts::manifest_pretty_json()),
        (
            "roady-rules.v2.schema.json",
            artifacts::schema_pretty_json(),
        ),
        (
            "roady-rules.v2.golden.json",
            artifacts::golden_pretty_json(),
        ),
    ] {
        let bytes = checked_in(name);
        assert_eq!(bytes, generated.as_bytes(), "stale {name}");
        assert!(bytes.ends_with(b"\n"), "{name} needs a trailing LF");
        assert!(
            !bytes.windows(2).any(|window| window == b"\r\n"),
            "{name} contains CRLF"
        );
    }
}

#[test]
fn v2_schema_is_the_exact_immutable_manifest_contract() {
    let manifest: Value = serde_json::from_slice(&checked_in("roady-rules.v2.json")).unwrap();
    let schema: Value = serde_json::from_slice(&checked_in("roady-rules.v2.schema.json")).unwrap();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["const"], manifest);
}

#[test]
fn golden_contains_complete_inputs_outputs_and_arithmetic_boundaries() {
    let golden: Value = serde_json::from_slice(&checked_in("roady-rules.v2.golden.json")).unwrap();
    let schedules = golden["schedule_vectors"].as_array().unwrap();
    assert_eq!(schedules.len(), 20);
    assert!(schedules.iter().all(|vector| {
        vector.get("input").is_some()
            && vector.get("output").is_some()
            && vector["output"]["windows"].as_array().unwrap().len() == 16
    }));

    let events = golden["canonical"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 16);
    assert!(events.iter().all(|vector| {
        vector.get("input").is_some()
            && vector.get("output").is_some()
            && vector["output"]["fits_max_event_record_bytes"] == true
            && vector["output"]["event_record_length"].as_u64().unwrap() <= 192
    }));

    let boundaries = golden["arithmetic_boundaries"].as_object().unwrap();
    for required in [
        "combo_multiplier",
        "cluck_terminal",
        "coin_clock",
        "package_clock",
        "time_pickup_clock",
        "credited_positive",
        "right_of_way_terminal",
        "courtesy_edges",
        "frenzy_orb_lifetime",
    ] {
        assert!(boundaries.contains_key(required), "missing {required}");
    }
}

#[test]
fn v1_generator_and_files_keep_frozen_sha256() {
    assert_eq!(
        roady_score_rules::v2::canonical::sha256(&checked_in("roady-rules.v1.json")),
        decode_hex("8a2ca8ce1676622922fbf1d1280e6fcc00840cbd09d7db4b25b25cfea20e8c04")
    );
    assert_eq!(
        roady_score_rules::v2::canonical::sha256(&checked_in("roady-rules.v1.schema.json")),
        decode_hex("aa4f8d174f44982ac7c55266d31edf839fbf0d9995f64a2ee045eeaddc56a39f")
    );
    let generator =
        fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/generate.rs")).unwrap();
    let generator_lf = String::from_utf8(generator)
        .unwrap()
        .replace("\r\n", "\n")
        .into_bytes();
    assert_eq!(
        roady_score_rules::v2::canonical::sha256(&generator_lf),
        decode_hex("e95b2b4289c193efa7bd0e99dd19a68deeac81e549bbc5f202db097005e7a266")
    );
}

#[test]
fn running_v2_generator_twice_is_byte_stable() {
    let before = v2_artifacts();
    run_generator();
    let once = v2_artifacts();
    run_generator();
    let twice = v2_artifacts();
    assert_eq!(before, once);
    assert_eq!(once, twice);
}

fn v2_artifacts() -> Vec<Vec<u8>> {
    [
        "roady-rules.v2.json",
        "roady-rules.v2.schema.json",
        "roady-rules.v2.golden.json",
    ]
    .map(checked_in)
    .into()
}

fn run_generator() {
    let status = Command::new(env!("CARGO"))
        .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .args([
            "run",
            "--quiet",
            "-p",
            "roady-score-rules",
            "--example",
            "generate_v2",
        ])
        .status()
        .unwrap();
    assert!(status.success());
}

fn decode_hex(value: &str) -> [u8; 32] {
    let mut bytes = [0; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        bytes[index] = u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap();
    }
    bytes
}
