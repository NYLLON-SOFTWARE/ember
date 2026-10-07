//! `Users::PushSubscriptions::TestNotificationsController`
//! (reference/app/controllers/users/push_subscriptions/test_notifications_controller.rb).

use matchbox_db::{Membership, PushSubscription};
use matchbox_kit::{Ctx, Error, Result};
use ruby_compat::integer_cast;

use crate::app::AppCtx;
use crate::concerns::{self, Before};
use crate::integrations::net::Network;
use crate::integrations::web_push;

/// `@push_subscription.notification(title: "Matchbox Test", body: Random.uuid, path: user_push_subscriptions_url).deliver`
pub async fn create(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    let user_id = concerns::require_current_user(c)?.id;
    let id = c.param_str("push_subscription_id").and_then(integer_cast).ok_or(Error::NotFound)?;
    let (subscription, badge) = c
        .app()
        .read(move |conn| {
            // `Current.user.push_subscriptions.find(params[:push_subscription_id])`
            let subscription = PushSubscription::find(conn, id)?;
            if subscription.user_id != user_id {
                return Err(matchbox_db::Error::RecordNotFound("Push::Subscription"));
            }
            Ok((subscription, Membership::unread_count(conn, user_id)?))
        })
        .await?;

    let location = c.url_for(&matchbox_routes::user_push_subscriptions());
    let web_push = c.app().web_push.as_ref().ok_or_else(|| Error::internal(anyhow::anyhow!("Web Push is off (no valid VAPID keys)")))?;
    web_push::deliver_test_notification(&Network::system(), web_push.vapid(), &subscription, badge, &location)
        .await
        .map_err(|error| Error::internal(anyhow::anyhow!("{error:?}")))?;
    c.redirect_to(&location)
}
