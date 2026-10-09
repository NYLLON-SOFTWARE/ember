//! `Webhook#deliver` (reference/app/models/webhook.rb): a bot's webhook gets the message as
//! JSON, and its answer becomes the bot's reply.
//!
//! Intentionally unguarded, unlike Opengraph::Fetch: only an administrator sets this URL and it
//! may point at internal services. Connect and each read time out after 7 seconds, and a
//! timeout is itself answered with a text reply. Unlike Rails, the whole delivery must finish
//! within a minute and the reply is read up to 100 MB (see `DELIVERY_DEADLINE`, `MAX_REPLY_SIZE`).

use std::sync::LazyLock;
use std::time::Duration;

use ember_richtext::uri;
use ember_storage::filename::Filename;
use ember_storage::{Staged, Storage};
use regex::Regex;

use crate::integrations::net::Network;
use crate::integrations::net::http::{self, Body, Endpoint, HttpError, Timeouts};

/// `Webhook::ENDPOINT_TIMEOUT`
pub const ENDPOINT_TIMEOUT: Duration = Duration::from_secs(ember_db::models::webhook::ENDPOINT_TIMEOUT_SECONDS);

/// The most a delivery may take, connecting and reading the reply included. `ENDPOINT_TIMEOUT`
/// applies to each read, so an endpoint that keeps trickling bytes would otherwise hold a job
/// slot forever.
pub const DELIVERY_DEADLINE: Duration = Duration::from_secs(60);

/// The largest reply read (after decompression); a larger one fails the delivery. Rails reads
/// any size into memory.
pub const MAX_REPLY_SIZE: usize = 100 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookDelivery {
    /// The webhook's status, or `None` when it timed out.
    pub status: Option<u16>,
    pub reply: WebhookReply,
}

/// What the bot says back in the message's room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookReply {
    None,
    /// `room.messages.create!(body: text, creator: bot).broadcast_create`: the text is assigned
    /// as the message's rich text body, as a bot's posted body is.
    Text(String),
    /// `room.messages.create_with_attachment!(attachment: blob, creator: bot).broadcast_create`,
    /// with the blob from [`Attachment::stage_blob`].
    Attachment(Attachment),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub data: Vec<u8>,
    /// `"attachment.#{mime_type.symbol}"`: "attachment." for unregistered types.
    pub filename: String,
    /// `mime_type.to_s`: the registered type, which for a synonym differs from what was sent.
    pub content_type: String,
}

impl Attachment {
    /// The upload half of `ActiveStorage::Blob.create_and_upload!(io:, filename:, content_type:)`,
    /// blocking: the caller saves the staged blob's row.
    pub fn stage_blob(&self, storage: &Storage) -> ember_storage::Result<Staged> {
        storage.stage_bytes(&self.data, Filename::new(self.filename.clone()), Some(&self.content_type))
    }
}

/// Raised out of `deliver` (the job fails), as in Rails: a bad URL, a connection failure other
/// than a timeout, or a reply content type Rails can't parse.
#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    /// `URI::InvalidURIError`, or `ArgumentError` from `Net::HTTP::Post.new`.
    #[error("{0}")]
    InvalidUrl(String),
    #[error("{0}")]
    Http(HttpError),
    /// `Mime::Type::InvalidMimeType`
    #[error("{0:?} is not a valid MIME type")]
    InvalidMimeType(String),
    #[error("the reply is larger than {} MB", MAX_REPLY_SIZE / 1024 / 1024)]
    ReplyTooLarge,
}

/// `Webhook#deliver(message)`: `payload` is `ember_db::Webhook::payload`.
pub async fn deliver(net: &Network, url: &str, payload: String) -> Result<WebhookDelivery, WebhookError> {
    deliver_within(net, url, payload, DELIVERY_DEADLINE).await
}

async fn deliver_within(net: &Network, url: &str, payload: String, deadline: Duration) -> Result<WebhookDelivery, WebhookError> {
    match tokio::time::timeout(deadline, post(net, url, payload)).await {
        Ok(Ok((status, content_type, body))) => Ok(WebhookDelivery { status: Some(status), reply: reply(status, content_type, body)? }),
        Ok(Err(WebhookError::Http(HttpError::OpenTimeout | HttpError::ReadTimeout))) => Ok(timed_out(ENDPOINT_TIMEOUT)),
        Ok(Err(error)) => Err(error),
        Err(_) => Ok(timed_out(deadline)),
    }
}

fn timed_out(after: Duration) -> WebhookDelivery {
    WebhookDelivery { status: None, reply: WebhookReply::Text(format!("Failed to respond within {} seconds", after.as_secs())) }
}

/// `post(payload)` over `Net::HTTP.new(uri.host, uri.port)`: the status, content type and body.
async fn post(net: &Network, url: &str, payload: String) -> Result<(u16, Option<String>, Vec<u8>), WebhookError> {
    let uri = uri::parse(url).map_err(|_| WebhookError::InvalidUrl(format!("bad URI (is not URI?): {url:?}")))?;
    if !uri.is_http() {
        return Err(WebhookError::InvalidUrl("not an HTTP URI".into()));
    }
    let host = uri.host.clone().filter(|h| !h.is_empty()).ok_or_else(|| WebhookError::InvalidUrl("no host component for URI".into()))?;
    let https = uri.scheme.as_deref().is_some_and(|s| s.eq_ignore_ascii_case("https"));
    let port = uri.port.and_then(|p| u16::try_from(p).ok()).ok_or_else(|| WebhookError::InvalidUrl("invalid port".into()))?;
    let endpoint = Endpoint { https, host: host.clone(), port, pinned_ip: None };

    let hostname = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(&host);
    let uri_host = if port == if https { 443 } else { 80 } { hostname.to_string() } else { format!("{hostname}:{port}") };
    let headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    let mut request =
        http::Request::net_http(hyper::Method::POST, http::request_uri(&uri), Some(uri_host), headers).transport(true, &endpoint);
    request.body = payload.into_bytes();

    let timeouts = Timeouts { open: ENDPOINT_TIMEOUT, read: ENDPOINT_TIMEOUT };
    let response = http::exchange(net, &endpoint, request, &timeouts).await.map_err(WebhookError::Http)?;
    let (status, content_type) = (response.status, response.content_type());
    let body = match response.read_body(MAX_REPLY_SIZE).await.map_err(WebhookError::Http)? {
        Body::Complete(body) => body,
        Body::TooLarge => return Err(WebhookError::ReplyTooLarge),
    };
    Ok((status, content_type, body))
}

/// `extract_text_from`, else `extract_attachment_from`.
fn reply(status: u16, content_type: Option<String>, body: Vec<u8>) -> Result<WebhookReply, WebhookError> {
    let Some(content_type) = content_type else { return Ok(WebhookReply::None) };
    if status == 200 && (content_type == "text/html" || content_type == "text/plain") {
        return Ok(WebhookReply::Text(String::from_utf8_lossy(&body).into_owned()));
    }
    let (symbol, registered) = mime_lookup(&content_type)?;
    Ok(WebhookReply::Attachment(Attachment {
        data: body,
        filename: format!("attachment.{}", symbol.unwrap_or("")),
        content_type: registered,
    }))
}

/// `Mime::Type.lookup(string)`: a registered type (by its string or a synonym), else a new
/// unregistered type, which must be a valid MIME type.
fn mime_lookup(string: &str) -> Result<(Option<&'static str>, String), WebhookError> {
    let registered = |s: &str| MIME_LOOKUP.iter().find(|(key, _, _)| *key == s).map(|(_, symbol, to_s)| (Some(*symbol), to_s.to_string()));
    if let Some(found) = registered(string) {
        return Ok(found);
    }
    let string = string.split(';').next().unwrap_or("").trim_end_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r', '\0']);
    if let Some(found) = registered(string) {
        return Ok(found);
    }
    if MIME_REGEXP.is_match(string) { Ok((None, string.to_string())) } else { Err(WebhookError::InvalidMimeType(string.to_string())) }
}

/// `Mime::Type::MIME_REGEXP` (in the Ruby source, "\s" inside the double-quoted parameter
/// pattern is a literal space).
static MIME_REGEXP: LazyLock<Regex> = LazyLock::new(|| {
    let name = r"[a-zA-Z0-9][a-zA-Z0-9!#$&\-^_.+]{0,126}";
    let value = format!(r#"(?:{name}|"[^"\r\\]*")"#);
    let parameter = format!(r" *; *{name}(?:={value})?");
    Regex::new(&format!(r"\A(?:\*/\*|{name}/(?:\*|{name})(?:{parameter})*[ \t\n\x0B\x0C\r]*)\z")).unwrap()
});

/// `Mime::LOOKUP` in the reference app: Action Dispatch's registrations plus turbo-rails'.
const MIME_LOOKUP: &[(&str, &str, &str)] = &[
    ("text/html", "html", "text/html"),
    ("application/xhtml+xml", "html", "text/html"),
    ("text/plain", "text", "text/plain"),
    ("text/javascript", "js", "text/javascript"),
    ("application/javascript", "js", "text/javascript"),
    ("application/x-javascript", "js", "text/javascript"),
    ("text/css", "css", "text/css"),
    ("text/calendar", "ics", "text/calendar"),
    ("text/csv", "csv", "text/csv"),
    ("text/vcard", "vcf", "text/vcard"),
    ("text/vtt", "vtt", "text/vtt"),
    ("vtt", "vtt", "text/vtt"),
    ("text/markdown", "md", "text/markdown"),
    ("image/png", "png", "image/png"),
    ("image/jpeg", "jpeg", "image/jpeg"),
    ("image/gif", "gif", "image/gif"),
    ("image/bmp", "bmp", "image/bmp"),
    ("image/tiff", "tiff", "image/tiff"),
    ("image/svg+xml", "svg", "image/svg+xml"),
    ("image/webp", "webp", "image/webp"),
    ("video/mpeg", "mpeg", "video/mpeg"),
    ("audio/mpeg", "mp3", "audio/mpeg"),
    ("audio/ogg", "ogg", "audio/ogg"),
    ("audio/aac", "m4a", "audio/aac"),
    ("audio/mp4", "m4a", "audio/aac"),
    ("video/webm", "webm", "video/webm"),
    ("video/mp4", "mp4", "video/mp4"),
    ("font/otf", "otf", "font/otf"),
    ("font/ttf", "ttf", "font/ttf"),
    ("font/woff", "woff", "font/woff"),
    ("font/woff2", "woff2", "font/woff2"),
    ("application/xml", "xml", "application/xml"),
    ("text/xml", "xml", "application/xml"),
    ("application/x-xml", "xml", "application/xml"),
    ("application/rss+xml", "rss", "application/rss+xml"),
    ("application/atom+xml", "atom", "application/atom+xml"),
    ("application/x-yaml", "yaml", "application/x-yaml"),
    ("text/yaml", "yaml", "application/x-yaml"),
    ("multipart/form-data", "multipart_form", "multipart/form-data"),
    ("application/x-www-form-urlencoded", "url_encoded_form", "application/x-www-form-urlencoded"),
    ("application/json", "json", "application/json"),
    ("text/x-json", "json", "application/json"),
    ("application/jsonrequest", "json", "application/json"),
    ("application/problem+json", "json", "application/json"),
    ("application/pdf", "pdf", "application/pdf"),
    ("application/zip", "zip", "application/zip"),
    ("application/gzip", "gzip", "application/gzip"),
    ("application/x-gzip", "gzip", "application/gzip"),
    ("text/vnd.turbo-stream.html", "turbo_stream", "text/vnd.turbo-stream.html"),
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    use base64::Engine;
    use serde_json::Value;

    use super::*;
    use crate::integrations::test_support::{FakeResolver, FakeServer, MappingDialer, Route, gzip_bomb, network, trickling_server};

    fn b64(s: &str) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD.decode(s).unwrap()
    }

    /// Replays testdata/webhook_cases.json and compares with what `Webhook#deliver` did in the
    /// reference (testdata/webhook_expected.json, from testdata/oracle/webhook.rb).
    #[tokio::test(flavor = "multi_thread")]
    async fn delivers_like_the_reference() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("testdata/webhook_cases.json")).unwrap();
        let expected: Vec<Value> = serde_json::from_str(include_str!("testdata/webhook_expected.json")).unwrap();
        let routes = cases
            .iter()
            .map(|c| {
                let mut route = Route::new("POST", "*", &format!("/{}", c["name"].as_str().unwrap()), c["status"].as_u64().unwrap() as u16);
                route.headers = serde_json::from_value(c["headers"].clone()).unwrap();
                route.body = match c["body_b64"].as_str() {
                    Some(body) => b64(body),
                    None => c["body"].as_str().unwrap_or("").as_bytes().to_vec(),
                };
                route.gzip = c["gzip"].as_bool().unwrap_or(false);
                route.delay = Duration::from_secs(c["delay"].as_u64().unwrap_or(0));
                route
            })
            .collect();
        let server = FakeServer::start(routes).await;
        let net = crate::integrations::net::Network::system();

        let runs = cases.iter().map(|c| {
            let net = net.clone();
            let url =
                c["url"].as_str().map(str::to_string).unwrap_or_else(|| format!("http://{}/{}", server.addr, c["name"].as_str().unwrap()));
            async move { deliver(&net, &url, r#"{"message":"hi"}"#.to_string()).await }
        });
        let outcomes = futures_join_all(runs).await;

        for ((case, expected), outcome) in cases.iter().zip(&expected).zip(outcomes) {
            let name = case["name"].as_str().unwrap();
            let actual = match outcome {
                Ok(delivery) => {
                    let reply = match delivery.reply {
                        WebhookReply::None => Value::Null,
                        WebhookReply::Text(text) => serde_json::json!({ "text": text }),
                        WebhookReply::Attachment(a) => {
                            serde_json::json!({ "filename": a.filename, "content_type": a.content_type, "data": a.data })
                        }
                    };
                    serde_json::json!({ "status": delivery.status, "reply": reply })
                }
                Err(WebhookError::InvalidMimeType(_)) => serde_json::json!({ "error": "Mime::Type::InvalidMimeType", "reply": null }),
                Err(WebhookError::Http(HttpError::Io(e))) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                    serde_json::json!({ "error": "Errno::ECONNREFUSED", "reply": null })
                }
                Err(error) => serde_json::json!({ "error": error.to_string(), "reply": null }),
            };
            let reply = &expected["reply"];
            let wanted_reply = if reply.is_null() {
                Value::Null
            } else if let Some(attachment) = reply.get("attachment") {
                serde_json::json!({
                    "filename": attachment["filename"],
                    "content_type": attachment["content_type"],
                    "data": b64(attachment["body_b64"].as_str().unwrap()),
                })
            } else {
                serde_json::json!({ "text": String::from_utf8_lossy(&b64(reply["text_b64"].as_str().unwrap())) })
            };
            let wanted = match expected.get("error") {
                Some(error) => serde_json::json!({ "error": error, "reply": wanted_reply }),
                None => serde_json::json!({ "status": expected["status"], "reply": wanted_reply }),
            };
            assert_eq!(actual, wanted, "{name}");
        }

        // The request as Net::HTTP sends it
        let request = server.received().into_iter().find(|r| r.target == "/text").unwrap();
        let wanted: Vec<(String, String)> = serde_json::from_value(expected[0]["requests"][0]["headers"].clone()).unwrap();
        let wanted: Vec<(String, String)> =
            wanted.into_iter().map(|(n, v)| if n == "Host" { (n, server.addr.to_string()) } else { (n, v) }).collect();
        assert_eq!(request.headers, wanted);
        assert_eq!(request.body, expected[0]["requests"][0]["body"].as_str().unwrap().as_bytes());
    }

    async fn futures_join_all<F: std::future::Future + Send + 'static>(futures: impl Iterator<Item = F>) -> Vec<F::Output>
    where
        F::Output: Send + 'static,
    {
        let handles: Vec<_> = futures.map(tokio::spawn).collect();
        let mut out = Vec::new();
        for handle in handles {
            out.push(handle.await.unwrap());
        }
        out
    }

    /// Unguarded: private and loopback addresses are fine, names resolve normally, and nothing
    /// is pinned.
    #[tokio::test]
    async fn reaches_internal_services() {
        let server = FakeServer::start(vec![Route::new("POST", "*", "/hook", 200).header("Content-Type", "text/plain").body("ok")]).await;
        let resolver = Arc::new(FakeResolver::new([("bots.internal", vec!["10.0.0.7"])]));
        let dialer = Arc::new(MappingDialer {
            public: HashSet::from(["10.0.0.7".parse().unwrap()]),
            to: server.addr,
            dialed: Mutex::new(Vec::new()),
        });
        let net = network(resolver.clone(), dialer.clone());
        let delivery = deliver(&net, "http://bots.internal:8080/hook", "{}".into()).await.unwrap();
        assert_eq!(delivery, WebhookDelivery { status: Some(200), reply: WebhookReply::Text("ok".into()) });
        assert_eq!(resolver.lookups(), ["bots.internal"]);
        assert_eq!(*dialer.dialed.lock().unwrap(), ["10.0.0.7:8080".parse().unwrap()]);
        assert_eq!(server.received()[0].header("Host"), Some("bots.internal:8080"));
    }

    /// An endpoint that keeps sending never trips the 7-second read timeout, but the delivery as a
    /// whole gives up and says so.
    #[tokio::test]
    async fn gives_up_on_a_trickling_reply() {
        let server = trickling_server("HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nConnection: close\r\n\r\n").await;
        let net = crate::integrations::net::Network::system();
        let deadline = Duration::from_secs(1);
        let delivery = deliver_within(&net, &format!("http://{server}/hook"), "{}".into(), deadline).await.unwrap();
        assert_eq!(delivery, WebhookDelivery { status: None, reply: WebhookReply::Text("Failed to respond within 1 seconds".into()) });
    }

    /// A reply that inflates past the limit fails the delivery, having read little more than
    /// the limit.
    #[tokio::test]
    async fn rejects_replies_over_the_limit() {
        let bomb = gzip_bomb(MAX_REPLY_SIZE / 1024 / 1024 + 1);
        let route = Route::new("POST", "*", "/hook", 200).header("Content-Type", "image/png").header("Content-Encoding", "gzip").body(bomb);
        let server = FakeServer::start(vec![route]).await;
        let net = crate::integrations::net::Network::system();
        let outcome = deliver(&net, &format!("http://{}/hook", server.addr), "{}".into()).await;
        assert!(matches!(outcome, Err(WebhookError::ReplyTooLarge)), "{outcome:?}");
    }

    #[test]
    fn looks_up_mime_types_like_rails() {
        assert_eq!(mime_lookup("image/jpeg").unwrap(), (Some("jpeg"), "image/jpeg".into()));
        assert_eq!(mime_lookup("application/x-gzip").unwrap(), (Some("gzip"), "application/gzip".into()));
        assert_eq!(mime_lookup("IMAGE/PNG").unwrap(), (None, "IMAGE/PNG".into()));
        assert_eq!(mime_lookup("text/html; charset=utf-8").unwrap(), (Some("html"), "text/html".into()));
        assert_eq!(mime_lookup("video/quicktime").unwrap(), (None, "video/quicktime".into()));
        for invalid in ["image", "", "text/html, text", "a/b c"] {
            assert!(mime_lookup(invalid).is_err(), "{invalid}");
        }
    }
}
