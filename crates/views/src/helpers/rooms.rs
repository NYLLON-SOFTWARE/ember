//! The parts of `RoomsHelper` and `Rooms::InvolvementsHelper` the sidebar and the profile's
//! memberships use; the rest of the rooms helpers are in `crate::rooms`.

use std::fmt::Display;

use super::assets::image_tag;
use super::forms::button_to;
use super::html::Html;
use super::links::link_options;
use super::tag::{AttrValue, Attrs, attrs, content_tag_block, content_tag_text};
use super::turbo::dom_id;
use super::url::{Param, with_query};
use crate::ViewContext;

/// `link_to_room(room, **attributes) { content }`. `options` is the attribute hash in Ruby
/// order, `data-*` entries included where the `data:` key was.
pub fn link_to_room(room_id: i64, options: &Attrs, content: impl Display) -> askama::Result<Html> {
    let id = room_id.to_string();
    let defaults = [
        ("data-rooms-list-target", AttrValue::Text("room")),
        ("data-room-id", AttrValue::Text(&id)),
        ("data-badge-dot-target", AttrValue::Text("unread")),
        ("data-sorted-list-target", AttrValue::Text("item")),
    ];
    let url = matchbox_routes::room(room_id);
    content_tag_block("a", link_options(&url, options.view().with_default_data(&defaults)), content)
}

/// `HUMANIZE_INVOLVEMENT`.
pub fn humanize_involvement(involvement: &str) -> &'static str {
    match involvement {
        "mentions" => "Notifying about @ mentions",
        "everything" => "Notifying about all messages",
        "nothing" => "Notifications are off",
        "invisible" => "Notifications are off and room invisible in sidebar",
        _ => "",
    }
}

/// `next_involvement_for(room, involvement:)`.
pub fn next_involvement(direct: bool, involvement: &str) -> &'static str {
    let order: &[&'static str] = if direct { &["everything", "nothing"] } else { &["mentions", "everything", "nothing", "invisible"] };
    let index = order.iter().position(|candidate| *candidate == involvement);
    index.and_then(|index| order.get(index + 1)).copied().unwrap_or(order[0])
}

/// A room as the involvement helpers see it.
pub struct InvolvementRoom<'r> {
    pub id: i64,
    /// `model_name.param_key` of the room's class: "rooms_open", "rooms_closed" or "rooms_direct".
    pub param_key: &'r str,
    pub direct: bool,
}

/// `button_to_change_involvement(room, involvement)`.
pub fn button_to_change_involvement<'r>(ctx: &ViewContext, room: impl std::borrow::Borrow<InvolvementRoom<'r>>, involvement: &str) -> Html {
    let room = room.borrow();
    let label_id = dom_id(room.param_key, room.id, Some("involvement_label"));
    let url = with_query(
        &matchbox_routes::room_involvement(room.id),
        vec![("involvement", Param::One(next_involvement(room.direct, involvement).to_string()))],
    );
    let content = format!(
        "{}{}",
        image_tag(ctx, format!("notification-bell-{involvement}.svg"), attrs().aria_hidden().size(20)).0,
        content_tag_text("span", attrs().class("for-screen-reader").id(label_id.as_str()), humanize_involvement(involvement)).0
    );
    let options = attrs()
        .method("put")
        .role("checkbox")
        .aria("checked", true)
        .aria("labelledby", label_id.as_str())
        .tabindex(0)
        .class(format!("btn {involvement}"));
    button_to(&url, options, &content)
}
