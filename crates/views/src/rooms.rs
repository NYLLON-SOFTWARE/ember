//! Views for `reference/app/views/rooms`, plus `RoomsHelper`, `Rooms::InvolvementsHelper` and
//! the `MessagesHelper` tags the room screen uses.

use askama::Template;
use jiff::Timestamp;
use serde::Deserialize;

use crate::ViewContext;
use crate::helpers as h;
use crate::layouts::Page;
use crate::messages::support::epoch_ms;
use crate::messages::{MessageItem, RoomKind, UserView, room_dom_id};

/// `room_display_name(room, for_user:)`: a direct room is named after its other members
/// (`room.users.without(for_user).pluck(:name).to_sentence`), falling back to the user's own
/// name when they're alone in it.
pub fn room_display_name(name: Option<&str>, direct: bool, other_member_names: &[String], for_user_name: Option<&str>) -> String {
    if direct {
        let sentence = h::to_sentence(other_member_names, " and ");
        if sentence.trim().is_empty() { for_user_name.unwrap_or_default().to_string() } else { sentence }
    } else {
        name.unwrap_or_default().to_string()
    }
}

/// `mention_prompt_tag(room)`'s `src`: `autocompletable_users_path(room_id: room.id)`.
pub fn mention_prompt_src(room_id: i64) -> String {
    format!("{}?room_id={room_id}", ember_routes::autocompletable_users())
}

/// A persisted room.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RoomView {
    pub id: i64,
    pub kind: RoomKind,
    pub name: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    /// `room_display_name(room)` for `Current.user`.
    pub display_name: String,
}

impl RoomView {
    pub fn dom_id(&self, prefix: &str) -> String {
        room_dom_id(self.kind, self.id, prefix)
    }

    pub fn is_direct(&self) -> bool {
        self.kind.is_direct()
    }

    /// `edit_polymorphic_path(room)`: `/rooms/opens/1/edit` and so on.
    pub fn edit_path(&self) -> String {
        match self.kind {
            RoomKind::Open => ember_routes::edit_rooms_open(self.id),
            RoomKind::Closed => ember_routes::edit_rooms_closed(self.id),
            RoomKind::Direct => ember_routes::edit_rooms_direct(self.id),
        }
    }

    /// "Ping" for direct rooms, "room" otherwise.
    pub fn noun(&self) -> &'static str {
        if self.is_direct() { "Ping" } else { "room" }
    }
}

/// What `rooms/show` shows.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ShowView {
    pub room: RoomView,
    #[serde(default)]
    pub favorite: bool,
    /// `room.updated_at`, the refresh controller's `loaded_at`.
    pub updated_at: Timestamp,
    /// `Current.user`, for the client-side message template.
    pub user: UserView,
    pub messages: Vec<MessageItem>,
    /// `@room == Room.original && !@room.messages.paged?` (`rooms/show/_invitation`).
    pub invitation: bool,
    /// `Current.account.join_code`, for the invitation's join link.
    #[serde(default)]
    pub join_code: String,
    /// `Turbo::StreamsChannel.signed_stream_name([room, :messages])`.
    pub messages_stream_name: String,
}

/// `rooms/show`.
#[derive(Template)]
#[template(path = "rooms/show.html", blocks = ["head", "content"])]
pub struct Show<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub show: &'a ShowView,
}

impl Page for Show<'_> {
    fn page_title(&self) -> Option<String> {
        Some(self.show.room.display_name.clone())
    }

    fn body_class(&self) -> Option<&str> {
        Some("sidebar")
    }
}

impl Show<'_> {
    fn loaded_at(&self) -> i64 {
        epoch_ms(self.show.updated_at)
    }
}

/// `Membership#involvement`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct InvolvementView {
    pub room_id: i64,
    pub kind: RoomKind,
    /// "mentions", "everything", "nothing" or "invisible".
    pub involvement: String,
}

/// `rooms/involvements/show`.
#[derive(Template)]
#[template(path = "rooms/involvements/show.html")]
pub struct InvolvementShow<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub involvement: &'a InvolvementView,
}

impl InvolvementShow<'_> {
    fn button(&self) -> h::Html {
        let room = h::InvolvementRoom {
            id: self.involvement.room_id,
            param_key: self.involvement.kind.param_key(),
            direct: self.involvement.kind.is_direct(),
        };
        h::button_to_change_involvement(self.ctx, &room, &self.involvement.involvement)
    }
}

/// What `rooms/refreshes/show` streams: messages created and updated since the client loaded.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RefreshView {
    pub room_id: i64,
    pub room_kind: RoomKind,
    pub new_messages: Vec<MessageItem>,
    pub updated_messages: Vec<MessageItem>,
}

/// `rooms/refreshes/show.turbo_stream`.
#[derive(Template)]
#[template(path = "rooms/refreshes/show.turbo_stream.html")]
pub struct RefreshShow<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub refresh: &'a RefreshView,
}

/// The room being created or edited by the open and closed room forms. `id` is `None` for a
/// new record.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct FormRoom {
    pub id: Option<i64>,
    pub name: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
}

/// `rooms/opens/{new,edit}`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct OpenFormView {
    pub room: FormRoom,
    /// `Current.user.can_administer?(room)`: administrators, the creator, or a new room.
    pub can_administer: bool,
    /// `User.active.ordered`.
    pub users: Vec<UserView>,
}

/// `rooms/closeds/{new,edit}`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ClosedFormView {
    pub room: FormRoom,
    pub can_administer: bool,
    pub current_user_id: i64,
    /// Active users with access (none for a new room).
    pub selected_users: Vec<UserView>,
    /// The other active users.
    pub unselected_users: Vec<UserView>,
}

/// `rooms/opens/new`.
#[derive(Template)]
#[template(path = "rooms/opens/new.html", blocks = ["head", "content"])]
pub struct OpensNew<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub form: &'a OpenFormView,
}

/// `rooms/opens/edit`.
#[derive(Template)]
#[template(path = "rooms/opens/edit.html", blocks = ["head", "content"])]
pub struct OpensEdit<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub form: &'a OpenFormView,
}

/// `rooms/closeds/new`.
#[derive(Template)]
#[template(path = "rooms/closeds/new.html", blocks = ["head", "content"])]
pub struct ClosedsNew<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub form: &'a ClosedFormView,
}

/// `rooms/closeds/edit`.
#[derive(Template)]
#[template(path = "rooms/closeds/edit.html", blocks = ["head", "content"])]
pub struct ClosedsEdit<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub form: &'a ClosedFormView,
}

impl Page for OpensNew<'_> {
    fn page_title(&self) -> Option<String> {
        Some("New chat room".into())
    }
}

impl Page for ClosedsNew<'_> {
    fn page_title(&self) -> Option<String> {
        Some("New chat room".into())
    }
}

impl Page for OpensEdit<'_> {
    fn page_title(&self) -> Option<String> {
        Some(format!("Edit settings for {}", self.form.room.name.as_deref().unwrap_or_default()))
    }
}

impl Page for ClosedsEdit<'_> {
    fn page_title(&self) -> Option<String> {
        Some(format!("Edit settings for {}", self.form.room.name.as_deref().unwrap_or_default()))
    }
}

impl FormRoom {
    /// `form_with model: room`'s action for an open or closed room.
    fn action(&self, kind: RoomKind) -> String {
        match (self.id, kind) {
            (Some(id), RoomKind::Open) => ember_routes::rooms_open(id),
            (Some(id), _) => ember_routes::rooms_closed(id),
            (None, RoomKind::Open) => ember_routes::rooms_opens(),
            (None, _) => ember_routes::rooms_closeds(),
        }
    }

    fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or_default()
    }
}

/// `rooms/directs/new`.
#[derive(Template)]
#[template(path = "rooms/directs/new.html", blocks = ["head", "content"])]
pub struct DirectsNew<'a> {
    pub ctx: &'a ViewContext<'a>,
}

impl Page for DirectsNew<'_> {}

/// `rooms/directs/edit`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct DirectEditView {
    pub room_id: i64,
    /// `room_display_name(@room)` for `Current.user`.
    pub display_name: String,
    /// `@room.users.many? ? @room.users.without(Current.user) : @room.users`.
    pub users: Vec<UserView>,
}

#[derive(Template)]
#[template(path = "rooms/directs/edit.html", blocks = ["head", "content"])]
pub struct DirectsEdit<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub edit: &'a DirectEditView,
}

impl Page for DirectsEdit<'_> {
    fn page_title(&self) -> Option<String> {
        Some(format!("Edit settings for {}", self.edit.display_name))
    }
}

impl OpensEdit<'_> {
    fn room_id(&self) -> i64 {
        self.form.room.id.unwrap_or_default()
    }
}

impl ClosedsEdit<'_> {
    fn room_id(&self) -> i64 {
        self.form.room.id.unwrap_or_default()
    }
}

/// `button_to_delete_room(room)`.
pub fn button_to_delete_room(ctx: &ViewContext, room_id: i64, display_name: &str) -> h::Html {
    let url = ctx.url(&ember_routes::room(room_id));
    let content = format!(
        r#"<img aria-hidden="true" src="{}" width="20" height="20" /><span class="overflow-ellipsis">{}</span>"#,
        h::escape(&ctx.asset("trash.svg")),
        h::escape(display_name)
    );
    let options = h::attrs()
        .method("delete")
        .class("btn btn--negative max-width")
        .aria("label", format!("Delete {display_name}"))
        .data("turbo_confirm", "Are you sure you want to delete this room and all messages in it? This can’t be undone.");
    h::button_to(&url, options, &content)
}

/// `rooms/layouts/_form`, the form wrapped around the open and closed room forms' fields.
#[derive(Template)]
#[template(path = "rooms/layouts/_form.html")]
pub struct FormLayout<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub room: &'a FormRoom,
    pub can_administer: bool,
    pub kind: RoomKind,
    pub content: String,
}

/// Block helpers for the room templates: A's shared ones plus `rooms/layouts/_form`.
mod filters {
    pub use crate::helpers::filters::*;

    use std::fmt::Display;

    use askama::{Template, Values};

    use super::{FormLayout, FormRoom, RoomKind};
    use crate::ViewContext;
    use crate::helpers::Html;

    /// `render layout: "rooms/layouts/form", locals: { room: } do ... end`.
    pub fn room_form(
        content: impl Display,
        _: &dyn Values,
        ctx: &ViewContext,
        room: &FormRoom,
        can_administer: &bool,
        kind: RoomKind,
    ) -> askama::Result<Html> {
        let layout = FormLayout { ctx, room, can_administer: *can_administer, kind, content: content.to_string() };
        Ok(Html::from(askama::filters::Safe(layout.render()?)))
    }
}
