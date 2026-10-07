//! `Messages::ByBotsController` (reference/app/controllers/messages/by_bots_controller.rb): the
//! bot API under `/rooms/:room_id/:bot_key/messages` (JSON by route default). Bodies are the raw
//! request body (`RawRequestBody`), or a top-level multipart `attachment`.

use matchbox_db::{Message, Room};
use matchbox_kit::{Ctx, Param, Response, Result, StatusCode, format, halt, permit_keys};
use matchbox_views::messages::json;
use ruby_compat::integer_cast;

use super::{
    MessageParams, attachment_assignment, broadcast_create, broadcast_replace, create_message, deliver_webhooks_to_bots, destroy_message,
    ensure_can_administer, find_paged_messages, present, set_message, update_message,
};
use crate::app::AppCtx;
use crate::concerns::{self, Before, before_actions, require_current_user};

fn before() -> Before {
    Before::default().allow_bot_access()
}

pub async fn index(c: &mut Ctx) -> Result {
    before_actions(c, before()).await?;
    let room = set_room(c).await?;
    let messages = find_paged_messages(c, &room).await?;
    set_pagination_headers(c, &room, &messages).await?;
    c.respond_to(&[&format::JSON])?;
    let base_url = c.url_for("");
    let body = present(c, move |presenter| {
        let messages = messages.iter().map(|m| presenter.message_json(m, &base_url)).collect::<matchbox_db::Result<Vec<_>>>()?;
        Ok(json::by_bots_index(&messages))
    })
    .await?;
    Ok(c.render(StatusCode::OK, &format::JSON, body))
}

pub async fn create(c: &mut Ctx) -> Result {
    before_actions(c, before()).await?;
    let room = set_room(c).await?;
    ensure_body_or_attachment_present(c)?;
    // MessagesController#create
    let attributes = message_params(c)?;
    let message = create_message(c, &room, attributes).await?;
    broadcast_create(c, &room, &message).await?;
    deliver_webhooks_to_bots(c, &room, &message).await?;

    let location = c.url_for(&matchbox_routes::message(message.id));
    c.head_with_location(StatusCode::CREATED, &location)
}

pub async fn update(c: &mut Ctx) -> Result {
    before_actions(c, before()).await?;
    let room = set_room(c).await?;
    let message = set_message(c, &room).await?;
    ensure_can_administer(c, &message)?;
    // MessagesController#update
    let attributes = message_params(c)?;
    let message = update_message(c, message, attributes).await?;
    broadcast_replace(c, &room, &message).await?;
    match c.respond_to(&[&format::HTML, &format::JSON])? {
        f if *f == format::JSON => render_show(c, message).await,
        _ => {
            let url = c.url_for(&matchbox_routes::room_message(room.id, message.id));
            c.redirect_to(&url)
        }
    }
}

pub async fn destroy(c: &mut Ctx) -> Result {
    before_actions(c, before()).await?;
    let room = set_room(c).await?;
    let message = set_message(c, &room).await?;
    ensure_can_administer(c, &message)?;
    destroy_message(c, &room, &message).await?;
    Ok(c.head(StatusCode::NO_CONTENT))
}

/// `set_room`: `Current.user.rooms.find_by(id: params[:room_id])`, else `head :not_found`.
async fn set_room(c: &mut Ctx) -> Result<Room> {
    let user_id = require_current_user(c)?.id;
    let room = match c.param_str("room_id").and_then(integer_cast) {
        Some(id) => c.app().read(move |conn| Room::find_for_user(conn, user_id, id)).await?,
        None => None,
    };
    match room {
        Some(room) => Ok(room),
        None => halt(concerns::head(StatusCode::NOT_FOUND)),
    }
}

/// `head :unprocessable_content if params[:attachment].blank? && raw_request_body.blank?`
fn ensure_body_or_attachment_present(c: &mut Ctx) -> Result<()> {
    let attachment_blank = c.params.get("attachment").is_none_or(Param::is_blank);
    if attachment_blank && is_blank(&raw_request_body(c)) {
        return halt(concerns::head(StatusCode::UNPROCESSABLE_ENTITY));
    }
    Ok(())
}

/// `params[:attachment] ? params.permit(:attachment) : { body: raw_request_body }`
fn message_params(c: &Ctx) -> Result<MessageParams> {
    if c.params.get("attachment").is_some_and(|p| !p.is_null()) {
        let permitted = c.params.permit(&permit_keys(&["attachment"]));
        Ok(MessageParams { attachment: attachment_assignment(&permitted)?, ..MessageParams::default() })
    } else {
        Ok(MessageParams { body: Some(raw_request_body(c)), ..MessageParams::default() })
    }
}

/// `RawRequestBody#raw_request_body`: the whole body, as UTF-8.
pub(crate) fn raw_request_body(c: &Ctx) -> String {
    String::from_utf8_lossy(c.request.raw_post()).into_owned()
}

/// `String#blank?`: empty or only whitespace.
pub(crate) fn is_blank(value: &str) -> bool {
    value.chars().all(char::is_whitespace)
}

/// `X-Total-Count`, and a `Link` to the next page when there is one.
async fn set_pagination_headers(c: &mut Ctx, room: &Room, messages: &[Message]) -> Result<()> {
    let after = c.params.get("after").is_some_and(Param::is_present);
    let (room_id, first, last) = (room.id, messages.first().cloned(), messages.last().cloned());
    // Offloaded: the count reads every message in the room.
    let (count, next_page) = c
        .app()
        .read_offloaded(move |conn| {
            let count = Message::count_in_room(conn, room_id)?;
            let next_page = match (first, last) {
                (Some(_), Some(last)) if after => Message::exists_after(conn, room_id, &last)?.then_some(("after", last.id)),
                (Some(first), Some(_)) => Message::exists_before(conn, room_id, &first)?.then_some(("before", first.id)),
                _ => None,
            };
            Ok((count, next_page))
        })
        .await?;
    c.set_header("x-total-count", &count.to_string());
    if let Some((key, id)) = next_page {
        let bot_key = c.param_str("bot_key").unwrap_or_default().to_string();
        let url = c.url_for(&format!("{}?{key}={id}", matchbox_routes::room_bot_messages(room_id, bot_key)));
        c.set_header("link", &format!("<{url}>; rel=\"next\""));
    }
    Ok(())
}

/// `render :show` (`messages/by_bots/show.json.jbuilder`).
async fn render_show(c: &mut Ctx, message: Message) -> Result<Response> {
    let base_url = c.url_for("");
    let body = present(c, move |presenter| Ok(json::by_bots_show(&presenter.message_json(&message, &base_url)?))).await?;
    Ok(c.render(StatusCode::OK, &format::JSON, body))
}
