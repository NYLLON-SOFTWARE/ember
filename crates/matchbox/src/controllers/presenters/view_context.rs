//! Builds the `matchbox_views::ViewContext` every page renders with: what the application
//! layout and the helpers read from `Current`, `request`, `flash`, the session and the config
//! (`reference/app/views/layouts/application.html.erb` and `app/helpers`).
//!
//! Gather the per-request data with [`Layout::load`] (it reads the database), then render inside
//! [`Layout::render`], which lends the templates a `ViewContext` for this request:
//!
//! ```ignore
//! let layout = Layout::load(c).await?;
//! let html = layout.render(c, |ctx| sessions::New { ctx, email_address, help_contact }.render())?;
//! Ok(layout.page(c, StatusCode::OK, html))
//! ```

use std::sync::LazyLock;

use matchbox_assets::StyleProfile;
use matchbox_db::{Account, User};
use matchbox_kit::{Ctx, Error, Format, Response, Result, StatusCode, format};
use matchbox_views::recorded::RecordedPage;
use matchbox_views::{AccountSummary, CurrentUser, Platform, ViewContext};

use crate::app::AppCtx;
use crate::concerns;

/// Everything the layout needs, loaded before rendering.
#[derive(Debug, Clone)]
pub struct Layout {
    style_profile: StyleProfile,
    pub current_user: Option<CurrentUser>,
    pub account: AccountSummary,
    pub custom_styles: Option<String>,
    pub platform: Platform,
    pub last_room_visited_id: Option<i64>,
    pub vapid_public_key: Option<String>,
    pub app_version: String,
}

impl Layout {
    /// `Current.account`, `Current.user`, `last_room_visited` and the platform. With no account
    /// yet (first run) the account summary is blank: the pages that reference the account raise
    /// in Rails then, the others don't read it.
    pub async fn load(c: &Ctx) -> Result<Self> {
        let app = c.app();
        let secrets = app.secrets.clone();
        let user = concerns::current_user(c).cloned();
        let user_id = user.as_ref().map(|user| user.id);
        let last_room = concerns::last_room_cookie(c);
        // One trip to a reader for all three: each trip is a hand-off to a reader thread and back.
        let (account, has_logo, last_room_visited_id) = app
            .read(move |conn| {
                let account = Account::first(conn)?;
                let has_logo = match &account {
                    Some(account) => super::attachments::attached_blob(conn, "Account", account.id, "logo")?.is_some(),
                    None => false,
                };
                let last_room_visited_id = match user_id {
                    Some(user_id) => concerns::last_room_visited_in(conn, user_id, last_room)?.map(|room| room.id),
                    None => None,
                };
                Ok((account, has_logo, last_room_visited_id))
            })
            .await?;

        Ok(Self {
            style_profile: StyleProfile::Legacy,
            current_user: user.as_ref().map(|user| current_user(&secrets, user)),
            account: account_summary(account.as_ref(), has_logo),
            custom_styles: account.and_then(|account| account.custom_styles),
            platform: super::accounts::platform(c),
            last_room_visited_id,
            vapid_public_key: app.vapid_public_key(),
            app_version: app.config.app_version.clone(),
        })
    }

    /// Renders with a `ViewContext` for this request. The flash is read (and so swept at the end
    /// of the request) the way the layout's `flash[:notice]` / `flash[:alert]` read it.
    pub fn render<T>(&self, c: &mut Ctx, render: impl FnOnce(&ViewContext) -> askama::Result<T>) -> Result<T> {
        let flash_notice = c.flash().notice().map(str::to_string);
        let flash_alert = c.flash().alert().map(str::to_string);
        let base_url = c.url_for("");
        let request_url = c.request.url();
        let referrer = c.request.referer().map(str::to_string);
        let stylesheets = stylesheet_tags_for(self.style_profile);

        let asset_path = |path: &str| matchbox_assets::asset_path(path);
        let ctx = ViewContext {
            current_user: self.current_user.clone(),
            account: self.account.clone(),
            flash_notice,
            flash_alert,
            platform: self.platform.clone(),
            vapid_public_key: self.vapid_public_key.clone(),
            asset_path: &asset_path,
            importmap_tags: matchbox_assets::javascript_importmap_tags(),
            stylesheet_tags: &stylesheets.html,
            custom_styles: self.custom_styles.clone(),
            cable_url: "/cable".into(),
            base_url,
            request_url,
            referrer,
            last_room_visited_id: self.last_room_visited_id,
            app_version: self.app_version.clone(),
        };
        render(&ctx).map_err(Error::internal)
    }

    /// A page rendered in the application layout: `text/html`, plus the `Link` preload header
    /// `stylesheet_link_tag` adds (`config.action_view.preload_links_header`).
    pub fn page(&self, c: &mut Ctx, status: StatusCode, html: impl Into<RecordedPage>) -> Response {
        self.style_header(c);
        let links = &stylesheet_tags_for(self.style_profile).preload_links;
        let existing = c.headers.get("link").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        c.set_header("link", &matchbox_assets::append_preload_links(&existing, links));
        render_recorded(c, status, &format::HTML, html.into())
    }

    /// A page rendered in turbo-rails' frame layout (no stylesheets, so no `Link` header).
    pub fn frame(&self, c: &mut Ctx, status: StatusCode, html: impl Into<RecordedPage>) -> Response {
        self.style_header(c);
        render_recorded(c, status, &format::HTML, html.into())
    }

    fn style_header(&self, c: &mut Ctx) {
        // Only migrated pages and requests from them need this transition hint. Legacy responses
        // otherwise retain their existing headers, including the cached chat paths.
        let matchbox_client = c.request.header("X-Matchbox-Style-Profile").is_some();
        let campfire_client = c.request.header("X-Campfire-Style-Profile").is_some();
        if self.style_profile == StyleProfile::Basecoat
            || c.request.header("X-Ember-Style-Profile").is_some()
            || matchbox_client
            || campfire_client
        {
            let profile = match self.style_profile {
                StyleProfile::Legacy => "legacy",
                StyleProfile::Basecoat => "basecoat",
            };
            c.set_header("X-Ember-Style-Profile", profile);
            if matchbox_client {
                c.set_header("X-Matchbox-Style-Profile", profile);
            }
            if campfire_client {
                c.set_header("X-Campfire-Style-Profile", profile);
            }
        }
    }
}

/// [`Ctx::render`] for a recorded page: its text with its cached fragments spliced in, which the
/// kit keeps as parts rather than joining them.
pub fn render_recorded(c: &mut Ctx, status: StatusCode, template: Format, page: RecordedPage) -> Response {
    let (text, fragments) = page.into_parts();
    c.render_spliced(status, template, text, fragments)
}

/// The layout's `stylesheet_link_tag :all, "data-turbo-track": "reload"`: the assets are fixed at
/// build time, so it renders once per process.
pub fn stylesheet_tags() -> &'static matchbox_assets::StylesheetTags {
    stylesheet_tags_for(StyleProfile::Legacy)
}

fn stylesheet_tags_for(profile: StyleProfile) -> &'static matchbox_assets::StylesheetTags {
    static LEGACY: LazyLock<matchbox_assets::StylesheetTags> =
        LazyLock::new(|| matchbox_assets::stylesheet_link_tag_for(StyleProfile::Legacy, &[("data-turbo-track", "reload")]));
    static BASECOAT: LazyLock<matchbox_assets::StylesheetTags> =
        LazyLock::new(|| matchbox_assets::stylesheet_link_tag_for(StyleProfile::Basecoat, &[("data-turbo-track", "reload")]));
    match profile {
        StyleProfile::Legacy => &LEGACY,
        StyleProfile::Basecoat => &BASECOAT,
    }
}

/// `Current.user` as the layout's meta tags and helpers see it.
pub fn current_user(secrets: &rails_compat::Secrets, user: &User) -> CurrentUser {
    CurrentUser {
        id: user.id,
        name: user.name.clone(),
        administrator: user.can_administer(None, false),
        bot: user.is_bot(),
        avatar_url: super::avatar_path(secrets, user),
    }
}

/// `Current.account` for the layout: its name, `fresh_account_logo_path` and whether a logo is
/// attached.
pub fn account_summary(account: Option<&Account>, has_logo: bool) -> AccountSummary {
    AccountSummary {
        name: account.map(|account| account.display_name().to_owned()).unwrap_or_default(),
        hide_translation_buttons: account.is_none_or(|account| account.settings().hide_translation_buttons()),
        logo_url: super::accounts::fresh_account_logo_path(account, None),
        has_logo,
    }
}

/// Renders a page in the application layout without the implicit render's template lookup: an
/// explicit `render template:` answers HTML whatever the request's format.
pub async fn page_in_any_format<T: Into<RecordedPage>>(
    c: &mut Ctx,
    status: StatusCode,
    full: impl FnOnce(&ViewContext) -> askama::Result<T>,
) -> Result {
    let layout = Layout::load(c).await?;
    let html = layout.render(c, full)?;
    Ok(layout.page(c, status, html))
}

/// Renders a page in the application layout, or, for templates that expose their `head`/`content`
/// blocks, turbo-rails' frame layout for a Turbo-Frame request
/// (`layout -> { "turbo_rails/frame" if turbo_frame_request? }`).
pub async fn page_or_frame<P: Into<RecordedPage>, F: Into<RecordedPage>>(
    c: &mut Ctx,
    status: StatusCode,
    full: impl FnOnce(&ViewContext) -> askama::Result<P>,
    frame: impl FnOnce(&ViewContext) -> askama::Result<F>,
) -> Result {
    find_template(c, &format::HTML)?;
    page_or_frame_in_any_format(c, status, full, frame).await
}

/// Opts a migrated page into its stylesheet set without changing the legacy pages' assets.
pub async fn page_or_frame_with_styles<P: Into<RecordedPage>, F: Into<RecordedPage>>(
    c: &mut Ctx,
    status: StatusCode,
    profile: StyleProfile,
    full: impl FnOnce(&ViewContext) -> askama::Result<P>,
    frame: impl FnOnce(&ViewContext) -> askama::Result<F>,
) -> Result {
    find_template(c, &format::HTML)?;
    render_page_or_frame(c, status, profile, full, frame).await
}

/// [`page_or_frame`] without the template lookup (see [`page_in_any_format`]).
pub async fn page_or_frame_in_any_format<P: Into<RecordedPage>, F: Into<RecordedPage>>(
    c: &mut Ctx,
    status: StatusCode,
    full: impl FnOnce(&ViewContext) -> askama::Result<P>,
    frame: impl FnOnce(&ViewContext) -> askama::Result<F>,
) -> Result {
    render_page_or_frame(c, status, StyleProfile::Legacy, full, frame).await
}

async fn render_page_or_frame<P: Into<RecordedPage>, F: Into<RecordedPage>>(
    c: &mut Ctx,
    status: StatusCode,
    profile: StyleProfile,
    full: impl FnOnce(&ViewContext) -> askama::Result<P>,
    frame: impl FnOnce(&ViewContext) -> askama::Result<F>,
) -> Result {
    let mut layout = Layout::load(c).await?;
    layout.style_profile = profile;
    if c.is_turbo_frame_request() {
        let html = layout.render(c, frame)?;
        Ok(layout.frame(c, status, html))
    } else {
        let html = layout.render(c, full)?;
        Ok(layout.page(c, status, html))
    }
}

/// The implicit render's template lookup (`default_render`): an action whose only template is
/// `<action>.html.erb` can't answer a request that doesn't accept HTML, which is
/// `ActionController::UnknownFormat` (406), and the response carries the template's format
/// whatever the `Accept` header preferred.
pub fn find_template(c: &mut Ctx, template: matchbox_kit::Format) -> Result<()> {
    c.respond_to(&[template]).map(|_| ())
}
