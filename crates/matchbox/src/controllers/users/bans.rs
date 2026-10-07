//! `Users::BansController` (reference/app/controllers/users/bans_controller.rb).

use matchbox_kit::{Ctx, Result};

use super::find_user;
use crate::app::AppCtx;
use crate::concerns::{self, Before};

/// `before_action :ensure_can_administer, :set_user`; `@user.ban`
pub async fn create(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    concerns::ensure_can_administer(c)?;
    let mut user = find_user(c, "user_id").await?;
    let id = user.id;
    c.app().write(move |tx| user.ban(tx)).await?;
    redirect_to_user(c, id)
}

/// `@user.unban`
pub async fn destroy(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    concerns::ensure_can_administer(c)?;
    let mut user = find_user(c, "user_id").await?;
    let id = user.id;
    c.app().write(move |tx| user.unban(tx)).await?;
    redirect_to_user(c, id)
}

/// `redirect_to @user`
fn redirect_to_user(c: &mut Ctx, id: i64) -> Result {
    let location = c.url_for(&matchbox_routes::user(id));
    c.redirect_to(&location)
}
