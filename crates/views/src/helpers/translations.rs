//! `TranslationsHelper`: the language popups beside form fields.

use super::assets::image_tag;
use super::html::{Html, Safe};
use super::tag::{Attrs, attrs, content_tag, content_tag_text};
use super::translations_table::TRANSLATIONS;
use crate::ViewContext;

/// `translations_for(key)`.
pub fn translations_for(key: &str) -> Html {
    let entries = TRANSLATIONS.iter().find(|(name, _)| *name == key).unwrap_or_else(|| panic!("unknown translation key {key}")).1;
    let items: String = entries
        .iter()
        .map(|(language, translation)| {
            format!(
                "{}{}",
                content_tag_text("dt", attrs(), language).0,
                content_tag_text("dd", attrs().class("margin-none"), translation).0
            )
        })
        .collect();
    content_tag("dl", attrs().class("language-list"), &items)
}

/// `translation_button(key)`.
pub fn translation_button(ctx: &ViewContext, key: &str) -> Html {
    translation_button_with_summary(ctx, key, attrs().class("btn").tabindex(-1))
}

/// A presentation variant retaining the existing popup actions and translated content.
pub fn translation_button_with_summary(ctx: &ViewContext, key: &str, summary_attrs: Attrs) -> Html {
    if ctx.account.hide_translation_buttons {
        return Safe(String::new());
    }
    let summary = content_tag(
        "summary",
        summary_attrs,
        &format!(
            "{}{}",
            image_tag(ctx, "globe.svg", attrs().size(20).aria_hidden().class("color-icon")).0,
            content_tag_text("span", attrs().class("for-screen-reader"), "Translate").0
        ),
    );
    let menu = content_tag("div", attrs().class("language-list-menu shadow").data("popup_target", "menu"), &translations_for(key).0);
    let details = attrs()
        .class("position-relative")
        .data("controller", "popup")
        .data("action", "keydown.esc->popup#close toggle->popup#toggle click@document->popup#closeOnClickOutside")
        .data("popup_orientation_top_class", "popup-orientation-top");
    Safe(content_tag("details", &details, &format!("{}{}", summary.0, menu.0)).0)
}
