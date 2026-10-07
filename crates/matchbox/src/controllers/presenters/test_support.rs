//! Request-level test support for these controllers: the whole app booted over a private copy of
//! the reference-built `default` parity seed, signed in with a Rails-issued session cookie
//! (`vectors/campfire_sessions.json`), with a tiny cookie jar and CSRF token handling.

use std::collections::BTreeMap;
use std::path::Path;

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use futures_util::StreamExt;
use tower::ServiceExt;

use crate::app::{Booted, boot};
use crate::config::Config;
use crate::test_support::seed_dir;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

pub const DAVID: i64 = 127326141;
pub const JASON: i64 = 149087659;
pub const KEVIN: i64 = 712064548;
pub const BENDER: i64 = 394959859;
pub const BENDER_KEY: &str = "394959859-BenderBot123";
/// Rooms::Closed "All Talk" (David, Jason, Bender): 131 messages.
pub const ALL_TALK: i64 = 486777696;
/// Rooms::Open "HQ" (David can't see messages; no messages).
pub const HQ: i64 = 201306877;
/// Rooms::Closed "Quiet Corner", created by Kevin, David a member.
pub const QUIET_CORNER: i64 = 699448326;
/// A Rooms::Direct between David and Jason.
pub const DIRECT_DAVID_JASON: i64 = 186869642;
/// Kevin and Bender's direct room: David isn't in it.
pub const DIRECT_KEVIN_BENDER: i64 = 340026324;

fn parity_env(name: &str) -> Option<String> {
    let env = std::fs::read_to_string(Path::new(ROOT).join("parity/.env.reference")).ok()?;
    env.lines().find_map(|line| line.strip_prefix(&format!("{name}=")).map(str::to_string))
}

/// David's Rails-issued `session_token` cookie header.
pub fn david_cookie() -> String {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../vectors/campfire_sessions.json"))).unwrap();
    vectors["sessions"].as_array().unwrap().iter().find(|s| s["user_name"] == "David").unwrap()["cookie_header"]
        .as_str()
        .unwrap()
        .to_string()
}

pub struct TestApp {
    pub booted: Booted,
    _dir: tempfile::TempDir,
}

impl TestApp {
    /// `None` (and a note) when the seed hasn't been built.
    pub async fn boot() -> Option<TestApp> {
        let seed = seed_dir("default")?;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("db")).unwrap();
        std::fs::copy(seed.join("db/production.sqlite3"), dir.path().join("db/production.sqlite3")).unwrap();
        copy_dir(&seed.join("storage"), &dir.path().join("files"));
        let root = dir.path().to_string_lossy().into_owned();
        let secret = parity_env("SECRET_KEY_BASE").unwrap();
        let config = Config::from_lookup(|name| match name {
            "SECRET_KEY_BASE" => Some(secret.clone()),
            "DISABLE_SSL" => Some("true".into()),
            "APP_VERSION" | "GIT_REVISION" => Some("parity".into()),
            "MATCHBOX_STORAGE_PATH" => Some(root.clone()),
            _ => None,
        })
        .unwrap();
        Some(TestApp { booted: boot(config).await.unwrap(), _dir: dir })
    }

    pub fn db(&self) -> &matchbox_db::Database {
        &self.booted.app.db
    }

    /// A browser signed in as David.
    pub fn david(&self) -> Browser<'_> {
        let mut browser = Browser { app: self, cookies: BTreeMap::new() };
        browser.absorb_cookie_header(&david_cookie());
        browser
    }

    pub fn anonymous(&self) -> Browser<'_> {
        Browser { app: self, cookies: BTreeMap::new() }
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[derive(Debug)]
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    /// How many data frames the body came in.
    pub frames: usize,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("{e}: {}", self.text()))
    }

    pub fn location(&self) -> Option<&str> {
        self.header("location")
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }
}

/// A client that keeps cookies between requests.
pub struct Browser<'a> {
    app: &'a TestApp,
    cookies: BTreeMap<String, String>,
}

pub struct Req {
    pub method: Method,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Req {
    pub fn new(method: Method, path: &str) -> Self {
        Req { method, path: path.to_string(), headers: Vec::new(), body: Vec::new() }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn form(mut self, pairs: &[(&str, &str)]) -> Self {
        let body: Vec<String> = pairs.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect();
        self.body = body.join("&").into_bytes();
        self.header("content-type", "application/x-www-form-urlencoded")
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    /// A multipart body with text fields and one file field.
    pub fn multipart(mut self, fields: &[(&str, &str)], file: (&str, &str, &str, &[u8])) -> Self {
        let boundary = "----matchboxtestboundary";
        let mut body = Vec::new();
        for (name, value) in fields {
            body.extend_from_slice(
                format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
            );
        }
        let (name, filename, content_type, data) = file;
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        self.body = body;
        self.header("content-type", &format!("multipart/form-data; boundary={boundary}"))
    }
}

pub fn encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

impl Browser<'_> {
    /// Sets a cookie as a response would have (the value as it goes in the header).
    pub fn set_cookie(&mut self, name: &str, value: &str) {
        self.cookies.insert(name.to_string(), value.to_string());
    }

    fn absorb_cookie_header(&mut self, header: &str) {
        for pair in header.split(';') {
            if let Some((name, value)) = pair.trim().split_once('=') {
                self.cookies.insert(name.to_string(), value.to_string());
            }
        }
    }

    fn absorb_set_cookies(&mut self, headers: &HeaderMap) {
        for value in headers.get_all(header::SET_COOKIE) {
            let value = value.to_str().unwrap();
            let pair = value.split(';').next().unwrap();
            if let Some((name, value)) = pair.split_once('=') {
                if value.is_empty() {
                    self.cookies.remove(name);
                } else {
                    self.cookies.insert(name.to_string(), value.to_string());
                }
            }
        }
    }

    pub async fn send(&mut self, req: Req) -> Reply {
        let mut request = Request::builder().method(req.method.clone()).uri(&req.path).header(header::HOST, "campfire.test");
        if !self.cookies.is_empty() {
            let cookie = self.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ");
            request = request.header(header::COOKIE, cookie);
        }
        let has_accept = req.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("accept"));
        if !has_accept {
            request = request.header(header::ACCEPT, "text/html,application/xhtml+xml");
        }
        for (name, value) in &req.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let request = request.body(Body::from(req.body)).unwrap();
        let response = self.app.booted.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let chunks: Vec<_> = response.into_body().into_data_stream().collect().await;
        let frames = chunks.len();
        let body = chunks.into_iter().flat_map(|chunk| chunk.unwrap()).collect();
        self.absorb_set_cookies(&headers);
        Reply { status, headers, body, frames }
    }

    pub async fn get(&mut self, path: &str) -> Reply {
        self.send(Req::new(Method::GET, path)).await
    }

    /// A write as the app's own pages make it: same-origin, by `Sec-Fetch-Site`.
    pub async fn write(&mut self, req: Req) -> Reply {
        self.send(req.header("sec-fetch-site", "same-origin")).await
    }
}
