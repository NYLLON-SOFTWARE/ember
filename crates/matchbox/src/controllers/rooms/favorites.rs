//! Matchbox personal channel favorites, stored separately from channel ordering.

use matchbox_db::Account;
use matchbox_kit::{Ctx, Error, Result, StatusCode, format};

use crate::app::AppCtx;
use crate::concerns::{self, Before};

pub async fn update(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    let json = *c.respond_to(&[&format::HTML, &format::JSON])? == format::JSON;
    let user_id = concerns::require_current_user(c)?.id;
    let room_id = c
        .param_str("room_id")
        .and_then(|id| id.parse::<i64>().ok())
        .filter(|id| *id > 0)
        .ok_or_else(|| Error::BadRequest("invalid room ID".into()))?;
    let favorite = match c.param_str("favorite") {
        Some("true") => true,
        Some("false") => false,
        _ => return Err(Error::BadRequest("favorite must be true or false".into())),
    };
    let favorites = c
        .app()
        .write(move |tx| Account::set_channel_favorite(tx, user_id, room_id, favorite))
        .await?
        .ok_or(Error::Status(StatusCode::FORBIDDEN))?;
    if json { c.json(StatusCode::OK, &serde_json::json!({ "favorite_channels": favorites })) } else { super::redirect_to_room(c, room_id) }
}
