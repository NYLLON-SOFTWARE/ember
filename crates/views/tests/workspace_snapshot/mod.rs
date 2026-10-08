//! Matchbox owns the redesigned page DOM; Rails remains the golden for message/protocol fragments.
pub fn assert_snapshot(group: &str, name: &str, actual: &[String]) {
    // Some cases also exercise the wrapper with the same expectation in parallel.
    static UPDATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = UPDATE_LOCK.lock().unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/matchbox").join(group).join(format!("{name}.json"));
    if std::env::var("MATCHBOX_UPDATE_VIEWS").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(actual).unwrap())).unwrap();
    }
    let expected: Vec<String> = serde_json::from_str(
        &std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}; see tests/golden/matchbox/README.md", path.display())),
    )
    .unwrap();
    if actual != expected {
        let index = expected.iter().zip(actual).position(|(a, b)| a != b).unwrap_or(expected.len().min(actual.len()));
        panic!(
            "{group}/{name}: Matchbox DOM changed at token {index}: expected {:?}, got {:?}; review before updating the snapshot",
            expected.get(index),
            actual.get(index)
        );
    }
}
