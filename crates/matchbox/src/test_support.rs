//! What the seed-backed tests share: they boot the whole app over a copy of a reference-built
//! parity seed (`parity/bin/seed build <name>`, which needs Docker), and pass without running when
//! it hasn't been built.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// `parity/.seed/<name>`, or `None` when it hasn't been built. The first test to find a seed
/// missing says so on stderr, and with `MATCHBOX_REQUIRE_SEED` set a missing seed fails instead.
pub fn seed_dir(name: &str) -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../parity/.seed").join(name);
    if dir.join("db/production.sqlite3").exists() {
        return Some(dir);
    }
    assert!(rails_compat::env::var_os("MATCHBOX_REQUIRE_SEED").is_none(), "parity/.seed/{name} isn't built (parity/bin/seed build {name})");
    note_missing(name);
    None
}

/// Once per seed, and straight to stderr: libtest captures `eprintln!` from a passing test.
fn note_missing(name: &str) {
    static NOTED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let mut noted = NOTED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !noted.iter().any(|noted| noted == name) {
        noted.push(name.to_string());
        let _ = writeln!(
            std::io::stderr(),
            "note: parity/.seed/{name} isn't built, so the tests that boot it pass without running \
             (parity/bin/seed build {name}; MATCHBOX_REQUIRE_SEED=1 fails them instead)"
        );
    }
}
