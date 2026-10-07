//! `WelcomeController` (reference/app/controllers/welcome_controller.rb): the root URL.

use matchbox_db::Room;
use matchbox_kit::{Ctx, Error, Result, StatusCode, format};
use matchbox_views::welcome;

use crate::app::AppCtx;
use crate::concerns::{self, Before};
use crate::controllers::presenters::page::framed_page;

/// To the last room visited, or a page saying there are no rooms yet.
pub async fn show(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    let user = concerns::require_current_user(c)?.clone();
    let user_id = user.id;
    let any_rooms = c.app().read(move |conn| Ok(!Room::for_user(conn, user_id)?.is_empty())).await?;
    if any_rooms {
        // `redirect_to room_url(last_room_visited)`
        let room = concerns::last_room_visited(c).await?.ok_or_else(|| Error::internal(anyhow::anyhow!("no last room")))?;
        let location = c.url_for(&matchbox_routes::room(room.id));
        return c.redirect_to(&location);
    }
    c.respond_to(&[&format::HTML])?;
    framed_page!(c, StatusCode::OK, |ctx| welcome::Show { ctx, current_user_name: user.name.clone() }).await
}
