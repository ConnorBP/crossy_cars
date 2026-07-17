use roady_score_rules::v3::{self, artifacts};
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

fn rules_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../rules")
}
fn checked_in(name: &str) -> Vec<u8> {
    fs::read(rules_dir().join(name)).unwrap()
}

fn repository_file(name: &str) -> Vec<u8> {
    fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(name),
    )
    .unwrap()
}

#[test]
fn checked_in_v3_artifacts_are_exact_generated_lf() {
    for (name, generated) in [
        ("roady-rules.v3.json", artifacts::manifest_pretty_json()),
        (
            "roady-rules.v3.schema.json",
            artifacts::schema_pretty_json(),
        ),
        (
            "roady-rules.v3.golden.json",
            artifacts::golden_pretty_json(),
        ),
    ] {
        let bytes = checked_in(name);
        assert_eq!(bytes, generated.as_bytes(), "stale {name}");
        assert!(bytes.ends_with(b"\n"));
        assert!(!bytes.windows(2).any(|window| window == b"\r\n"));
    }
}

#[test]
fn strict_schema_and_complete_vectors_are_published() {
    let manifest: Value = serde_json::from_slice(&checked_in("roady-rules.v3.json")).unwrap();
    let schema: Value = serde_json::from_slice(&checked_in("roady-rules.v3.schema.json")).unwrap();
    let golden: Value = serde_json::from_slice(&checked_in("roady-rules.v3.golden.json")).unwrap();
    assert_eq!(schema["const"], manifest);
    assert_eq!(golden["schedule_vectors"].as_array().unwrap().len(), 20);
    assert_eq!(golden["canonical"]["events"].as_array().unwrap().len(), 16);
    assert_eq!(
        golden["terminal_reason_vectors"].as_array().unwrap().len(),
        6
    );
    assert_eq!(golden["drowned_vectors"].as_array().unwrap().len(), 2);
    assert!(
        golden["canonical"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(
                |event| event["output"]["event_record_length"].as_u64().unwrap()
                    <= v3::canonical::MAX_EVENT_RECORD_BYTES as u64
            )
    );
}

#[test]
fn frozen_v1_v2_artifact_hashes_have_not_changed() {
    for (name, expected) in [
        (
            "roady-rules.v1.json",
            "8a2ca8ce1676622922fbf1d1280e6fcc00840cbd09d7db4b25b25cfea20e8c04",
        ),
        (
            "roady-rules.v1.schema.json",
            "aa4f8d174f44982ac7c55266d31edf839fbf0d9995f64a2ee045eeaddc56a39f",
        ),
        (
            "roady-rules.v2.json",
            "13b1380857bfc8e6fa1f8177a15a29c3dba89d25fe45ec6c7506b9c12584dd10",
        ),
        (
            "roady-rules.v2.schema.json",
            "b62ae10eccc5cb45fe85f8cd2b4533a73a0998915569962d69c5e17dfc6788c1",
        ),
        (
            "roady-rules.v2.golden.json",
            "e672c6bd6d32f205da3b8d4fdd6c65723e9de0ad4f8a0013efd201f8a24f7e8a",
        ),
    ] {
        assert_eq!(
            hex(&v3::canonical::sha256(&checked_in(name))),
            expected,
            "frozen {name}"
        );
    }

    // Git stores these text fixtures with LF. Normalize only checkout CRLF so
    // this assertion is stable on Windows while still pinning every byte.
    for (name, expected) in [
        (
            "crates/roady-score-rules/src/v2.rs",
            "974beb97f6220839b17e75b992722de1a97cf3f55f6eee6f36ffafa1e144b1dc",
        ),
        (
            "crates/roady-score-rules/src/v2/artifacts.rs",
            "d7e943a8b2ac56c8fe182cd720752eaceee9a1759fcf5da3b772210218ba6721",
        ),
        (
            "crates/roady-score-rules/src/v2/canonical.rs",
            "2686113711148aea4c4c10b1bb9e1366676009bad4dc9f43120354e74b642a97",
        ),
        (
            "crates/roady-score-rules/tests/golden.rs",
            "eaf1b7fc9be5d9feda958ac52a7ed4db3a1b2c152f3332978d5f0d4ac580e217",
        ),
        (
            "crates/roady-score-rules/tests/golden_v2.rs",
            "fa0f4a95264894fbaba9aa23c263f81d4d9ef6aa3feff669b8ba56bfee9d40b8",
        ),
        (
            "leaderboard/src/rules-v2.ts",
            "38de29f0e737e6dc2f45b77e6baed636f6b9c064f7404a848b312d981dc1772e",
        ),
        (
            "leaderboard/test/rules-v2.test.ts",
            "e775d15a70479ae764937e9915a810602f1e47cce36776d69e832691befaa643",
        ),
    ] {
        let normalized = String::from_utf8(repository_file(name))
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(
            hex(&v3::canonical::sha256(normalized.as_bytes())),
            expected,
            "frozen {name}"
        );
    }
}

#[test]
fn generator_is_byte_stable() {
    let before = artifacts_bytes();
    for _ in 0..2 {
        let status = Command::new(env!("CARGO"))
            .current_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .args([
                "run",
                "--quiet",
                "-p",
                "roady-score-rules",
                "--example",
                "generate_v3",
            ])
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(artifacts_bytes(), before);
    }
}

fn artifacts_bytes() -> Vec<Vec<u8>> {
    [
        "roady-rules.v3.json",
        "roady-rules.v3.schema.json",
        "roady-rules.v3.golden.json",
    ]
    .map(checked_in)
    .into()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
