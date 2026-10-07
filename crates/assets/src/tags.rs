//! The asset tags in reference/app/views/layouts/application.html.erb:
//!
//!   <%= stylesheet_link_tag :all, "data-turbo-track": "reload" %>
//!   <%= javascript_importmap_tags %>

use crate::embedded;
use crate::helpers::stylesheet_path;
use ruby_compat::erb::html_escape;
use std::sync::LazyLock;

/// Mutually exclusive page styles while the frontend migrates to Basecoat.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum StyleProfile {
    #[default]
    Legacy,
    Basecoat,
}

/// What `stylesheet_link_tag` renders, plus the preload links Rails adds to the response's
/// `link` header while rendering it (`config.action_view.preload_links_header`, on by default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StylesheetTags {
    pub html: String,
    pub preload_links: Vec<String>,
}

/// Rails' MAX_HEADER_SIZE for the preload `link` header (ActionView::Helpers::AssetTagHelper).
const MAX_LINK_HEADER_SIZE: usize = 1_000;

/// Propshaft::Helper#all_stylesheets_paths: every CSS logical path on the load path, sorted.
pub fn all_stylesheet_paths() -> &'static [&'static str] {
    embedded::STYLESHEETS
}

/// A page profile's styles, shared by HTML tags and HTTP preload links.
pub fn stylesheet_paths_for(profile: StyleProfile) -> &'static [&'static str] {
    static LEGACY: LazyLock<Vec<&'static str>> =
        LazyLock::new(|| all_stylesheet_paths().iter().copied().filter(|path| !path.starts_with("basecoat/")).collect());
    match profile {
        StyleProfile::Legacy => &LEGACY,
        StyleProfile::Basecoat => &["basecoat/app.css"],
    }
}

pub fn stylesheet_link_tag_for(profile: StyleProfile, options: &[(&str, &str)]) -> StylesheetTags {
    stylesheet_link_tag(stylesheet_paths_for(profile), options)
}

/// The original legacy `stylesheet_link_tag :all, **options` set, excluding Basecoat styles.
pub fn stylesheet_link_tag_all(options: &[(&str, &str)]) -> StylesheetTags {
    stylesheet_link_tag_for(StyleProfile::Legacy, options)
}

/// `stylesheet_link_tag *sources, **options` as Propshaft::Helper renders it: one Rails
/// `stylesheet_link_tag` per source, joined by newlines. Missing sources panic, like Rails raises.
pub fn stylesheet_link_tag(sources: &[&str], options: &[(&str, &str)]) -> StylesheetTags {
    let mut html = Vec::with_capacity(sources.len());
    let mut preload_links = Vec::with_capacity(sources.len());

    for source in sources {
        let href = stylesheet_path(source);
        if !href.is_empty() && !href.starts_with("data:") {
            preload_links.push(format!("<{href}>; rel=preload; as=style; nopush"));
        }

        let mut tag = format!("<link rel=\"stylesheet\" href=\"{}\"", html_escape(&href));
        for (name, value) in options {
            tag.push_str(&format!(" {name}=\"{}\"", html_escape(value)));
        }
        tag.push_str(" />");
        html.push(tag);
    }

    StylesheetTags { html: html.join("\n"), preload_links }
}

/// Appends preload links to a response's existing `link` header value the way
/// `send_preload_links_header` does: a link that would push the header past 1,000 bytes is
/// left out (Propshaft renders each stylesheet separately, so later, shorter ones can still fit).
pub fn append_preload_links(header: &str, preload_links: &[String]) -> String {
    let mut header = header.to_string();
    for link in preload_links {
        if header.len() + link.len() > MAX_LINK_HEADER_SIZE {
            continue;
        }
        if !header.is_empty() {
            header.push(',');
        }
        header.push_str(link);
    }
    header
}

/// `javascript_importmap_tags`: the import map, a modulepreload link per pin, and the
/// `import "application"` module script. Computed at build time from config/importmap.rb.
pub fn javascript_importmap_tags() -> &'static str {
    embedded::IMPORTMAP_TAGS
}
