//! `Messages::Boosts::ByBotsController` (reference/app/controllers/messages/boosts/by_bots_controller.rb):
//! bots boost with the raw request body as the content.

use ember_db::{Message, Room};
use ember_kit::{Ctx, Error, Result, StatusCode, format, halt};
use ember_views::messages::json;
use ruby_compat::integer_cast;

use super::{broadcast_create, create_boost, destroy_boost, set_boost};
use crate::app::AppCtx;
use crate::concerns::{self, Before, before_actions, require_current_user};
use crate::controllers::messages::by_bots::{is_blank, raw_request_body};
use crate::controllers::messages::present;

fn before() -> Before {
    Before::default().allow_bot_access()
}

pub async fn create(c: &mut Ctx) -> Result {
    before_actions(c, before()).await?;
    let message = set_message(c).await?;
    // ensure_content_present
    let content = raw_request_body(c);
    if is_blank(&content) {
        return halt(concerns::head(StatusCode::UNPROCESSABLE_ENTITY));
    }
    let boost = create_boost(c, &message, Some(content)).await?;
    broadcast_create(c, &message, &boost).await?;

    // render :show, status: :created
    c.respond_to(&[&format::JSON])?;
    let base_url = c.url_for("");
    let body = present(c, move |presenter| Ok(json::boosts_by_bots_show(&presenter.boost_json(&boost, &message, &base_url)?))).await?;
    Ok(c.render(StatusCode::CREATED, &format::JSON, body))
}

pub async fn destroy(c: &mut Ctx) -> Result {
    before_actions(c, before()).await?;
    let message = set_message(c).await?;
    // set_boost, with `rescue ActiveRecord::RecordNotFound` → head :not_found
    let boost = match set_boost(c, &message).await {
        Ok(boost) => boost,
        Err(Error::NotFound) => return Ok(concerns::head(StatusCode::NOT_FOUND)),
        Err(error) => return Err(error),
    };
    destroy_boost(c, &message, boost).await?;
    Ok(c.head(StatusCode::NO_CONTENT))
}

/// The room among `Current.user.rooms`, then its message; `head :not_found` without one.
async fn set_message(c: &mut Ctx) -> Result<Message> {
    let user_id = require_current_user(c)?.id;
    let room_id = c.param_str("room_id").and_then(integer_cast);
    let message_id = c.param_str("message_id").and_then(integer_cast);
    let message = c
        .app()
        .read(move |conn| {
            let Some(room) = room_id.map(|id| Room::find_for_user(conn, user_id, id)).transpose()?.flatten() else {
                return Ok(None);
            };
            match message_id {
                Some(id) => Ok(Message::find_by_id(conn, id)?.filter(|message| message.room_id == room.id)),
                None => Ok(None),
            }
        })
        .await?;
    match message {
        Some(message) => Ok(message),
        None => halt(concerns::head(StatusCode::NOT_FOUND)),
    }
}
