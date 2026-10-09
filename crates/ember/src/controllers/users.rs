//! `UsersController` (reference/app/controllers/users_controller.rb): joining with the account's
//! join code, and a user's page.

pub mod avatars;
pub mod bans;
pub mod profiles;
pub mod push_subscriptions;
pub mod sidebars;

use askama::Template;
use ember_db::{Account, NewUser, User};
use ember_kit::{Ctx, Error, ParamMap, Result, StatusCode, format, halt, permit_keys};
use ember_views::users;
use ruby_compat::integer_cast;

use super::presenters::attachments::{self, Assignment, Record};
use super::presenters::{self, view_context};
use crate::app::AppCtx;
use crate::concerns::{self, Before};
use crate::controllers::presenters::page::framed_page;

/// `require_unauthenticated_access only: %i[ new create ]`, `before_action :verify_join_code`
pub async fn new(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default().require_unauthenticated_access()).await?;
    let account = verify_join_code(c).await?;
    c.respond_to(&[&format::HTML])?;
    let help_contact = c.app().read(presenters::accounts::help_contact).await?;
    let join_code = account.join_code;
    framed_page!(c, StatusCode::OK, |ctx| users::New { ctx, join_code: join_code.clone(), help_contact: help_contact.clone() }).await
}

pub async fn create(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default().require_unauthenticated_access()).await?;
    verify_join_code(c).await?;
    let params = user_params(c)?;
    let email_address = params.get("email_address").and_then(|p| p.to_s());
    let attributes = NewUser {
        // users.name is NOT NULL: a missing name fails the insert, as in Rails.
        name: params
            .get("name")
            .and_then(|p| p.to_s())
            .ok_or_else(|| Error::internal(anyhow::anyhow!("NOT NULL constraint failed: users.name")))?,
        email_address: email_address.clone(),
        password_digest: concerns::password_digest(
            c,
            params.get("password").and_then(|p| p.to_s()).filter(|password| !password.is_empty()),
        )
        .await?,
        ..NewUser::default()
    };
    let avatar = Assignment::from_params(&params, "avatar")?.stage(c.app()).await?;

    // `User.create!(user_params)`
    let result = c
        .app()
        .db
        .write(move |tx| {
            let user = User::create(tx, attributes)?;
            let pending = attachments::assign(tx, Record::user(user.id), "avatar", avatar)?;
            Ok((user, pending))
        })
        .await;
    match result {
        Ok((user, pending)) => {
            attachments::analyze_later(c.app(), pending);
            concerns::start_new_session_for(c, user).await?;
            let root = c.url_for(&ember_routes::root());
            c.redirect_to(&root)
        }
        // rescue ActiveRecord::RecordNotUnique: `redirect_to new_session_url(email_address: user_params[:email_address])`
        Err(error) if error.is_record_not_unique() => {
            let mut location = c.url_for(&ember_routes::new_session());
            if let Some(email_address) = email_address {
                location.push_str(&format!("?email_address={}", ruby_compat::cgi_escape(&email_address)));
            }
            c.redirect_to(&location)
        }
        Err(error) => Err(Error::internal(error)),
    }
}

/// `before_action :set_user, only: :show`
pub async fn show(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    let user = find_user(c, "id").await?;
    c.respond_to(&[&format::HTML])?;
    let secrets = c.app().secrets.clone();
    let transfer_id = presenters::accounts::transfer_id(&secrets, user.id, c.now());
    let user = presenters::user_summary(&secrets, &user);
    view_context::page_or_frame(
        c,
        StatusCode::OK,
        |ctx| show_page(ctx, &user, &transfer_id).render(),
        |ctx| {
            let page = show_page(ctx, &user, &transfer_id);
            ember_views::layouts::frame(ctx, page.as_head(), page.as_content())
        },
    )
    .await
}

fn show_page<'a>(ctx: &'a ember_views::ViewContext<'a>, user: &users::UserSummary, transfer_id: &str) -> users::Show<'a> {
    users::Show { ctx, user: user.clone(), transfer_id: transfer_id.to_string() }
}

/// `User.find(params[key])`: 404 when there's no such user.
pub async fn find_user(c: &Ctx, key: &str) -> Result<User> {
    let id = c.param_str(key).and_then(integer_cast).ok_or(Error::NotFound)?;
    c.app().read(move |conn| User::find_by_id(conn, id)).await?.ok_or(Error::NotFound)
}

/// `head :not_found if Current.account.join_code != params[:join_code]`
async fn verify_join_code(c: &mut Ctx) -> Result<Account> {
    let account = c
        .app()
        .read(Account::first)
        .await?
        // `Current.account.join_code` on nil raises NoMethodError.
        .ok_or_else(|| Error::internal(anyhow::anyhow!("undefined method 'join_code' for nil")))?;
    if c.param_str("join_code") != Some(account.join_code.as_str()) {
        return halt(c.head(StatusCode::NOT_FOUND));
    }
    Ok(account)
}

/// `params.require(:user).permit(:name, :avatar, :email_address, :password)`
fn user_params(c: &Ctx) -> Result<ParamMap> {
    Ok(c.params.require("user")?.permit(&permit_keys(&["name", "avatar", "email_address", "password"])))
}
