//! `UsersHelper`, `Users::AvatarsHelper`, `Users::FilterHelper`, `Users::ProfilesHelper`,
//! `Users::SidebarHelper` and `AccountsHelper`.

use super::assets::image_tag;
use super::forms::button_to;
use super::html::Html;
use super::links::link_to;
use super::tag::{AttrValue, Attrs, AttrsView, attrs, builder_tag, content_tag, content_tag_text};
use super::turbo::turbo_frame_options;
use super::url::rooms_directs_with_users;
use crate::ViewContext;

/// `Users::AvatarsHelper::AVATAR_COLORS`.
pub const AVATAR_COLORS: [&str; 18] = [
    "#AF2E1B", "#CC6324", "#3B4B59", "#BFA07A", "#ED8008", "#ED3F1C", "#BF1B1B", "#736B1E", "#D07B53", "#736356", "#AD1D1D", "#BF7C2A",
    "#C09C6F", "#698F9C", "#7C956B", "#5D618F", "#3B3633", "#67695E",
];

/// `avatar_background_color(user)`: `Zlib.crc32(user.to_param)` picks the color.
pub fn avatar_background_color(user_id: impl std::borrow::Borrow<i64>) -> &'static str {
    let user_id = *user_id.borrow();
    let crc = crc32fast::hash(user_id.to_string().as_bytes());
    AVATAR_COLORS[(crc % AVATAR_COLORS.len() as u32) as usize]
}

/// `User#initials`: `name.scan(/\b\w/).join`. Ruby's `\w` is ASCII-only while `\b` sees
/// Unicode word characters, so "Élodie" contributes nothing.
pub fn initials(name: &str) -> String {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut previous: Option<char> = None;
    let mut out = String::new();
    for c in name.chars() {
        if (c.is_ascii_alphanumeric() || c == '_') && !previous.is_some_and(is_word) {
            out.push(c);
        }
        previous = Some(c);
    }
    out
}

/// `User#title`: `[ name, bio ].compact_blank.join(" – ")`.
pub fn user_title(name: &str, bio: Option<&str>) -> String {
    [Some(name), bio].into_iter().flatten().filter(|part| !part.trim().is_empty()).collect::<Vec<_>>().join(" – ")
}

/// What `avatar_tag` needs to know about a user.
#[derive(Clone, Debug, Default)]
pub struct AvatarUser {
    pub id: i64,
    /// `User#title`.
    pub title: String,
    /// `fresh_user_avatar_path(user)`.
    pub avatar_path: String,
}

/// `avatar_tag(user, **options)`: the options go to the image.
pub fn avatar_tag(ctx: &ViewContext, user: impl std::borrow::Borrow<AvatarUser>, options: Attrs) -> Html {
    let user = user.borrow();
    let image = image_tag(ctx, &user.avatar_path, attrs().aria_hidden().size(48).merge(options));
    link_to(&ember_routes::user(user.id), attrs().title(user.title.as_str()).class("btn avatar").data("turbo_frame", "_top"), &image.0)
}

/// `button_to_direct_room_with(user)`.
pub fn button_to_direct_room_with(ctx: &ViewContext, user_id: impl std::borrow::Borrow<i64>) -> Html {
    let user_id = *user_id.borrow();
    button_to(
        &rooms_directs_with_users(&[user_id]),
        attrs().class("btn btn--primary full-width txt--large"),
        &image_tag(ctx, "messages.svg", attrs()).0,
    )
}

/// The bot curl commands in `accounts/bots/_bot`.
pub fn curl_text_line(url: impl AsRef<str>) -> String {
    format!("curl -d 'Hello!' {}", url.as_ref())
}

pub fn curl_upload_line(url: impl AsRef<str>) -> String {
    format!("curl -F \"attachment=@/path/to/file\" {}", url.as_ref())
}

/// `account_logo_tag(style:)`. A nil style leaves a trailing space in the class.
pub fn account_logo_tag(ctx: &ViewContext, style: Option<&str>) -> Html {
    let image = image_tag(ctx, &ctx.account.logo_url, attrs().alt("Account logo").size(300));
    content_tag("figure", attrs().class(format!("account-logo avatar {}", style.unwrap_or(""))), &image.0)
}

/// `profile_form_submit_button`.
pub fn profile_form_submit_button(ctx: &ViewContext) -> Html {
    let content = format!(
        "{}{}",
        image_tag(ctx, "check.svg", attrs().aria_hidden().size(20)).0,
        content_tag_text("span", attrs().class("for-screen-reader"), "Save changes").0
    );
    content_tag("button", attrs().class("btn btn--reversed center txt-large").type_("submit"), &content)
}

/// `sidebar_turbo_frame_tag(src:) { content }`.
pub fn sidebar_turbo_frame_tag(src: Option<&str>, content: &str) -> Html {
    content_tag("turbo-frame", sidebar_turbo_frame_options(src), content)
}

/// `sidebar_turbo_frame_tag`'s attributes: `data: { turbo_permanent: true, controller: ...,
/// rooms_list_unread_class: ..., action: ... }` for `turbo_frame_tag`.
pub fn sidebar_turbo_frame_options(src: Option<&str>) -> AttrsView<'_> {
    let data = AttrsView::with_capacity(7)
        .attr("data-turbo-permanent", true)
        .attr("data-controller", "rooms-list read-rooms turbo-frame")
        .attr("data-rooms-list-unread-class", "unread")
        // html_safe in the reference so "->" isn't escaped
        .attr(
            "data-action",
            AttrValue::Safe(
                "presence:present@window->rooms-list#read read-rooms:read->rooms-list#read turbo:frame-load->rooms-list#loaded refresh-room:visible@window->turbo-frame#reload",
            ),
        );
    turbo_frame_options("user_sidebar", src, Some("_top"), data)
}

/// `user_filter_menu_tag { content }`.
pub fn user_filter_menu_tag(content: &str) -> Html {
    let options = attrs()
        .class("flex flex-column gap margin-none pad overflow-y constrain-height")
        .data("controller", "filter")
        .data("filter_active_class", "filter--active")
        .data("filter_selected_class", "selected");
    content_tag("menu", &options, content)
}

/// `user_filter_search_tag`.
pub fn user_filter_search_tag() -> Html {
    builder_tag(
        "input",
        attrs()
            .type_("search")
            .id("search")
            .attr("autocorrect", "off")
            .autocomplete("off")
            .attr("data-1p-ignore", "true")
            .class("input input--transparent full-width")
            .placeholder("Filter…")
            .data("action", "input->filter#filter"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_initials_like_ruby() {
        assert_eq!(initials("Élodie Ünal-Smith o'Brien 3po _x ñ"), "SoB3_");
        assert_eq!(initials("David Heinemeier Hansson"), "DHH");
    }

    #[test]
    fn avatar_colors_like_zlib_crc32() {
        // `AVATAR_COLORS[Zlib.crc32(id.to_s) % 18]` in the reference.
        for (id, index) in [(1, 11), (2, 13), (42, 8), (1000, 3)] {
            assert_eq!(avatar_background_color(id), AVATAR_COLORS[index], "{id}");
        }
    }
}
