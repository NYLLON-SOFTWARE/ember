//! `Accounts::LogosController` (reference/app/controllers/accounts/logos_controller.rb): the
//! account logo (or the stock app icon) as a PNG, public so it can be the PWA icon.

use matchbox_db::Account;
use matchbox_kit::{Ctx, ExpiresIn, Freshness, Result, SendOptions};
use matchbox_storage::Variation;

use crate::app::AppCtx;
use crate::concerns::{self, Before};
use crate::controllers::presenters::attachments::{self, Record};
use crate::controllers::presenters::cache_key_with_version;
use crate::controllers::users::avatars::asset_file;

/// `expires_in 5.minutes, public: true, stale_while_revalidate: 1.week`
const MAX_AGE: u64 = 5 * 60;
const STALE_WHILE_REVALIDATE: u64 = 7 * 24 * 60 * 60;

/// `allow_unauthenticated_access only: :show`
pub async fn show(c: &mut Ctx) -> Result {
    c.use_live_response(); // `include ActiveStorage::Streaming`
    concerns::before_actions(c, Before::default().allow_unauthenticated_access()).await?;
    let account = c.app().read(Account::first).await?;

    // `stale?(etag: Current.account)`; there's no accounts/logos/show template to digest.
    let freshness = Freshness {
        etag: account.as_ref().map(|account| cache_key_with_version("accounts", account.id, account.updated_at.jiff())),
        ..Freshness::default()
    };
    if let Some(not_modified) = c.fresh_when(freshness) {
        return Ok(not_modified);
    }
    c.expires_in(MAX_AGE, ExpiresIn { public: true, stale_while_revalidate: Some(STALE_WHILE_REVALIDATE), ..ExpiresIn::default() });

    let small = c.param_str("size") == Some("small");
    let variant = match &account {
        // `logo.variant(size).processed if logo.variable?`: :small is 192, :large 512, both PNG.
        Some(account) => {
            let size = if small { 192 } else { 512 };
            attachments::processed_variant(
                c.app(),
                Record::account(account.id),
                "logo",
                Variation::resize_to_limit(size, size, Some("png")),
            )
            .await?
        }
        None => None,
    };
    match variant {
        Some(variant) => {
            let path = c.app().storage.service.path_for(&variant.key);
            c.send_file(path, SendOptions::inline("image/png"))
        }
        // send_stock_icon
        None => {
            let filename = if small { "app-icon-192.png" } else { "app-icon.png" };
            let path = asset_file(&format!("logos/{filename}"))?;
            c.send_file(path, SendOptions::inline("image/png"))
        }
    }
}

/// `Current.account.logo.destroy`
pub async fn destroy(c: &mut Ctx) -> Result {
    c.use_live_response(); // `include ActiveStorage::Streaming`
    concerns::before_actions(c, Before::default()).await?;
    concerns::ensure_can_administer(c)?;
    let account = super::current_account(c).await?;
    c.app().write(move |tx| attachments::destroy(tx, Record::account(account.id), "logo")).await?;
    let location = c.url_for(&matchbox_routes::edit_account());
    c.redirect_to(&location)
}
