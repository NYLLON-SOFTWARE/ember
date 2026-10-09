//! Rendering helpers for these controllers: pages in the application layout (or turbo-rails'
//! frame layout for Turbo-Frame requests), and partials rendered outside a request for
//! broadcasts (`ApplicationController.render`).

use ember_db::{Account, Boost, Membership, Message, Room};
use ember_kit::{Ctx, Format, Result, StatusCode};
use ember_views::fragment_cache::Fragment;
use ember_views::helpers as h;
use ember_views::layouts::{Application, FrameLayout};
use ember_views::recorded::RecordedPage;
use ember_views::{Platform, ViewContext};

use crate::app::App;
use crate::channels::Partials;
use crate::controllers::presenters::view_context::{Layout, account_summary, find_template, render_recorded};

/// A template that extends `layouts/application` itself (with `blocks = ["head", "content"]`):
/// the full page, or for a Turbo-Frame request its `head` and `content` in turbo-rails' frame
/// layout (`layout -> { "turbo_rails/frame" if turbo_frame_request? }`).
///
/// `framed_page!(c, StatusCode::OK, |ctx| rooms::Show { ctx, show: &show }).await`
macro_rules! framed_page {
    ($c:expr, $status:expr, $profile:expr, |$ctx:ident| $page:expr) => {
        $crate::controllers::presenters::view_context::page_or_frame_with_styles(
            $c,
            $status,
            $profile,
            |$ctx| ember_views::render_sized!($page),
            |$ctx| {
                let page = $page;
                ember_views::layouts::frame($ctx, page.as_head(), page.as_content())
            },
        )
    };
    ($c:expr, $status:expr, |$ctx:ident| $page:expr) => {
        $crate::controllers::presenters::view_context::page_or_frame(
            $c,
            $status,
            |$ctx| ember_views::render_sized!($page),
            |$ctx| {
                let page = $page;
                ember_views::layouts::frame($ctx, page.as_head(), page.as_content())
            },
        )
    };
}
pub(crate) use framed_page;

/// A content-only template: Rails wraps it in the application layout, or in turbo-rails'
/// `layouts/turbo_rails/frame` when the request carries a `Turbo-Frame` header.
pub async fn content(c: &mut Ctx, status: StatusCode, render: impl FnOnce(&ViewContext) -> askama::Result<String>) -> Result {
    use askama::Template;

    find_template(c, &ember_kit::format::HTML)?;
    let layout = Layout::load(c).await?;
    let frame = c.is_turbo_frame_request();
    let html = layout.render(c, |ctx| {
        let content = render(ctx)?;
        if frame {
            FrameLayout { ctx, head: h::empty(), content: h::raw(content.into()) }.render()
        } else {
            Application::new(ctx, h::raw(content)).render()
        }
    })?;
    Ok(if frame { layout.frame(c, status, html) } else { layout.page(c, status, html) })
}

/// A content-only template in the application layout even for Turbo-Frame requests: a controller
/// that declares its own `layout` (MessagesController's `layout false, only: :index`) replaces
/// turbo-rails' frame layout choice.
pub async fn content_in_application_layout(
    c: &mut Ctx,
    status: StatusCode,
    render: impl FnOnce(&ViewContext) -> askama::Result<String>,
) -> Result {
    use askama::Template;

    find_template(c, &ember_kit::format::HTML)?;
    let layout = Layout::load(c).await?;
    let html = layout.render(c, |ctx| Application::new(ctx, h::raw(render(ctx)?)).render())?;
    Ok(layout.page(c, status, html))
}

/// A template rendered with `layout false` (or a turbo stream), no layout, labelled with the
/// template's format.
pub async fn bare<T: Into<RecordedPage>>(
    c: &mut Ctx,
    status: StatusCode,
    template: Format,
    render: impl FnOnce(&ViewContext) -> askama::Result<T>,
) -> Result {
    find_template(c, template)?;
    let layout = Layout::load(c).await?;
    let html = layout.render(c, render)?;
    Ok(render_recorded(c, status, template, html.into()))
}

/// Renders with the `ViewContext` `ApplicationController.render` has: no request, no
/// `Current.user`, no CSRF tokens, and the renderer's default host (`http://example.org`).
pub fn render_detached<T>(app: &App, account: Option<&Account>, render: impl FnOnce(&ViewContext) -> T) -> T {
    render_detached_at(app, account, "http://example.org", render)
}

/// The base of the URLs in a [`render_detached_at`] during a request. `SetCurrentRequest`'s
/// `default_url_options` only carries `request.host` and `request.protocol`, so the port comes
/// from the renderer's own env (`example.org:80`) and never shows: a request to
/// `http://localhost:3999` broadcasts `http://localhost/...` links
/// (`reference/app/controllers/concerns/set_current_request.rb`).
pub fn renderer_base_url(c: &Ctx) -> String {
    format!("{}{}", c.request.protocol(), c.request.host())
}

/// [`render_detached`] during a request: URLs get the request's host through
/// `default_url_options` (`SetCurrentRequest`), see [`renderer_base_url`].
pub fn render_detached_at<T>(app: &App, account: Option<&Account>, base_url: &str, render: impl FnOnce(&ViewContext) -> T) -> T {
    let asset_path = |path: &str| ember_assets::asset_path(path);
    let stylesheets = crate::controllers::presenters::view_context::stylesheet_tags();
    let ctx = ViewContext {
        current_user: None,
        account: account_summary(account, false),
        flash_notice: None,
        flash_alert: None,
        platform: Platform::default(),
        vapid_public_key: app.vapid_public_key(),
        asset_path: &asset_path,
        importmap_tags: ember_assets::javascript_importmap_tags(),
        stylesheet_tags: &stylesheets.html,
        custom_styles: None,
        cable_url: "/cable".into(),
        base_url: base_url.to_string(),
        request_url: format!("{base_url}/"),
        referrer: None,
        last_room_visited_id: None,
        app_version: app.config.app_version.clone(),
    };
    // Renders outside a request (broadcasts from jobs) share the fragment cache too.
    ember_views::fragment_cache::with(&app.fragment_cache, || render(&ctx))
}

/// The broadcast partials, rendered up front by the controller (which has the view models) and
/// handed to `channels::Broadcasts`.
#[derive(Default)]
pub struct Rendered {
    pub message: Option<Fragment>,
    pub message_presentation: Option<String>,
    pub boost: Option<Fragment>,
    pub shared_room: Option<String>,
    /// `users/sidebars/rooms/_direct`, per membership id.
    pub direct_rooms: Vec<(i64, Fragment)>,
}

impl Partials for Rendered {
    fn message(&self, _: &Message) -> String {
        self.message.as_deref().cloned().unwrap_or_default()
    }

    fn message_presentation(&self, _: &Message) -> String {
        self.message_presentation.clone().unwrap_or_default()
    }

    fn boost(&self, _: &Boost) -> String {
        self.boost.as_deref().cloned().unwrap_or_default()
    }

    fn shared_room(&self, _: &Room) -> String {
        self.shared_room.clone().unwrap_or_default()
    }

    fn direct_room(&self, membership: &Membership) -> String {
        self.direct_rooms.iter().find(|(id, _)| *id == membership.id).map(|(_, html)| String::clone(html)).unwrap_or_default()
    }
}
