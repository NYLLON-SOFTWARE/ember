//! The reference's asset pipeline, precomputed at build time and embedded in the binary.
//!
//! `build.rs` digests and compiles `reference/app/assets`, `reference/app/javascript`,
//! `reference/vendor/javascript` and the gem assets in `vendor/` exactly as Propshaft's
//! `assets:precompile` does, renders the import map from `reference/config/importmap.rb`, and
//! embeds everything along with `reference/public`.
//!
//! - [`asset_path`] and friends: ActionView's asset URL helpers over the Propshaft manifest.
//! - [`stylesheet_link_tag`] / [`stylesheet_link_tag_all`] and [`javascript_importmap_tags`]:
//!   the layout's head tags, plus the `link` preload header Rails sends alongside them.
//! - [`serve`]: ActionDispatch::Static over the embedded public/ directory.

mod helpers;
mod lucide;
mod serve;
mod tags;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}

pub use helpers::{
    MissingAssetError, asset_path, asset_url, audio_path, digested_path, image_path, image_url, javascript_path, stylesheet_path,
    try_asset_path,
};
pub use lucide::{LucideIcon, lucide_icon, lucide_icons};
pub use serve::{Body, StaticRequest, StaticResponse, serve};
pub use tags::{
    StyleProfile, StylesheetTags, all_stylesheet_paths, append_preload_links, javascript_importmap_tags, stylesheet_link_tag,
    stylesheet_link_tag_all, stylesheet_link_tag_for, stylesheet_paths_for,
};

/// The URL prefix digested assets are served under (`config.assets.prefix`).
pub const PREFIX: &str = "/assets";

/// Propshaft's manifest, `{"logical": {"digested_path": ..., "integrity": null}}`, as served
/// from `/assets/.manifest.json`.
pub fn manifest_json() -> &'static str {
    embedded::MANIFEST_JSON
}

/// Every `(logical path, digested path)` pair, sorted by logical path.
pub fn manifest() -> &'static [(&'static str, &'static str)] {
    embedded::MANIFEST
}
