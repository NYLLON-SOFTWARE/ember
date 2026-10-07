//! ActionView::Helpers::AssetUrlHelper over Propshaft's static resolver (no asset host, no
//! relative_url_root, as the reference is configured).

use crate::{PREFIX, embedded};
use std::fmt;

/// Propshaft::MissingAssetError, raised by `compute_asset_path` for an unknown logical path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingAssetError(pub String);

impl fmt::Display for MissingAssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "The asset '{}' was not found in the load path.", self.0)
    }
}

impl std::error::Error for MissingAssetError {}

/// The digested path (relative to `/assets/`) for a logical path, from the manifest.
pub fn digested_path(logical_path: &str) -> Option<&'static str> {
    embedded::MANIFEST.binary_search_by(|(logical, _)| (*logical).cmp(logical_path)).ok().map(|index| embedded::MANIFEST[index].1)
}

/// `asset_path(source)`: "/assets/<digested>" for pipeline assets; URLs and absolute paths pass
/// through; a `?query` or `#fragment` tail is kept.
pub fn try_asset_path(source: &str) -> Result<String, MissingAssetError> {
    compute(source, None)
}

/// Like [`try_asset_path`], but panics for a missing asset the way Rails raises
/// Propshaft::MissingAssetError while rendering.
pub fn asset_path(source: &str) -> String {
    try_asset_path(source).unwrap_or_else(|e| panic!("{e}"))
}

pub fn image_path(source: &str) -> String {
    asset_path(source)
}

pub fn audio_path(source: &str) -> String {
    asset_path(source)
}

/// `javascript_path`: appends ".js" unless the source already ends in it.
pub fn javascript_path(source: &str) -> String {
    compute(source, Some(".js")).unwrap_or_else(|e| panic!("{e}"))
}

/// `stylesheet_path`: appends ".css" unless the source already ends in it.
pub fn stylesheet_path(source: &str) -> String {
    compute(source, Some(".css")).unwrap_or_else(|e| panic!("{e}"))
}

/// `asset_url(source)`: the path joined onto the request's base URL (e.g. "https://host:3000"),
/// which is what Rails uses as the host when no asset_host is configured.
pub fn asset_url(base_url: &str, source: &str) -> String {
    let path = asset_path(source);
    if path.is_empty() || is_uri(&path) { path } else { file_join(base_url, &path) }
}

pub fn image_url(base_url: &str, source: &str) -> String {
    asset_url(base_url, source)
}

fn compute(source: &str, extname: Option<&str>) -> Result<String, MissingAssetError> {
    if source.is_empty() {
        return Ok(String::new());
    }
    if is_uri(source) {
        return Ok(source.to_string());
    }

    // tail, source = source[/([?#].+)$/], source.sub(/([?#].+)$/, "")
    let (source, tail) = match source.find(['?', '#']).filter(|&i| i + 1 < source.len()) {
        Some(i) => (&source[..i], &source[i..]),
        None => (source, ""),
    };

    let mut source = source.to_string();
    if let Some(extname) = extname
        && file_extname(&source) != extname
    {
        source.push_str(extname);
    }

    if !source.starts_with('/') {
        source = match digested_path(&source) {
            Some(digested) => format!("{PREFIX}/{digested}"),
            None => return Err(MissingAssetError(source)),
        };
    }

    Ok(format!("{source}{tail}"))
}

/// ActionView::Helpers::AssetUrlHelper::URI_REGEXP: %r{^[-a-z]+://|^(?:cid|data):|^//}i
fn is_uri(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    if lower.starts_with("//") || lower.starts_with("cid:") || lower.starts_with("data:") {
        return true;
    }
    match lower.find("://") {
        Some(i) if i > 0 => lower[..i].chars().all(|c| c == '-' || c.is_ascii_lowercase()),
        _ => false,
    }
}

fn file_extname(path: &str) -> &str {
    let base = path.rsplit('/').next().unwrap_or(path);
    let trimmed = base.trim_start_matches('.');
    trimmed.rfind('.').map_or("", |dot| &trimmed[dot..])
}

/// File.join(host, path)
fn file_join(host: &str, path: &str) -> String {
    format!("{}/{}", host.trim_end_matches('/'), path.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_logical_paths() {
        assert_eq!(asset_path("matchbox-icon.png"), "/assets/matchbox-icon-3d9986c5.png");
        assert_eq!(image_path("bot.svg"), "/assets/bot-8a69692e.svg");
        assert_eq!(audio_path("56k.mp3"), "/assets/56k-67359aa6.mp3");
        assert_eq!(
            asset_path("screenshots/android-chat.png"),
            format!("/assets/{}", digested_path("screenshots/android-chat.png").unwrap())
        );
    }

    #[test]
    fn keeps_tails_and_passes_through_urls_and_absolute_paths() {
        assert_eq!(asset_path("bot.svg?v=1#x"), "/assets/bot-8a69692e.svg?v=1#x");
        assert_eq!(asset_path("https://example.com/a.png"), "https://example.com/a.png");
        assert_eq!(asset_path("//cdn.example.com/a.png"), "//cdn.example.com/a.png");
        assert_eq!(asset_path("data:image/png;base64,xx"), "data:image/png;base64,xx");
        assert_eq!(asset_path("/rooms/1"), "/rooms/1");
        assert_eq!(asset_path(""), "");
    }

    #[test]
    fn appends_type_extensions() {
        assert_eq!(stylesheet_path("base"), stylesheet_path("base.css"));
        assert!(javascript_path("application").starts_with("/assets/application-"));
    }

    #[test]
    fn urls_join_the_base_url() {
        assert_eq!(image_url("https://chat.example.com", "add.svg"), format!("https://chat.example.com{}", image_path("add.svg")));
    }

    #[test]
    fn missing_assets_are_errors() {
        assert_eq!(try_asset_path("nope.png").unwrap_err().to_string(), "The asset 'nope.png' was not found in the load path.");
    }
}
