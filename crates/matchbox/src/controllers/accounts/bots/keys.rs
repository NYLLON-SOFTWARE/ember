//! `Accounts::Bots::KeysController` (reference/app/controllers/accounts/bots/keys_controller.rb).

use matchbox_kit::{Ctx, Result};

use crate::app::AppCtx;
use crate::concerns::{self, Before};

/// `User.active_bots.find(params[:bot_id]).reset_bot_key`
pub async fn update(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    concerns::ensure_can_administer(c)?;
    let mut bot = super::find_active_bot(c, "bot_id").await?;
    c.app().write(move |tx| bot.reset_bot_key(tx)).await?;
    let location = c.url_for(&matchbox_routes::account_bots());
    c.redirect_to(&location)
}
