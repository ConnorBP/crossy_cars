fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rules");
    let artifacts = [
        (
            "roady-rules.v3.json",
            roady_score_rules::v3::artifacts::manifest_pretty_json(),
        ),
        (
            "roady-rules.v3.schema.json",
            roady_score_rules::v3::artifacts::schema_pretty_json(),
        ),
        (
            "roady-rules.v3.golden.json",
            roady_score_rules::v3::artifacts::golden_pretty_json(),
        ),
    ];
    for (name, contents) in artifacts {
        assert!(!contents.as_bytes().windows(2).any(|bytes| bytes == b"\r\n"));
        std::fs::write(root.join(name), contents).unwrap();
    }
}
