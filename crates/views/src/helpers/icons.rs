//! Icons come from the pinned, generated Lucide catalog, never user-supplied markup.

use std::fmt::Write;

use super::{Html, Safe, escape};

pub fn lucide_icon(name: &str) -> Safe<&'static str> {
    Safe(matchbox_assets::lucide_icon(name).expect("template icon must exist in the pinned Lucide catalog").svg)
}

pub fn channel_icon(name: Option<&str>) -> Safe<&'static str> {
    Safe(name.and_then(matchbox_assets::lucide_icon).map_or("#", |icon| icon.svg))
}

pub fn channel_icon_name(name: Option<&str>) -> &'static str {
    name.and_then(matchbox_assets::lucide_icon).map_or("", |icon| icon.name)
}

pub fn channel_icon_label(name: Option<&str>) -> &'static str {
    name.and_then(matchbox_assets::lucide_icon).map_or("Default hashtag", |icon| icon.label)
}

/// Preserve the native select as a complete, no-JavaScript fallback on settings pages.
pub fn channel_icon_options(selected: Option<&str>) -> Html {
    let selected = channel_icon_name(selected);
    let mut options = String::new();
    let chosen = if selected.is_empty() { " selected" } else { "" };
    write!(options, r#"<option value=""{chosen}>Default hashtag</option>"#).unwrap();
    for icon in matchbox_assets::lucide_icons() {
        let chosen = if selected == icon.name { " selected" } else { "" };
        write!(options, r#"<option value="{}"{chosen}>{}</option>"#, escape(icon.name), escape(icon.label)).unwrap();
    }
    Safe(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_catalog_icons_can_become_markup() {
        assert!(channel_icon(Some("coffee")).0.starts_with("<svg"));
        for name in [None, Some("<script>alert(1)</script>"), Some("../coffee"), Some("missing-icon")] {
            assert_eq!(channel_icon(name).0, "#");
            assert_eq!(channel_icon_name(name), "");
        }
    }

    #[test]
    fn native_select_uses_the_same_catalog_and_marks_one_selection() {
        let options = channel_icon_options(Some("coffee")).0;
        assert!(options.contains(r#"<option value="coffee" selected>Coffee</option>"#));
        assert_eq!(options.matches(" selected").count(), 1);
        assert_eq!(options.matches("<option ").count(), matchbox_assets::lucide_icons().len() + 1);
        let default = channel_icon_options(Some("unknown")).0;
        assert!(default.starts_with(r#"<option value="" selected>Default hashtag</option>"#));
    }
}
