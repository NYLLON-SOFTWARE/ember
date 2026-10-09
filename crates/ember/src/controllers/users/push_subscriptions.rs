//! `Users::PushSubscriptionsController` (reference/app/controllers/users/push_subscriptions_controller.rb):
//! the signed-in user's Web Push subscriptions.

pub mod test_notifications;

use std::collections::HashMap;
use std::sync::Mutex;

use ember_db::{CachedStatements, Connection, PushSubscription};
use ember_kit::{Ctx, Error, ParamMap, Result, StatusCode, format, permit_keys};
use ember_views::users;
use ruby_compat::integer_cast;
use rusqlite::OptionalExtension;
use rusqlite::types::Value;

use crate::app::AppCtx;
use crate::concerns::{self, Before};
use crate::controllers::presenters;
use crate::controllers::presenters::page::framed_page;
use crate::integrations::net::{Network, guard};

pub async fn index(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    c.respond_to(&[&format::HTML])?;
    let user_id = concerns::require_current_user(c)?.id;
    let subscriptions = c.app().read(move |conn| PushSubscription::for_user(conn, user_id)).await?;
    let push_subscriptions: Vec<_> = subscriptions.iter().map(presenters::accounts::push_subscription).collect();
    framed_page!(c, StatusCode::OK, |ctx| users::PushSubscriptionsIndex { ctx, push_subscriptions: push_subscriptions.clone() }).await
}

pub async fn create(c: &mut Ctx) -> Result {
    c.wrap_parameters("push_subscription", None);
    concerns::before_actions(c, Before::default()).await?;
    let user_id = concerns::require_current_user(c)?.id;
    let params = push_subscription_params(c)?;

    let existing = {
        let params = params.clone();
        c.app().read(move |conn| find_by(conn, user_id, &params)).await?
    };
    match existing {
        // Existing endpoints must pass current validations
        Some(subscription) => {
            if validate(&subscription).await.is_empty() {
                let id = subscription.id;
                c.app().write(move |tx| presenters::accounts::touch(tx.conn(), "push_subscriptions", id, tx.now())).await?;
                Ok(c.head(StatusCode::OK))
            } else {
                Ok(c.head(StatusCode::UNPROCESSABLE_ENTITY))
            }
        }
        None => {
            let value = |key: &str| params.get(key).and_then(|p| p.to_s());
            let subscription = PushSubscription::new(
                user_id,
                value("endpoint").as_deref(),
                value("p256dh_key").as_deref(),
                value("auth_key").as_deref(),
                c.request.user_agent(),
            );
            let resolved = resolve_endpoint(&subscription).await;
            let result = c
                .app()
                .db
                .write(move |tx| PushSubscription::create(tx, &subscription, &|host| resolved.get(host).cloned().flatten()))
                .await;
            match result {
                Ok(_) => Ok(c.head(StatusCode::OK)),
                Err(ember_db::Error::RecordInvalid(_)) => Ok(c.head(StatusCode::UNPROCESSABLE_ENTITY)),
                Err(error) => Err(Error::internal(error)),
            }
        }
    }
}

/// `@push_subscriptions.destroy_by(id: params[:id])`
pub async fn destroy(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    let user_id = concerns::require_current_user(c)?.id;
    if let Some(id) = c.param_str("id").and_then(integer_cast) {
        c.app()
            .write(move |tx| match PushSubscription::find(tx.conn(), id) {
                Ok(subscription) if subscription.user_id == user_id => subscription.destroy(tx),
                Ok(_) | Err(ember_db::Error::RecordNotFound(_)) => Ok(()),
                Err(error) => Err(error),
            })
            .await?;
    }
    let location = c.url_for(&ember_routes::user_push_subscriptions());
    c.redirect_to(&location)
}

/// `params.require(:push_subscription).permit(:endpoint, :p256dh_key, :auth_key)`
fn push_subscription_params(c: &Ctx) -> Result<ParamMap> {
    Ok(c.params.require("push_subscription")?.permit(&permit_keys(&["endpoint", "p256dh_key", "auth_key"])))
}

/// `Current.user.push_subscriptions.find_by(push_subscription_params)`: only the given keys are
/// conditions (none at all finds the user's first subscription); nil is `IS NULL`.
fn find_by(conn: &Connection, user_id: i64, params: &ParamMap) -> ember_db::Result<Option<PushSubscription>> {
    let mut sql = String::from(r#"SELECT "push_subscriptions"."id" FROM "push_subscriptions" WHERE "push_subscriptions"."user_id" = ?"#);
    let mut values = vec![Value::Integer(user_id)];
    for (key, param) in params.iter() {
        match param.to_s() {
            Some(value) => {
                sql.push_str(&format!(r#" AND "push_subscriptions"."{key}" = ?"#));
                values.push(Value::Text(value));
            }
            None => sql.push_str(&format!(r#" AND "push_subscriptions"."{key}" IS NULL"#)),
        }
    }
    sql.push_str(" LIMIT 1");
    let id: Option<i64> = conn.query_row_cached(&sql, rusqlite::params_from_iter(values), |row| row.get(0)).optional()?;
    id.map(|id| PushSubscription::find(conn, id)).transpose()
}

/// `subscription.valid?`
async fn validate(subscription: &PushSubscription) -> ember_db::Errors {
    let resolved = resolve_endpoint(subscription).await;
    subscription.validate(&|host| resolved.get(host).cloned().flatten())
}

/// `RestrictedHTTP::PrivateNetworkGuard.resolve(endpoint_uri.host)`, done ahead of the (synchronous)
/// validation for the one host it asks about: a first pass records the host, then it's resolved.
async fn resolve_endpoint(subscription: &PushSubscription) -> HashMap<String, Option<String>> {
    let asked = Mutex::new(None);
    subscription.validate(&|host| {
        *asked.lock().unwrap() = Some(host.to_string());
        None
    });
    let mut resolved = HashMap::new();
    if let Some(host) = asked.into_inner().unwrap() {
        let network = Network::system();
        let ip = guard::resolve(&*network.resolver, &host).await.ok().map(|ip| ip.to_string());
        resolved.insert(host, ip);
    }
    resolved
}
