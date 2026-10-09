//! `ApplicationHelper`, `CableHelper`, `VersionHelper`, `TimeHelper`, `ClipboardHelper`,
//! `DropTargetHelper` and `QrCodeHelper` (`reference/app/helpers/*.rb`).

use rails_compat::encoding;

use super::assets::image_tag;
use super::html::{Html, Safe};
use super::links::link_to;
use super::tag::{attrs, builder_tag, content_tag, content_tag_text, legacy_tag};
use crate::ViewContext;

/// `page_title_tag`: `@page_title || "Ember"`.
pub fn page_title_tag(page_title: Option<&str>) -> Html {
    content_tag_text("title", attrs(), page_title.unwrap_or("Ember"))
}

/// `current_user_meta_tags`.
pub fn current_user_meta_tags(ctx: &ViewContext) -> Html {
    match &ctx.current_user {
        Some(user) => Safe(format!(
            "{}{}",
            legacy_tag("meta", attrs().name("current-user-id").attr("content", user.id)).0,
            legacy_tag("meta", attrs().name("current-user-name").attr("content", user.name.as_str())).0
        )),
        None => Safe(String::new()),
    }
}

/// `script_aware_action_cable_meta_tag`.
pub fn script_aware_action_cable_meta_tag(ctx: &ViewContext) -> Html {
    builder_tag("meta", attrs().name("action-cable-url").attr("content", ctx.cable_url.as_str()))
}

/// `custom_styles_tag`: the account's CSS, unescaped.
pub fn custom_styles_tag(ctx: &ViewContext) -> Html {
    match &ctx.custom_styles {
        Some(styles) => content_tag("style", attrs().data("turbo_track", "reload"), styles),
        None => Safe(String::new()),
    }
}

/// `body_classes`: `[ @body_class, admin_body_class, account_logo_body_class ].compact.join(" ")`.
pub fn body_classes(ctx: &ViewContext, body_class: Option<&str>) -> String {
    let admin = ctx.can_administer().then_some("admin");
    let logo = ctx.account.has_logo.then_some("account-has-logo");
    [body_class, admin, logo].into_iter().flatten().collect::<Vec<_>>().join(" ")
}

/// `link_back`: to the referrer, unless it's missing or the current page.
pub fn link_back(ctx: &ViewContext) -> Html {
    let back_url = match &ctx.referrer {
        Some(referrer) if *referrer != ctx.request_url => referrer.clone(),
        _ => matchbox_routes::root(),
    };
    link_back_to(ctx, &back_url)
}

/// `link_back_to(destination)`.
pub fn link_back_to(ctx: &ViewContext, destination: impl std::fmt::Display) -> Html {
    let content = format!(
        "{}{}",
        image_tag(ctx, "arrow-left.svg", attrs().aria_hidden().size(20)).0,
        content_tag_text("span", attrs().class("for-screen-reader"), "Go Back").0
    );
    link_to(&destination.to_string(), attrs().class("btn"), &content)
}

/// `RoomsHelper#link_back_to_last_room_visited`.
pub fn link_back_to_last_room_visited(ctx: &ViewContext) -> Html {
    match ctx.last_room_visited_id {
        Some(room_id) => link_back_to(ctx, matchbox_routes::room(room_id)),
        None => link_back_to(ctx, matchbox_routes::root()),
    }
}

/// `version_badge`.
pub fn version_badge(ctx: &ViewContext) -> Html {
    content_tag_text("span", attrs().class("version-badge"), &ctx.app_version)
}

/// `button_to_copy_to_clipboard(url) { content }`.
pub fn button_to_copy_to_clipboard(url: &str, content: &str) -> Html {
    let options = attrs()
        .class("btn")
        .data("controller", "copy-to-clipboard")
        .data("action", "copy-to-clipboard#copy")
        .data("copy_to_clipboard_success_class", "btn--success")
        .data("copy_to_clipboard_content_value", url);
    content_tag("button", &options, content)
}

/// `link_to_zoom_qr_code(url) { content }`: the QR code route takes the URL,
/// `Base64.urlsafe_encode64`d (reference/app/helpers/qr_code_helper.rb).
pub fn link_to_zoom_qr_code(url: &str, content: &str) -> Html {
    let path = matchbox_routes::qr_code(encoding::urlsafe_encode_padded(url.as_bytes()));
    let options =
        attrs().class("btn").data("lightbox_target", "image").data("action", "lightbox#open").data("lightbox_url_value", path.as_str());
    link_to(&path, options, content)
}

/// `web_share_session_button(url, title, text) { content }` (`Users::ProfilesHelper`).
pub fn web_share_session_button(url: &str, title: &str, text: &str, content: &str) -> Html {
    let options = attrs()
        .class("btn")
        .hidden()
        .data("controller", "web-share")
        .data("action", "web-share#share")
        .data("web_share_url_value", url)
        .data("web_share_text_value", text)
        .data("web_share_title_value", title);
    content_tag("button", &options, content)
}

/// `truncate(text, length:, omission:)` with Rails' default of no separator: the result,
/// omission included, is at most `length` characters. Returns plain text (escape on output).
pub fn truncate(text: &str, length: usize, omission: &str) -> String {
    if text.chars().count() <= length {
        return text.to_string();
    }
    let keep = length.saturating_sub(omission.chars().count());
    let mut out: String = text.chars().take(keep).collect();
    out.push_str(omission);
    out
}

/// `String#capitalize`: first character upcased, the rest downcased.
pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect(),
        None => String::new(),
    }
}

/// `Array#to_sentence` with the default English connectors, or a custom `two_words_connector`.
pub fn to_sentence(items: &[String], two_words_connector: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [one, two] => format!("{one}{two_words_connector}{two}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_like_rails() {
        assert_eq!(truncate("abcdef", 4, "…"), "abc…");
        assert_eq!(truncate("abcd", 4, "…"), "abcd");
    }

    #[test]
    fn builds_sentences() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(to_sentence(&names(&["A", "B"]), "+"), "A+B");
        assert_eq!(to_sentence(&names(&["A", "B", "C"]), "+"), "A, B, and C");
        assert_eq!(to_sentence(&names(&["A"]), " and "), "A");
        assert_eq!(to_sentence(&names(&["A", "B"]), " and "), "A and B");
    }

    #[test]
    fn qr_code_links_take_the_url_urlsafe_base64_encoded() {
        // `Base64.urlsafe_encode64` in the reference: padded, with `-` and `_`.
        assert!(link_to_zoom_qr_code("http://x/?a", "").0.contains("href=\"/qr_code/aHR0cDovL3gvP2E=\""));
        assert!(link_to_zoom_qr_code("http://x/?>?", "").0.contains("href=\"/qr_code/aHR0cDovL3gvPz4_\""));
    }
}
