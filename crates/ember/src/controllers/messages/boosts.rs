//! `Messages::BoostsController` (reference/app/controllers/messages/boosts_controller.rb). Its
//! `show`/`edit`/`update` routes have no action or template (`action_not_found`).

pub mod by_bots;

use askama::Template;
use ember_db::{Boost, Message, Room};
use ember_kit::{Ctx, Error, Result, StatusCode, format, permit_keys};
use ember_views::messages as views;
use ruby_compat::integer_cast;

use super::present;
use crate::app::AppCtx;
use crate::concerns::{Before, before_actions, require_current_user};
use crate::controllers::presenters::page::{self, Rendered};
use crate::controllers::presenters::user_view;

pub async fn index(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let message = set_message(c).await?;
    c.respond_to(&[&format::HTML])?;
    let view = present(c, move |presenter| presenter.message(&message)).await?;
    page::content(c, StatusCode::OK, |ctx| views::BoostsIndex { ctx, message: &view }.render()).await
}

pub async fn new(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let message = set_message(c).await?;
    c.respond_to(&[&format::HTML])?;
    let user = user_view(&c.app().secrets, require_current_user(c)?);
    let view = present(c, move |presenter| presenter.message(&message)).await?;
    page::content(c, StatusCode::OK, |ctx| views::NewBoost { ctx, message: &view, user: &user }.render()).await
}

pub async fn create(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let message = set_message(c).await?;
    // params.require(:boost).permit(:content)
    let content = c.params.require("boost")?.permit(&permit_keys(&["content"])).get("content").and_then(|p| p.as_str()).map(str::to_string);
    let boost = create_boost(c, &message, content).await?;
    broadcast_create(c, &message, &boost).await?;
    let url = c.url_for(&ember_routes::message_boosts(message.id));
    c.redirect_to(&url)
}

pub async fn destroy(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let message = set_message(c).await?;
    let boost = set_boost(c, &message).await?;
    destroy_boost(c, &message, boost).await?;
    // No destroy template: `head :no_content`.
    Ok(c.head(StatusCode::NO_CONTENT))
}

/// `Current.user.reachable_messages.find(params[:message_id])`
async fn set_message(c: &mut Ctx) -> Result<Message> {
    let user_id = require_current_user(c)?.id;
    let Some(id) = c.param_str("message_id").and_then(integer_cast) else { return Err(Error::NotFound) };
    c.app().read(move |conn| Message::find_reachable(conn, user_id, id)).await
}

/// `@message.boosts.find_by!(id: params[:id], booster: Current.user)`
pub(crate) async fn set_boost(c: &mut Ctx, message: &Message) -> Result<Boost> {
    let user_id = require_current_user(c)?.id;
    let Some(id) = c.param_str("id").and_then(integer_cast) else { return Err(Error::NotFound) };
    let message_id = message.id;
    c.app().read(move |conn| Boost::find_by_message_and_booster(conn, message_id, id, user_id)).await
}

/// `@message.boosts.create!(content:)`, boosted by `Current.user`.
pub(crate) async fn create_boost(c: &Ctx, message: &Message, content: Option<String>) -> Result<Boost> {
    let booster_id = require_current_user(c)?.id;
    let message_id = message.id;
    // A nil content violates the column's NOT NULL (ActiveRecord::NotNullViolation, a 500).
    let content = content.ok_or_else(|| Error::internal(anyhow::anyhow!("NOT NULL constraint failed: boosts.content")))?;
    c.app().write(move |tx| Boost::create(tx, message_id, booster_id, &content)).await
}

/// `@boost.destroy!` then `broadcast_remove`.
pub(crate) async fn destroy_boost(c: &Ctx, message: &Message, boost: Boost) -> Result<()> {
    let destroyed = boost.clone();
    c.app().write(move |tx| destroyed.destroy(tx)).await?;
    let room_id = message.room_id;
    let room = c.app().read(move |conn| Room::find(conn, room_id)).await?;
    c.app().broadcasts.boost_remove(&room, &boost);
    Ok(())
}

/// `broadcast_create`: `messages/boosts/_boost` appended to the message's boosts.
pub(crate) async fn broadcast_create(c: &Ctx, message: &Message, boost: &Boost) -> Result<()> {
    let (app, message, boost) = (c.app().clone(), message.clone(), boost.clone());
    let base_url = page::renderer_base_url(c);
    c.app()
        .read(move |conn| {
            let presenter = crate::controllers::presenters::Presenter::new(conn, &app, None);
            let view = presenter.boost(&boost)?;
            let account = ember_db::Account::first(conn)?;
            let html = page::render_detached_at(&app, account.as_ref(), &base_url, |ctx| views::boost(ctx, &view));
            let room = Room::find(conn, message.room_id)?;
            let partials = Rendered { boost: Some(html), ..Rendered::default() };
            app.broadcasts.boost_create(&room, &message, &boost, &partials);
            Ok(())
        })
        .await
}
