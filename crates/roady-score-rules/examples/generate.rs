fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rules");
    std::fs::write(
        root.join("roady-rules.v1.json"),
        roady_score_rules::manifest_pretty_json(),
    )
    .unwrap();
    std::fs::write(
        root.join("roady-rules.v1.schema.json"),
        roady_score_rules::schema_pretty_json(),
    )
    .unwrap();
}
