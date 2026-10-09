//! Ember environment names, with the original names accepted for existing installations.

use std::env::VarError;
use std::ffi::OsString;

/// Prefer `EMBER_`, then `MATCHBOX_`, then `CAMPFIRE_`, including when called with a legacy
/// name. Fall back only when a name is absent: an empty value is explicit.
pub fn lookup<T>(name: &str, get: impl Fn(&str) -> Option<T>) -> Option<T> {
    let prefixes = ["EMBER_", "MATCHBOX_", "CAMPFIRE_"];
    match prefixes.iter().find_map(|prefix| name.strip_prefix(*prefix)) {
        Some(suffix) => prefixes.iter().find_map(|prefix| get(&format!("{prefix}{suffix}"))),
        None => get(name),
    }
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
    fn current_settings_win_over_each_legacy_name() {
        let values = [("CAMPFIRE_STORAGE_PATH", "campfire"), ("MATCHBOX_STORAGE_PATH", "matchbox"), ("EMBER_STORAGE_PATH", "ember")];
        for requested in ["EMBER_STORAGE_PATH", "MATCHBOX_STORAGE_PATH", "CAMPFIRE_STORAGE_PATH"] {
            let get = |name: &str| values.iter().find(|(key, _)| *key == name).map(|(_, value)| *value);
            assert_eq!(lookup(requested, get), Some("ember"));
        }
    }

    #[test]
    fn falls_back_through_both_legacy_names() {
        let values = [("CAMPFIRE_STORAGE_PATH", "campfire"), ("MATCHBOX_STORAGE_PATH", "matchbox")];
        let get = |name: &str| values.iter().find(|(key, _)| *key == name).map(|(_, value)| *value);
        assert_eq!(lookup("EMBER_STORAGE_PATH", get), Some("matchbox"));
        assert_eq!(lookup("EMBER_STORAGE_PATH", |name| (name == "CAMPFIRE_STORAGE_PATH").then_some("campfire")), Some("campfire"));
        assert_eq!(lookup::<&str>("EMBER_STORAGE_PATH", |_| None), None);
    }

    #[test]
    fn empty_settings_do_not_fall_back() {
        for empty_name in ["EMBER_STORAGE_PATH", "MATCHBOX_STORAGE_PATH"] {
            assert_eq!(
                lookup("EMBER_STORAGE_PATH", |name| match name {
                    name if name == empty_name => Some(""),
                    "CAMPFIRE_STORAGE_PATH" => Some("campfire"),
                    _ => None,
                }),
                Some("")
            );
        }
    }

    #[test]
    fn unrelated_variables_keep_their_exact_names() {
        assert_eq!(lookup("SECRET_KEY_BASE", |name| (name == "SECRET_KEY_BASE").then_some("secret")), Some("secret"));
        assert_eq!(lookup("SECRET_KEY_BASE", |name| (name == "EMBER_SECRET_KEY_BASE").then_some("secret")), None);
    }
}
