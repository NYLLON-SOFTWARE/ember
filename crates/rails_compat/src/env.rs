//! Matchbox environment names, with the original names accepted for existing installations.

use std::env::VarError;
use std::ffi::OsString;

/// Prefer the current name; fall back only when it is absent (an empty value is explicit).
pub fn lookup<T>(name: &str, get: impl Fn(&str) -> Option<T>) -> Option<T> {
    get(name).or_else(|| name.strip_prefix("MATCHBOX_").and_then(|suffix| get(&format!("CAMPFIRE_{suffix}"))))
}

/// Like `std::env::var_os`, accepting legacy installation settings.
pub fn var_os(name: &str) -> Option<OsString> {
    lookup(name, |key| std::env::var_os(key))
}

/// Like `std::env::var`, accepting legacy installation settings.
pub fn var(name: &str) -> Result<String, VarError> {
    var_os(name).ok_or(VarError::NotPresent)?.into_string().map_err(VarError::NotUnicode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_settings_work_and_explicit_new_settings_win() {
        let values = [("CAMPFIRE_STORAGE_PATH", "old"), ("MATCHBOX_STORAGE_PATH", "new")];
        let get = |name: &str| values.iter().find(|(key, _)| *key == name).map(|(_, value)| *value);
        assert_eq!(lookup("MATCHBOX_STORAGE_PATH", get), Some("new"));
        assert_eq!(lookup("MATCHBOX_STORAGE_PATH", |name| (name == "CAMPFIRE_STORAGE_PATH").then_some("old")), Some("old"));
        assert_eq!(lookup("MATCHBOX_STORAGE_PATH", |name| (name == "MATCHBOX_STORAGE_PATH").then_some("")), Some(""));
        assert_eq!(lookup("SECRET_KEY_BASE", get), None);
    }
}
