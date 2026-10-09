//! Views for `reference/app/views/accounts`.

use askama::Template;

use crate::ViewContext;
use crate::helpers::{self as h, filters};
use crate::layouts::Page;
use crate::users::UserSummary;

/// `User.administrator.first`, shown by `accounts/_help_contact`.
#[derive(Clone, Debug)]
pub struct HelpContact {
    pub name: String,
    pub email_address: String,
}

/// `accounts/_help_contact.html.erb` on its own.
#[derive(Template)]
#[template(path = "accounts/_help_contact.html")]
pub struct HelpContactPartial<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub help_contact: Option<HelpContact>,
}

/// `accounts/edit.html.erb`.
#[derive(Template)]
#[template(path = "accounts/edit.html", blocks = ["head", "content"])]
pub struct Edit<'a> {
    pub ctx: &'a ViewContext<'a>,
    /// `Current.account.id`: `form_with model: @account` posts to `/account.<id>` because the
    /// account is a singular resource (a quirk the reference ships with).
    pub account_id: i64,
    pub join_code: String,
    pub restrict_room_creation_to_administrators: bool,
    pub administrators: Vec<UserSummary>,
    pub members: Vec<UserSummary>,
    pub rooms: Vec<crate::users::SidebarRoom>,
    pub room_order: String,
    /// `@page.next_param` unless `@page.last?`.
    pub next_page: Option<String>,
}

impl Edit<'_> {
    fn account_action(&self) -> String {
        format!("{}.{}", h::routes::account(), self.account_id)
    }
}

impl Page for Edit<'_> {
    fn page_title(&self) -> Option<String> {
        Some("Account settings".into())
    }
}

/// `accounts/_invite.html.erb` on its own.
#[derive(Template)]
#[template(path = "accounts/_invite.html")]
pub struct Invite<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub join_code: String,
}

/// `accounts/users/_user.html.erb` on its own.
#[derive(Template)]
#[template(path = "accounts/users/_user.html")]
pub struct UserPartial<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub user: UserSummary,
}

/// `accounts/users/_next_page_container.html.erb` on its own.
#[derive(Template)]
#[template(path = "accounts/users/_next_page_container.html")]
pub struct NextPageContainer {
    pub page: String,
}

/// `accounts/users/index.turbo_stream.erb`.
#[derive(Template)]
#[template(path = "accounts/users/index.turbo_stream.html")]
pub struct UsersIndexTurboStream<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub users: Vec<UserSummary>,
    pub next_page: Option<String>,
}

/// A bot, as `accounts/bots/_bot` and `_form` see it.
#[derive(Clone, Debug, Default)]
pub struct Bot {
    pub user: UserSummary,
    /// `User#bot_key`: "id-token".
    pub bot_key: String,
    /// `bot.rooms.without_directs.ordered`.
    pub rooms: Vec<BotRoom>,
}

#[derive(Clone, Debug)]
pub struct BotRoom {
    pub id: i64,
    /// `room_display_name(room)`, the room's name for shared rooms.
    pub name: String,
}

/// The fields `accounts/bots/_form` fills in.
#[derive(Clone, Debug, Default)]
pub struct BotForm {
    pub name: Option<String>,
    pub webhook_url: Option<String>,
    /// `url_for(bot.avatar)` when attached.
    pub avatar_attachment_url: Option<String>,
}

/// `accounts/bots/index.html.erb`.
#[derive(Template)]
#[template(path = "accounts/bots/index.html", blocks = ["head", "content"])]
pub struct BotsIndex<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub bots: Vec<Bot>,
}

impl Page for BotsIndex<'_> {
    fn page_title(&self) -> Option<String> {
        Some("Chat bots".into())
    }
}

/// `accounts/bots/new.html.erb`.
#[derive(Template)]
#[template(path = "accounts/bots/new.html", blocks = ["head", "content"])]
pub struct BotsNew<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub bot: BotForm,
}

impl Page for BotsNew<'_> {
    fn page_title(&self) -> Option<String> {
        Some("New chat bot".into())
    }
}

/// `accounts/bots/edit.html.erb`.
#[derive(Template)]
#[template(path = "accounts/bots/edit.html", blocks = ["head", "content"])]
pub struct BotsEdit<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub bot_id: i64,
    pub bot: BotForm,
}

impl Page for BotsEdit<'_> {
    fn page_title(&self) -> Option<String> {
        Some("Edit bot".into())
    }
}

/// `accounts/custom_styles/edit.html.erb`.
#[derive(Template)]
#[template(path = "accounts/custom_styles/edit.html", blocks = ["head", "content"])]
pub struct CustomStylesEdit<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub custom_styles: Option<String>,
}

impl Page for CustomStylesEdit<'_> {
    fn page_title(&self) -> Option<String> {
        Some("Custom styles".into())
    }
}
