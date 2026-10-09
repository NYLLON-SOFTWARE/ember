//! `Accounts::JoinCodesController` (reference/app/controllers/accounts/join_codes_controller.rb).

use ember_kit::{Ctx, Result};

use crate::app::AppCtx;
use crate::concerns::{self, Before};

/// `Current.account.reset_join_code`
pub async fn create(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    concerns::ensure_can_administer(c)?;
    let mut account = super::current_account(c).await?;
    c.app().write(move |tx| account.reset_join_code(tx)).await?;
    let location = c.url_for(&ember_routes::edit_account());
    c.redirect_to(&location)
}
