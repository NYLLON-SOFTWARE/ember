//! `Rooms::RefreshesController` (reference/app/controllers/rooms/refreshes_controller.rb): what
//! changed in a room since the client last loaded it.

use askama::Template;
use ember_db::{Message, Timestamp};
use ember_kit::{Ctx, Error, Result, StatusCode, format};
use ember_views::rooms::{RefreshShow, RefreshView};

use crate::app::AppCtx;
use crate::concerns::{self, Before, before_actions};
use crate::controllers::presenters::page;
use crate::controllers::presenters::{Presenter, room_kind};

pub async fn show(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let (_, room) = concerns::set_room(c).await?;
    let last_updated_at = set_last_updated_at(c)?;
    c.respond_to(&[&format::TURBO_STREAM])?;

    let app = c.app().clone();
    let request_host = Some(c.request.host());
    let refresh = c
        .app()
        .read_offloaded(move |conn| {
            let new_messages = Message::page_created_since(conn, room.id, last_updated_at)?;
            let new_ids: Vec<i64> = new_messages.iter().map(|message| message.id).collect();
            let updated_messages = Message::page_updated_since(conn, room.id, last_updated_at, &new_ids)?;
            let presenter = Presenter::new(conn, &app, request_host);
            ember_views::fragment_cache::with(&app.fragment_cache, || {
                Ok(RefreshView {
                    room_id: room.id,
                    room_kind: room_kind(room.room_type),
                    new_messages: presenter.messages(&new_messages)?,
                    updated_messages: presenter.messages(&updated_messages)?,
                })
            })
        })
        .await?;
    page::bare(c, StatusCode::OK, &format::TURBO_STREAM, |ctx| RefreshShow { ctx, refresh: &refresh }.render()).await
}

/// `Time.at(0, params[:since].to_i, :millisecond)`
fn set_last_updated_at(c: &Ctx) -> Result<Timestamp> {
    let since = match c.param("since") {
        None => 0,
        Some(param) => match param.as_str() {
            Some(value) => ruby_compat::to_i(value),
            None if param.is_null() => 0,
            // `to_i` isn't defined for a hash or an array.
            None => return Err(Error::internal(anyhow::anyhow!("undefined method 'to_i'"))),
        },
    };
    // Outside the representable range (a crafted `since`), the nearest end of it.
    let since = jiff::Timestamp::from_microsecond(since.saturating_mul(1000)).unwrap_or(if since < 0 {
        jiff::Timestamp::MIN
    } else {
        jiff::Timestamp::MAX
    });
    Ok(Timestamp::from_jiff(since))
}
