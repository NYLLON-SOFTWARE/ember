//! `reference/app/models/push/subscription.rb`, and the subscriber queries of
//! `reference/app/models/room/message_pusher.rb`. Delivery lives in the app.

use rusqlite::{Connection, Row, params};

use crate::database::Tx;
use crate::error::{Errors, OptionalExt, Result};
use crate::models::{Membership, Message, Room, User};
use crate::rich_text::RichText;
use crate::sql::{self, CachedStatements, placeholders, query_all, query_one};
use crate::time::Timestamp;

/// `Push::Subscription::PERMITTED_ENDPOINT_HOSTS`
pub const PERMITTED_ENDPOINT_HOSTS: &[&str] =
    &["jmt17.google.com", "fcm.googleapis.com", "updates.push.services.mozilla.com", "web.push.apple.com", "notify.windows.com"];

#[derive(Debug, Clone, PartialEq)]
pub struct PushSubscription {
    pub id: i64,
    pub user_id: i64,
    pub endpoint: Option<String>,
    pub p256dh_key: Option<String>,
    pub auth_key: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// How long a payload's title and body may be, counted as the bytes they take in the JSON
/// message. An encrypted Web Push record holds at most 4096 bytes: 4078 of JSON once the padding
/// and tag are in, and the icon, path and badge take under 150 of that.
pub const MAX_PAYLOAD_TITLE_BYTES: usize = 256;
pub const MAX_PAYLOAD_BODY_BYTES: usize = 3072;

/// What `Room::MessagePusher#build_payload` sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushPayload {
    pub title: String,
    pub body: String,
    pub path: String,
}

impl PushSubscription {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            user_id: row.get("user_id")?,
            endpoint: row.get("endpoint")?,
            p256dh_key: row.get("p256dh_key")?,
            auth_key: row.get("auth_key")?,
            user_agent: row.get("user_agent")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }

    /// An unsaved subscription, for validation.
    pub fn new(user_id: i64, endpoint: Option<&str>, p256dh_key: Option<&str>, auth_key: Option<&str>, user_agent: Option<&str>) -> Self {
        Self {
            id: 0,
            user_id,
            endpoint: endpoint.map(Into::into),
            p256dh_key: p256dh_key.map(Into::into),
            auth_key: auth_key.map(Into::into),
            user_agent: user_agent.map(Into::into),
            created_at: Timestamp::from_second(0),
            updated_at: Timestamp::from_second(0),
        }
    }

    pub fn find(conn: &Connection, id: i64) -> Result<Self> {
        query_one(conn, r#"SELECT * FROM "push_subscriptions" WHERE "push_subscriptions"."id" = ? LIMIT 1"#, [id], Self::from_row)?
            .or_not_found("Push::Subscription")
    }

    pub fn count(conn: &Connection) -> Result<i64> {
        sql::count(conn, r#"SELECT COUNT(*) FROM "push_subscriptions""#, [])
    }

    pub fn for_user(conn: &Connection, user_id: i64) -> Result<Vec<Self>> {
        query_all(conn, r#"SELECT * FROM "push_subscriptions" WHERE "push_subscriptions"."user_id" = ?"#, [user_id], Self::from_row)
    }

    /// `user.push_subscriptions.find_by(endpoint:, p256dh_key:, auth_key:)`
    pub fn find_for_user_by_keys(
        conn: &Connection,
        user_id: i64,
        endpoint: &str,
        p256dh_key: &str,
        auth_key: &str,
    ) -> Result<Option<Self>> {
        query_one(
            conn,
            r#"SELECT * FROM "push_subscriptions" WHERE "push_subscriptions"."user_id" = ? AND "push_subscriptions"."endpoint" = ? AND "push_subscriptions"."p256dh_key" = ? AND "push_subscriptions"."auth_key" = ? LIMIT 1"#,
            params![user_id, endpoint, p256dh_key, auth_key],
            Self::from_row,
        )
    }

    /// `create`: validates (see [`PushSubscription::validate`]) then inserts.
    pub fn create(tx: &mut Tx<'_>, subscription: &PushSubscription, resolve: &dyn Fn(&str) -> Option<String>) -> Result<Self> {
        subscription.validate(resolve).into_result()?;
        let now = tx.now();
        let id: i64 = tx.conn().query_row_cached(
            r#"INSERT INTO "push_subscriptions" ("auth_key", "created_at", "endpoint", "p256dh_key", "updated_at", "user_agent", "user_id") VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING "id""#,
            params![subscription.auth_key, now, subscription.endpoint, subscription.p256dh_key, now, subscription.user_agent, subscription.user_id],
            |r| r.get(0),
        )?;
        Ok(Self { id, created_at: now, updated_at: now, ..subscription.clone() })
    }

    pub fn destroy(&self, tx: &mut Tx<'_>) -> Result<()> {
        tx.conn().execute_cached(r#"DELETE FROM "push_subscriptions" WHERE "push_subscriptions"."id" = ?"#, [self.id])?;
        Ok(())
    }

    /// `Push::Subscription.destroy_by(endpoint:, user_id:)`
    pub fn destroy_by_endpoint(tx: &mut Tx<'_>, user_id: i64, endpoint: &str) -> Result<()> {
        let ids: Vec<i64> = query_all(
            tx.conn(),
            r#"SELECT "push_subscriptions"."id" FROM "push_subscriptions" WHERE "push_subscriptions"."endpoint" = ? AND "push_subscriptions"."user_id" = ?"#,
            params![endpoint, user_id],
            |r| r.get(0),
        )?;
        for id in ids {
            tx.conn().execute_cached(r#"DELETE FROM "push_subscriptions" WHERE "push_subscriptions"."id" = ?"#, [id])?;
        }
        Ok(())
    }

    /// The badge for a notification: `user.memberships.unread.count`.
    pub fn badge(&self, conn: &Connection) -> Result<i64> {
        Membership::unread_count(conn, self.user_id)
    }

    /// Validations: endpoint present, then `validate_endpoint_url`. `resolve` is
    /// `RestrictedHTTP::PrivateNetworkGuard.resolve` (None for private or unresolvable).
    pub fn validate(&self, resolve: &dyn Fn(&str) -> Option<String>) -> Errors {
        let mut errors = Errors::default();
        let endpoint = self.endpoint.as_deref().unwrap_or("");
        if endpoint.trim().is_empty() {
            errors.add("endpoint", "can't be blank");
        }
        match EndpointUri::parse(endpoint) {
            None => errors.add("endpoint", "is not a valid URL"),
            Some(uri) if uri.scheme != "https" => errors.add("endpoint", "must use HTTPS"),
            Some(uri) if uri.port != Some(443) => errors.add("endpoint", "must use the default HTTPS port"),
            Some(uri) if !permitted_endpoint_host(&uri.host) => errors.add("endpoint", "is not a permitted push service"),
            Some(_) if self.resolved_endpoint_ip(resolve).is_none() => {
                errors.add("endpoint", "resolves to a private or invalid IP address")
            }
            Some(_) => {}
        }
        errors
    }

    /// `resolved_endpoint_ip`: only for a permitted https:443 endpoint.
    pub fn resolved_endpoint_ip(&self, resolve: &dyn Fn(&str) -> Option<String>) -> Option<String> {
        let uri = EndpointUri::parse(self.endpoint.as_deref()?)?;
        if uri.scheme == "https" && uri.port == Some(443) && permitted_endpoint_host(&uri.host) { resolve(&uri.host) } else { None }
    }

    // Room::MessagePusher

    /// `build_payload`: direct rooms show the sender; others the room and "Sender: body".
    /// Unlike Rails, a long title or body is cut short (with an ellipsis) so the notification
    /// still fits a push message.
    pub fn payload_for(conn: &Connection, rich_text: &dyn RichText, room: &Room, message: &Message) -> Result<PushPayload> {
        let creator = message.creator(conn)?;
        let body = message.plain_text_body(conn, rich_text)?;
        let path = format!("/rooms/{}", room.id);
        let (title, body) =
            if room.direct() { (creator.name, body) } else { (room.name.clone().unwrap_or_default(), format!("{}: {body}", creator.name)) };
        Ok(PushPayload {
            title: truncate_json_string(title, MAX_PAYLOAD_TITLE_BYTES),
            body: truncate_json_string(body, MAX_PAYLOAD_BODY_BYTES),
            path,
        })
    }

    /// `push_subscriptions_for_users_involved_in_everything`. Bans retain subscriptions for
    /// unban, but only active users may receive future room content (unlike the pinned reference).
    pub fn for_users_involved_in_everything(conn: &Connection, room_id: i64, creator_id: i64, now: Timestamp) -> Result<Vec<Self>> {
        query_all(
            conn,
            r#"SELECT "push_subscriptions".* FROM "push_subscriptions" INNER JOIN "users" ON "users"."id" = "push_subscriptions"."user_id" INNER JOIN "memberships" ON "memberships"."user_id" = "users"."id" WHERE "users"."status" = 0 AND ("memberships"."connected_at" IS NULL OR "memberships"."connected_at" < ?) AND "memberships"."room_id" = ? AND "memberships"."user_id" != ? AND "memberships"."involvement" = 'everything'"#,
            params![Membership::connection_cutoff(now), room_id, creator_id],
            Self::from_row,
        )
    }

    /// `push_subscriptions_for_mentionable_users(message.mentionees)`
    pub fn for_mentioned_users(
        conn: &Connection,
        room_id: i64,
        creator_id: i64,
        mentionee_ids: &[i64],
        now: Timestamp,
    ) -> Result<Vec<Self>> {
        if mentionee_ids.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!(
            r#"SELECT "push_subscriptions".* FROM "push_subscriptions" INNER JOIN "users" ON "users"."id" = "push_subscriptions"."user_id" INNER JOIN "memberships" ON "memberships"."user_id" = "users"."id" WHERE "users"."status" = 0 AND ("memberships"."connected_at" IS NULL OR "memberships"."connected_at" < ?) AND "memberships"."room_id" = ? AND "memberships"."user_id" != ? AND "memberships"."involvement" = 'mentions' AND "push_subscriptions"."user_id" IN ({})"#,
            placeholders(mentionee_ids.len())
        );
        let mut values: Vec<rusqlite::types::Value> =
            vec![Membership::connection_cutoff(now).to_db().into(), room_id.into(), creator_id.into()];
        values.extend(mentionee_ids.iter().map(|id| rusqlite::types::Value::from(*id)));
        query_all(conn, &sql, rusqlite::params_from_iter(values), Self::from_row)
    }

    /// `Room::MessagePusher#push`: the payload, and the subscriptions it goes to (everything
    /// first, then mentions).
    pub fn pushes_for(
        conn: &Connection,
        rich_text: &dyn RichText,
        message: &Message,
        now: Timestamp,
    ) -> Result<(PushPayload, Vec<Self>, Vec<Self>)> {
        let room = Room::find(conn, message.room_id)?;
        let payload = Self::payload_for(conn, rich_text, &room, message)?;
        let everything = Self::for_users_involved_in_everything(conn, room.id, message.creator_id, now)?;
        let mentionee_ids: Vec<i64> = message.mentionees(conn, rich_text)?.iter().map(|u: &User| u.id).collect();
        let mentions = Self::for_mentioned_users(conn, room.id, message.creator_id, &mentionee_ids, now)?;
        Ok((payload, everything, mentions))
    }
}

/// `text`, cut short with an ellipsis if it takes more than `max_bytes` as a JSON string's
/// contents.
fn truncate_json_string(mut text: String, max_bytes: usize) -> String {
    const ELLIPSIS: char = '…';
    if text.chars().map(json_len).sum::<usize>() <= max_bytes {
        return text;
    }
    let mut used = ELLIPSIS.len_utf8();
    let cut = text
        .char_indices()
        .find(|(_, c)| {
            used += json_len(*c);
            used > max_bytes
        })
        .map_or(text.len(), |(index, _)| index);
    text.truncate(cut);
    text.push(ELLIPSIS);
    text
}

/// The bytes `c` takes in a JSON string: `"`, `\` and the short escapes take two, other
/// control characters six (`\u001f`).
fn json_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if c < ' ' => 6,
        c => c.len_utf8(),
    }
}

fn permitted_endpoint_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    !host.is_empty() && PERMITTED_ENDPOINT_HOSTS.iter().any(|permitted| host == *permitted || host.ends_with(&format!(".{permitted}")))
}

/// The parts of `URI.parse(endpoint)` the validation looks at.
struct EndpointUri {
    scheme: String,
    host: String,
    port: Option<u16>,
}

impl EndpointUri {
    fn parse(endpoint: &str) -> Option<Self> {
        if endpoint.is_empty() || endpoint.chars().any(|c| c.is_whitespace()) {
            return None;
        }
        let (scheme, rest) = endpoint.split_once("://")?;
        let scheme = scheme.to_ascii_lowercase();
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let authority = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
        let (host, port) = match authority.rsplit_once(':') {
            _ if authority.ends_with(']') => (authority.to_string(), None),
            Some((host, port)) => (host.to_string(), Some(port.parse::<u16>().ok()?)),
            None => (authority.to_string(), None),
        };
        let default_port = match scheme.as_str() {
            "https" => Some(443),
            "http" => Some(80),
            _ => None,
        };
        Some(Self { scheme, host, port: port.or(default_port) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_by_json_escaped_length() {
        assert_eq!(truncate_json_string("short".into(), 5), "short");
        assert_eq!(truncate_json_string("longer".into(), 5), "lo…");
        assert_eq!(truncate_json_string("\u{1}".repeat(10), 16), format!("{}…", "\u{1}".repeat(2)));
        assert_eq!(truncate_json_string(r#"""""#.into(), 5), "\"…");
        assert_eq!(truncate_json_string("😀😀".into(), 7), "😀…");
    }
}
