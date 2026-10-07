//! `Accounts::CustomStylesController` (reference/app/controllers/accounts/custom_styles_controller.rb).

use matchbox_kit::{Ctx, Param, Redirect, Result, StatusCode, format, permit_keys};
use matchbox_views::accounts;

use crate::app::AppCtx;
use crate::concerns::{self, Before};
use crate::controllers::presenters::page::framed_page;

/// `before_action :ensure_can_administer, :set_account`
pub async fn edit(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    concerns::ensure_can_administer(c)?;
    let account = super::current_account(c).await?;
    c.respond_to(&[&format::HTML])?;
    let custom_styles = account.custom_styles;
    framed_page!(c, StatusCode::OK, |ctx| accounts::CustomStylesEdit { ctx, custom_styles: custom_styles.clone() }).await
}

/// `@account.update!(params.require(:account).permit(:custom_styles))`
pub async fn update(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    concerns::ensure_can_administer(c)?;
    let mut account = super::current_account(c).await?;
    let params = c.params.require("account")?.permit(&permit_keys(&["custom_styles"]));
    let custom_styles = params.contains_key("custom_styles").then(|| params.get("custom_styles").and_then(Param::to_s));
    c.app().write(move |tx| account.update(tx, None, custom_styles.as_ref().map(|styles| styles.as_deref()), None)).await?;
    let location = c.url_for(&matchbox_routes::edit_account_custom_styles());
    c.redirect_to_with(&location, Redirect { notice: Some("✓".into()), ..Redirect::default() })
}
