//! `Rooms::InvolvementsController` (reference/app/controllers/rooms/involvements_controller.rb).

use askama::Template;
use ember_db::Involvement;
use ember_kit::{Ctx, Error, Result, StatusCode};
use ember_views::rooms::{InvolvementShow, InvolvementView};

use crate::app::AppCtx;
use crate::concerns::{self, Before, before_actions};
use crate::controllers::presenters::page;
use crate::controllers::presenters::room_kind;
use crate::controllers::rooms::render_shared_room;

pub async fn show(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let (membership, room) = concerns::set_room(c).await?;
    let involvement = InvolvementView {
        room_id: room.id,
        kind: room_kind(room.room_type),
        involvement: membership.involvement.map(|i| i.name().to_string()).unwrap_or_default(),
    };
    page::content(c, StatusCode::OK, |ctx| InvolvementShow { ctx, involvement: &involvement }.render()).await
}

pub async fn update(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let (membership, room) = concerns::set_room(c).await?;
    let involvement = involvement_param(c)?;
    let previous = membership.involvement;
    let membership = c
        .app()
        .write(move |tx| {
            let mut membership = membership;
            membership.update_involvement(tx, involvement)?;
            Ok(membership)
        })
        .await?;

    // broadcast_visibility_changes
    let partials = render_shared_room(c, &room).await?;
    c.app().broadcasts.involvement_change(&room, &membership, previous, &partials).map_err(Error::internal)?;

    let url = c.url_for(&ember_routes::room_involvement(room.id));
    c.redirect_to(&url)
}

/// `params[:involvement]` as the enum casts it: a blank value (missing, "", "  ", `[]`) is stored
/// as nil, anything that isn't one of the values raises ArgumentError ('... is not a valid
/// involvement'). Verified against the reference with `update!(involvement: "")`.
fn involvement_param(c: &Ctx) -> Result<Option<Involvement>> {
    let Some(param) = c.param("involvement").filter(|param| !param.is_blank()) else { return Ok(None) };
    param
        .as_str()
        .and_then(Involvement::from_name)
        .map(Some)
        .ok_or_else(|| Error::internal(anyhow::anyhow!("{:?} is not a valid involvement", param.to_s())))
}
