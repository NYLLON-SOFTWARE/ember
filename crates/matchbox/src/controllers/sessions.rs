//! `SessionsController` (reference/app/controllers/sessions_controller.rb): sign in and out.

pub mod transfers;

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use jiff::{SignedDuration, Timestamp};
use matchbox_db::PushSubscription;
use matchbox_kit::{Ctx, Result, StatusCode, format, halt};
use matchbox_views::sessions;

use super::presenters;
use crate::app::AppCtx;
use crate::concerns::{self, Before, current_user};
use crate::controllers::presenters::page::framed_page;

/// `rate_limit to: 10, within: 3.minutes, only: :create`
const RATE_LIMIT_TO: u64 = 10;
const RATE_LIMIT_WITHIN: SignedDuration = SignedDuration::from_mins(3);

const REJECTION: &str = "Too many requests or unauthorized.";

/// `allow_unauthenticated_access only: %i[ new create ]`, `before_action :ensure_user_exists, only: :new`
pub async fn new(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default().allow_unauthenticated_access()).await?;
    ensure_user_exists(c).await?;
    render_new(c, StatusCode::OK).await
}

pub async fn create(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default().allow_unauthenticated_access()).await?;
    rate_limit(c).await?;

    let email_address = c.param_str("email_address").map(str::to_string);
    let password = c.param_str("password").map(str::to_string);
    let user = match (email_address, password) {
        (Some(email_address), Some(password)) => concerns::authenticate_by(c, email_address, password).await?,
        _ => None,
    };

    match user {
        Some(user) => {
            concerns::start_new_session_for(c, user).await?;
            let location = concerns::post_authenticating_url(c);
            c.redirect_to(&location)
        }
        None => render_rejection(c, StatusCode::UNAUTHORIZED).await,
    }
}

pub async fn destroy(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    remove_push_subscription(c).await?;
    concerns::terminate_current_session(c).await?;
    let root = c.url_for(&matchbox_routes::root());
    c.redirect_to(&root)
}

/// `redirect_to first_run_url if User.none?`
async fn ensure_user_exists(c: &mut Ctx) -> Result<()> {
    let none = c.app().read(presenters::accounts::no_users).await?;
    if none {
        let first_run = c.url_for(&matchbox_routes::first_run());
        return halt(c.redirect_to(&first_run)?);
    }
    Ok(())
}

/// `flash.now[:alert] = "Too many requests or unauthorized."; render :new, status:`
async fn render_rejection(c: &mut Ctx, status: StatusCode) -> Result {
    c.flash().now("alert", REJECTION);
    render_new(c, status).await
}

async fn render_new(c: &mut Ctx, status: StatusCode) -> Result {
    c.respond_to(&[&format::HTML])?;
    let email_address = c.param_str("email_address").map(str::to_string);
    let help_contact = c.app().read(presenters::accounts::help_contact).await?;
    framed_page!(c, status, |ctx| sessions::New { ctx, email_address: email_address.clone(), help_contact: help_contact.clone() }).await
}

/// `Push::Subscription.destroy_by(endpoint: params[:push_subscription_endpoint], user_id: Current.user.id)`
async fn remove_push_subscription(c: &mut Ctx) -> Result<()> {
    let Some(endpoint) = c.param_str("push_subscription_endpoint").map(str::to_string) else { return Ok(()) };
    let Some(user_id) = current_user(c).map(|user| user.id) else { return Ok(()) };
    c.app().write(move |tx| PushSubscription::destroy_by_endpoint(tx, user_id, &endpoint)).await
}

// --- Rate limiting ---------------------------------------------------------------------------------

/// The Rails cache entries `rate_limit` counts in: `"rate-limit:sessions:#{request.remote_ip}"`,
/// incremented with `expires_in: within`, which (like Redis' `EXPIRE ... NX`) only sets the
/// expiry when the counter starts. One process holds them all, like one Redis would.
static RATE_LIMITS: LazyLock<Mutex<HashMap<String, (u64, Timestamp)>>> = LazyLock::new(Default::default);

/// `rate_limiting(to:, within:, by: -> { request.remote_ip }, with: -> { render_rejection :too_many_requests })`
async fn rate_limit(c: &mut Ctx) -> Result<()> {
    let key = format!("rate-limit:sessions:{}", c.request.remote_ip()?);
    if increment(&key, c.now()) > RATE_LIMIT_TO {
        return halt(render_rejection(c, StatusCode::TOO_MANY_REQUESTS).await?);
    }
    Ok(())
}

fn increment(key: &str, now: Timestamp) -> u64 {
    let mut limits = RATE_LIMITS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    limits.retain(|_, (_, expires_at)| *expires_at > now);
    let entry = limits.entry(key.to_string()).or_insert((0, now + RATE_LIMIT_WITHIN));
    entry.0 += 1;
    entry.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_within_a_fixed_window() {
        let start: Timestamp = "2024-06-01T12:00:00Z".parse().unwrap();
        let key = "rate-limit:sessions:test-window";
        for expected in 1..=11 {
            assert_eq!(increment(key, start + SignedDuration::from_secs(expected as i64)), expected);
        }
        // The window started at the first hit and doesn't slide.
        assert_eq!(increment(key, start + SignedDuration::from_secs(181)), 1);
    }
}
