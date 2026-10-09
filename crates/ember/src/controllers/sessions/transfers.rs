//! `Sessions::TransfersController` (reference/app/controllers/sessions/transfers_controller.rb):
//! sign in on another device with a user's transfer link.

use ember_db::User;
use ember_kit::{Ctx, Result, StatusCode, format};
use ember_views::sessions;

use crate::app::AppCtx;
use crate::concerns::{self, Before};
use crate::controllers::presenters;
use crate::controllers::presenters::page::framed_page;

/// `allow_unauthenticated_access`: an auto-submitting form that PUTs back to this URL.
pub async fn show(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default().allow_unauthenticated_access()).await?;
    c.respond_to(&[&format::HTML])?;
    // `url_for({})`: this request's own path.
    let action = c.request.path().to_string();
    framed_page!(c, StatusCode::OK, |ctx| sessions::TransferShow { ctx, action: action.clone() }).await
}

pub async fn update(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default().allow_unauthenticated_access()).await?;
    let transfer_id = c.param_str("id").unwrap_or_default().to_string();
    let user_id = presenters::accounts::user_id_from_transfer_id(&c.app().secrets, &transfer_id, c.now());
    // `User.active.find_by_transfer_id(params[:id])`
    let user = match user_id {
        Some(id) => c.app().read(move |conn| Ok(User::find_by_id(conn, id)?.filter(User::is_active))).await?,
        None => None,
    };
    match user {
        Some(user) => {
            concerns::start_new_session_for(c, user).await?;
            let location = concerns::post_authenticating_url(c);
            c.redirect_to(&location)
        }
        None => Ok(c.head(StatusCode::BAD_REQUEST)),
    }
}
