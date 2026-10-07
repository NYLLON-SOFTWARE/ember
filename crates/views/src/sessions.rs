//! Views for `reference/app/views/sessions`.

use askama::Template;

use crate::ViewContext;
use crate::accounts::HelpContact;
use crate::helpers::{self as h, filters};
use crate::layouts::Page;

/// `AllowBrowser::VERSIONS`, minus the browsers it blocks outright (`ie: false`).
pub const ALLOW_BROWSER_VERSIONS: [(&str, &str); 4] = [("safari", "17.2"), ("chrome", "120"), ("firefox", "121"), ("opera", "104")];

/// `sessions/new.html.erb`.
#[derive(Template)]
#[template(path = "sessions/new.html", blocks = ["head", "content"])]
pub struct New<'a> {
    pub ctx: &'a ViewContext<'a>,
    /// `params[:email_address]`.
    pub email_address: Option<String>,
    /// `User.administrator.first`, for `accounts/_help_contact`.
    pub help_contact: Option<HelpContact>,
}

impl Page for New<'_> {
    fn page_title(&self) -> Option<String> {
        Some("Sign in".into())
    }
}

/// `sessions/incompatible_browser.html.erb`, rendered by `AllowBrowser` for old browsers.
#[derive(Template)]
#[template(path = "sessions/incompatible_browser.html", blocks = ["head", "content"])]
pub struct IncompatibleBrowser<'a> {
    pub ctx: &'a ViewContext<'a>,
}

impl Page for IncompatibleBrowser<'_> {
    fn page_title(&self) -> Option<String> {
        Some(if self.ctx.platform.apple_messages { "Matchbox" } else { "Unsupported browser" }.into())
    }
}

/// `sessions/transfers/show.html.erb`: an auto-submitting form that PUTs back to the page's
/// own URL (`url_for({})`, i.e. `session_transfer_path(id)`).
#[derive(Template)]
#[template(path = "sessions/transfers/show.html", blocks = ["head", "content"])]
pub struct TransferShow<'a> {
    pub ctx: &'a ViewContext<'a>,
    /// The request path, `session_transfer_path(params[:id])`.
    pub action: String,
}

impl Page for TransferShow<'_> {}
