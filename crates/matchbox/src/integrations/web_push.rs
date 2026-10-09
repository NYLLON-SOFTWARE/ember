//! Web Push: `WebPush::Notification` and `WebPush::Pool` (reference/lib/web_push), the gem
//! request they make (web-push 3.1.0 with the `WebPush::PersistentRequest` patch in
//! reference/config/initializers/web_push.rb), and `Room::MessagePusher#push`.
//!
//! Delivery resolves the endpoint on the worker, only for a permitted `https://…:443` push
//! service, through the private network guard, and connects to that address with no proxy.

pub mod encryption;
pub mod pool;
mod vapid;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use matchbox_db::{Connection, Message, PushPayload, PushSubscription, RichText, Timestamp};
use rails_compat::encoding;

use crate::integrations::net::http::{self, Endpoint, HttpError, Timeouts};
use crate::integrations::net::{Network, guard};

pub use encryption::EncryptionError;
pub use pool::Pool;
pub use vapid::{VapidConfig, VapidError};

/// `WebPush::Request#default_options[:ttl]`: four weeks.
const TTL_SECONDS: u64 = 60 * 60 * 24 * 7 * 4;
const URGENCY: &str = "high";
/// Per connect and read. The web-push gem leaves `Net::HTTP`'s 60 seconds, which let a slow push
/// service hold one of the pool's few workers for minutes.
const TIMEOUTS: Timeouts = Timeouts { open: Duration::from_secs(10), read: Duration::from_secs(10) };
/// For a whole delivery, however the push service trickles its reply.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(30);
/// `Rails.application.routes.url_helpers.account_logo_path`
const ICON_PATH: &str = "/account/logo";

/// `Push::Subscription#notification(title:, body:, path:)`: built on the enqueuing side, with
/// the badge counted there. The endpoint is resolved only when it's delivered.
#[derive(Debug, Clone)]
pub struct Notification {
    pub title: String,
    pub body: String,
    pub path: String,
    pub badge: i64,
    pub subscription: PushSubscription,
}

impl Notification {
    /// `subscription.notification(**payload)`: the badge is `user.memberships.unread.count`.
    pub fn build(conn: &Connection, subscription: &PushSubscription, payload: &PushPayload) -> matchbox_db::Result<Self> {
        Ok(Self {
            title: payload.title.clone(),
            body: payload.body.clone(),
            path: payload.path.clone(),
            badge: subscription.badge(conn)?,
            subscription: subscription.clone(),
        })
    }

    /// `encoded_message`: `JSON.generate` (plain JSON, no HTML escaping).
    pub fn encoded_message(&self) -> String {
        serde_json::json!({
            "title": self.title,
            "options": { "body": self.body, "icon": ICON_PATH, "data": { "path": self.path, "badge": self.badge } }
        })
        .to_string()
    }

    /// `WebPush::Notification#deliver`: nothing happens when the endpoint isn't a permitted push
    /// service or doesn't resolve to a public address (`Ok(None)`); otherwise the push
    /// service's status.
    pub async fn deliver(&self, net: &Network, vapid: &VapidConfig) -> Result<Option<u16>, DeliveryError> {
        let Some(endpoint_ip) = self.resolved_endpoint_ip(net).await else { return Ok(None) };
        let endpoint = self.subscription.endpoint.as_deref().unwrap_or_default();
        payload_send(net, vapid, endpoint, endpoint_ip, &self.subscription, self.encoded_message().as_bytes(), unix_now()).await.map(Some)
    }

    /// `Push::Subscription#resolved_endpoint_ip`
    async fn resolved_endpoint_ip(&self, net: &Network) -> Option<std::net::IpAddr> {
        let host = self.subscription.resolved_endpoint_ip(&|host| Some(host.to_string()))?;
        guard::resolve(net.resolver.as_ref(), &host).await.ok()
    }
}

/// What `WebPush.payload_send` raised.
#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    /// `WebPush::ExpiredSubscription` (410) and `WebPush::InvalidSubscription` (404): the push
    /// service doesn't know the subscription (any more).
    #[error("{kind}: host: {host}, status: {status}")]
    SubscriptionGone { kind: &'static str, host: String, status: u16 },
    /// The gem's other `ResponseError`s: 401/403 and 400 "UnauthorizedRegistration"
    /// (`Unauthorized`), 413, 429, 5xx, and any other non-2xx.
    #[error("{kind}: host: {host}, status: {status}")]
    Response { kind: &'static str, host: String, status: u16 },
    /// `OpenSSL::PKey::EC::Point::Error`: the subscription's key isn't a P-256 point.
    #[error("{0}")]
    InvalidSubscriptionKey(String),
    /// `ArgumentError`: blank or malformed keys, an oversized payload.
    #[error("{0}")]
    Argument(String),
    /// `OpenSSL::SSL::SSLError`: a failed TLS session, which may well be our side's fault (a
    /// clock or CA store problem).
    #[error("{0}")]
    Tls(String),
    /// Connection failures and timeouts.
    #[error("{0}")]
    Http(HttpError),
}

impl DeliveryError {
    /// Whether the subscription can never be delivered to, so the pool destroys it.
    /// `WebPush::Pool#deliver` also destroys it for a 410 and any `OpenSSL::OpenSSLError`,
    /// which includes TLS failures and a bad VAPID key; those say nothing about the
    /// subscription, and a 404 does (RFC 8030, section 7.3).
    pub fn invalidates_subscription(&self) -> bool {
        matches!(self, DeliveryError::SubscriptionGone { .. } | DeliveryError::InvalidSubscriptionKey(_))
    }

    /// The Ruby exception class, for the pool's log line.
    pub fn class_name(&self) -> &'static str {
        match self {
            DeliveryError::SubscriptionGone { kind, .. } | DeliveryError::Response { kind, .. } => kind,
            DeliveryError::InvalidSubscriptionKey(_) => "OpenSSL::PKey::EC::Point::Error",
            DeliveryError::Argument(_) => "ArgumentError",
            DeliveryError::Tls(_) => "OpenSSL::SSL::SSLError",
            DeliveryError::Http(HttpError::OpenTimeout) => "Net::OpenTimeout",
            DeliveryError::Http(HttpError::ReadTimeout) => "Net::ReadTimeout",
            DeliveryError::Http(_) => "SystemCallError",
        }
    }
}

impl From<EncryptionError> for DeliveryError {
    fn from(error: EncryptionError) -> Self {
        match error {
            EncryptionError::Argument(message) => DeliveryError::Argument(message),
            EncryptionError::InvalidKey(message) => DeliveryError::InvalidSubscriptionKey(message),
        }
    }
}

impl From<HttpError> for DeliveryError {
    fn from(error: HttpError) -> Self {
        match error {
            HttpError::Tls(message) => DeliveryError::Tls(message),
            other => DeliveryError::Http(other),
        }
    }
}

/// `WebPush.payload_send(message:, endpoint:, endpoint_ip:, p256dh:, auth:, vapid:,
/// urgency: "high")` through the pinned-address branch of `WebPush::PersistentRequest`.
async fn payload_send(
    net: &Network,
    vapid: &VapidConfig,
    endpoint: &str,
    endpoint_ip: std::net::IpAddr,
    subscription: &PushSubscription,
    message: &[u8],
    now: i64,
) -> Result<u16, DeliveryError> {
    let uri =
        matchbox_richtext::uri::parse(endpoint).map_err(|_| DeliveryError::Argument(format!("bad URI(is not URI?): {endpoint:?}")))?;
    let host = uri.host.clone().unwrap_or_default();
    let payload = encryption::encrypt(message, subscription.p256dh_key.as_deref(), subscription.auth_key.as_deref())?;

    let mut headers = vec![
        ("Content-Type".to_string(), "application/octet-stream".to_string()),
        ("Ttl".to_string(), TTL_SECONDS.to_string()),
        ("Urgency".to_string(), URGENCY.to_string()),
        ("Content-Encoding".to_string(), "aes128gcm".to_string()),
        ("Content-Length".to_string(), payload.len().to_string()),
    ];
    let audience = format!("{}://{}", uri.scheme.as_deref().unwrap_or("").to_ascii_lowercase(), host);
    headers.push(("Authorization".to_string(), vapid.authorization(&audience, now)));

    let endpoint = Endpoint { https: true, host: host.clone(), port: uri.port.unwrap_or(443) as u16, pinned_ip: Some(endpoint_ip) };
    let mut request = http::Request::net_http(hyper::Method::POST, http::request_uri(&uri), None, headers).transport(true, &endpoint);
    request.body = payload;
    let response = tokio::time::timeout(DELIVERY_DEADLINE, http::exchange(net, &endpoint, request, &TIMEOUTS))
        .await
        .map_err(|_| HttpError::ReadTimeout)??;
    verify_response(response.status, &response.reason, &host)
}

/// `WebPush::Request#verify_response`
fn verify_response(status: u16, reason: &str, host: &str) -> Result<u16, DeliveryError> {
    let error = |kind| Err(DeliveryError::Response { kind, host: host.to_string(), status });
    let gone = |kind| Err(DeliveryError::SubscriptionGone { kind, host: host.to_string(), status });
    match status {
        410 => gone("WebPush::ExpiredSubscription"),
        404 => gone("WebPush::InvalidSubscription"),
        401 | 403 => error("WebPush::Unauthorized"),
        400 if reason == "UnauthorizedRegistration" => error("WebPush::Unauthorized"),
        413 => error("WebPush::PayloadTooLarge"),
        429 => error("WebPush::TooManyRequests"),
        500..=599 => error("WebPush::PushServiceError"),
        200..=299 => Ok(status),
        _ => error("WebPush::ResponseError"),
    }
}

/// `Users::PushSubscriptions::TestNotificationsController#create`: a "Ember Test"
/// notification with a random body, delivered inline. `path` is `user_push_subscriptions_url`
/// (a full URL); `badge` is the subscriber's unread count. Errors propagate, as in Rails.
pub async fn deliver_test_notification(
    net: &Network,
    vapid: &VapidConfig,
    subscription: &PushSubscription,
    badge: i64,
    path: &str,
) -> Result<(), DeliveryError> {
    let notification = Notification {
        title: "Ember Test".into(),
        body: uuid::Uuid::new_v4().to_string(),
        path: path.to_string(),
        badge,
        subscription: subscription.clone(),
    };
    notification.deliver(net, vapid).await.map(|_| ())
}

/// `Room::PushMessageJob#perform` / `Room::MessagePusher#push`: the payload goes to the
/// subscriptions of everyone involved in everything, then to mentioned users involved in
/// mentions. Badges are counted here; delivery happens on the pool.
pub fn push_message(
    pool: &Pool,
    conn: &Connection,
    rich_text: &dyn RichText,
    message: &Message,
    now: Timestamp,
) -> matchbox_db::Result<PushPayload> {
    let (payload, everything, mentions) = PushSubscription::pushes_for(conn, rich_text, message, now)?;
    pool.queue(conn, &payload, everything)?;
    pool.queue(conn, &payload, mentions)?;
    Ok(payload)
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `WebPush.decode64` (`Base64.urlsafe_decode64`): either alphabet, padding optional.
pub(crate) fn decode64(value: &str) -> Result<Vec<u8>, EncryptionError> {
    encoding::urlsafe_decode(value).ok_or_else(|| EncryptionError::Argument("invalid base64".into()))
}

/// `trim_encode64`: urlsafe Base64 without padding.
pub(crate) fn encode64_nopad(bytes: &[u8]) -> String {
    encoding::urlsafe_encode_unpadded(bytes)
}

#[cfg(test)]
mod tests;
