fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rules");
    let artifacts = [
        (
            "roady-rules.v2.json",
            roady_score_rules::v2::artifacts::manifest_pretty_json(),
        ),
        (
            "roady-rules.v2.schema.json",
            roady_score_rules::v2::artifacts::schema_pretty_json(),
        ),
        (
            "roady-rules.v2.golden.json",
            roady_score_rules::v2::artifacts::golden_pretty_json(),
        ),
    ];
    for (name, contents) in artifacts {
        assert!(
            !contents.as_bytes().windows(2).any(|bytes| bytes == b"\r\n"),
            "generated v2 artifact must use LF"
        );
        std::fs::write(root.join(name), contents).unwrap();
    }
}
