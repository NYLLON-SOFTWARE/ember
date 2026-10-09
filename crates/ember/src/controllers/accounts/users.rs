//! `Accounts::UsersController` (reference/app/controllers/accounts/users_controller.rb): the
//! people list's next pages, role changes and removal.

use askama::Template;
use ember_db::{Role, User, UserChanges};
use ember_kit::{Ctx, Error, Result, format};
use ember_views::accounts;
use ruby_compat::integer_cast;

use crate::app::AppCtx;
use crate::concerns::{self, Before};
use crate::controllers::presenters;
use crate::controllers::presenters::pagination::Page;
use crate::controllers::presenters::view_context::Layout;

/// `set_page_and_extract_portion_from User.active.ordered.without_bots, per_page: 500`,
/// rendered as `index.turbo_stream.erb` (the only template, so other formats are 406).
pub async fn index(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    c.respond_to(&[&format::TURBO_STREAM])?;
    let users = c.app().read_offloaded(User::active_ordered_without_bots).await?;
    let page = Page::new(c.param_str("page"), users.len() as i64, &[500]);
    let secrets = c.app().secrets.clone();
    let users: Vec<_> = page.records(&users).iter().map(|user| presenters::user_summary(&secrets, user)).collect();
    let next_page = (!page.is_last()).then(|| page.next_param().to_string());

    let layout = Layout::load(c).await?;
    let html = layout.render(c, |ctx| accounts::UsersIndexTurboStream { ctx, users, next_page }.render())?;
    page.apply_headers(c);
    Ok(c.turbo_stream(html))
}

/// `@user.update(role: params.require(:user)[:role].presence_in(%w[ member administrator ]) || "member")`
pub async fn update(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    concerns::ensure_can_administer(c)?;
    let mut user = set_user(c).await?;
    let role = match c.params.require("user")?.get("role").and_then(|role| role.as_str()) {
        Some("administrator") => Role::Administrator,
        _ => Role::Member,
    };
    c.app().write(move |tx| user.update(tx, UserChanges { role: Some(role), ..UserChanges::default() })).await?;
    redirect_to_edit_account(c)
}

/// `@user.deactivate`
pub async fn destroy(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    concerns::ensure_can_administer(c)?;
    let mut user = set_user(c).await?;
    c.app().write(move |tx| user.deactivate(tx)).await?;
    redirect_to_edit_account(c)
}

/// `User.active.find(params[:user_id] || params[:id])`
async fn set_user(c: &Ctx) -> Result<User> {
    let id = c.param_str("user_id").or_else(|| c.param_str("id")).and_then(integer_cast).ok_or(Error::NotFound)?;
    c.app().read(move |conn| User::find_active(conn, id)).await
}

fn redirect_to_edit_account(c: &mut Ctx) -> Result {
    let location = c.url_for(&ember_routes::edit_account());
    c.redirect_to(&location)
}
