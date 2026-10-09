//! The front server (Thruster's job) end to end, over real sockets.
//!
//! The ACME tests need a local Pebble CA and are skipped unless `PEBBLE_MINICA` points at its
//! `test/certs/pebble.minica.pem` (the root of the directory's own HTTPS certificate):
//!
//!   docker run -d --name pebble --network host --add-host campfire.test:127.0.0.1 \
//!     -e PEBBLE_VA_NOSLEEP=1 -e PEBBLE_WFE_NONCEREJECT=0 ghcr.io/letsencrypt/pebble:latest
//!   docker cp pebble:/test/certs/pebble.minica.pem /tmp/pebble.minica.pem
//!   PEBBLE_MINICA=/tmp/pebble.minica.pem cargo test -p matchbox_kit --test front acme -- --test-threads 1
//!
//! Pebble validates TLS-ALPN-01 on port 5001, so the test serves HTTPS there. (HTTP-01 can't be
//! tested this way: like autocert, the challenge handler checks the `Host` header, port included,
//! against TLS_DOMAIN, so it only answers validations on port 80.)

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Request};
use axum::http::{HeaderMap, Response};
use axum::routing::{any, get, post};
use futures_util::{SinkExt, StreamExt};
use matchbox_kit::front::{self, AcmeOptions, FrontConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

struct Server {
    http: u16,
    target: u16,
    stop: Option<oneshot::Sender<()>>,
    done: tokio::task::JoinHandle<()>,
}

impl Server {
    /// On fresh ports, picking new ones when something took one between `free_port` and the bind.
    async fn start(vars: &[(&str, &str)], app: Router) -> Self {
        for _ in 0..5 {
            if let Some(server) = Self::try_start(vars, app.clone(), None, free_port(), free_port()).await {
                return server;
            }
        }
        panic!("front server didn't start");
    }

    async fn start_with(vars: &[(&str, &str)], app: Router, acme: Option<AcmeOptions>, http: u16, https: u16) -> Self {
        Self::try_start(vars, app, acme, http, https).await.expect("front server didn't start")
    }

    /// `None` when the server stops before it's listening (a port was taken).
    async fn try_start(vars: &[(&str, &str)], app: Router, acme: Option<AcmeOptions>, http: u16, https: u16) -> Option<Self> {
        let target = free_port();
        let mut env: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        env.entry("HTTP_PORT".into()).or_insert(http.to_string());
        env.entry("HTTPS_PORT".into()).or_insert(https.to_string());
        env.insert("TARGET_PORT".into(), target.to_string());
        env.insert("LOG_REQUESTS".into(), "false".into());
        let config = FrontConfig::from_lookup(|name| env.get(name).cloned());
        let (stop, stopped) = oneshot::channel::<()>();
        let done = tokio::spawn(async move {
            if let Err(error) = front::serve_with(config, app, acme, async move {
                let _ = stopped.await;
            })
            .await
            {
                eprintln!("front server stopped: {error}");
            }
        });
        for _ in 0..100 {
            if done.is_finished() {
                return None;
            }
            if TcpStream::connect(("127.0.0.1", http)).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Some(Self { http, target, stop: Some(stop), done })
    }

    async fn stop(mut self) {
        let _ = self.stop.take().unwrap().send(());
        let _ = tokio::time::timeout(Duration::from_secs(10), &mut self.done).await;
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A raw HTTP/1.1 exchange on a fresh connection, read to EOF.
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn all(&self, name: &str) -> Vec<&str> {
        self.headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()).collect()
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.all(name).first().copied()
    }
}

async fn exchange(port: u16, request: &str) -> Reply {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut raw)).await.unwrap().unwrap();
    parse(&raw)
}

fn parse(raw: &[u8]) -> Reply {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("a complete head");
    let head = std::str::from_utf8(&raw[..split]).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers: Vec<(String, String)> =
        lines.map(|l| l.split_once(": ").map(|(n, v)| (n.to_ascii_lowercase(), v.to_string())).unwrap()).collect();
    let mut body = raw[split + 4..].to_vec();
    if headers.iter().any(|(n, v)| n == "transfer-encoding" && v == "chunked") {
        body = dechunk(&body);
    }
    Reply { status, headers, body }
}

fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let line_end = data.windows(2).position(|w| w == b"\r\n").unwrap();
        let size = usize::from_str_radix(std::str::from_utf8(&data[..line_end]).unwrap().trim(), 16).unwrap();
        data = &data[line_end + 2..];
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

fn get_request(path: &str, extra: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: chat.test\r\nConnection: close\r\n{extra}\r\n")
}

/// An app with the kinds of responses Ember sends.
fn test_app() -> (Router, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let router = Router::new()
        .route(
            "/public",
            get(move || {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                async move {
                    Response::builder()
                        .header("cache-control", "public, max-age=60")
                        .header("content-type", "text/css")
                        .header("etag", "\"v1\"")
                        .header("set-cookie", "tracked=1; path=/")
                        .header("vary", "Accept-Encoding")
                        .body(Body::from(format!("render {n}")))
                        .unwrap()
                }
            }),
        )
        .route(
            "/streamed",
            get(|| async {
                // `send_file`: a streamed body whose length is declared up front, so the server
                // never polls it past its last byte.
                let chunks = futures_util::stream::iter(["stream", "ed"].map(|c| Ok::<_, std::io::Error>(axum::body::Bytes::from(c))));
                Response::builder()
                    .header("cache-control", "public, max-age=60")
                    .header("content-length", "8")
                    .header("vary", "Accept-Encoding")
                    .body(Body::from_stream(chunks))
                    .unwrap()
            }),
        )
        .route(
            "/private",
            get(|| async {
                Response::builder()
                    .header("cache-control", "max-age=0, private, must-revalidate")
                    .header("vary", "Accept-Encoding")
                    .header("set-cookie", "session=1; path=/")
                    .body(Body::from("private"))
                    .unwrap()
            }),
        )
        .route(
            "/page",
            get(|| async {
                Response::builder()
                    .header("content-type", "text/html; charset=utf-8")
                    .body(Body::from("<p>matchbox</p>".repeat(200)))
                    .unwrap()
            }),
        )
        .route("/headers", get(echo_headers).post(echo_headers))
        .route("/upload", post(|body: axum::body::Bytes| async move { format!("{} bytes", body.len()) }))
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(3)).await;
                "late"
            }),
        )
        .route(
            "/cable",
            any(|ws: WebSocketUpgrade| async move {
                ws.on_upgrade(|mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if let Message::Text(text) = message {
                            let _ = socket.send(Message::Text(format!("echo {text}").into())).await;
                        }
                    }
                })
            }),
        );
    (router, hits)
}

async fn echo_headers(ConnectInfo(remote): ConnectInfo<SocketAddr>, request: Request) -> String {
    let headers: &HeaderMap = request.headers();
    let get = |name| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("-").to_string();
    if request.uri().query() == Some("h2") {
        return format!("host={} cookie={} version={:?} uri={}", get("host"), get("cookie"), request.version(), request.uri());
    }
    format!(
        "for={} host={} proto={} forwarded={} start={} peer={}",
        get("x-forwarded-for"),
        get("x-forwarded-host"),
        get("x-forwarded-proto"),
        get("forwarded"),
        get("x-request-start").starts_with("t="),
        remote.ip()
    )
}

#[tokio::test]
async fn caches_public_responses() {
    let (app, renders) = test_app();
    let server = Server::start(&[], app).await;

    let first = exchange(server.http, &get_request("/public", "Accept-Encoding: identity\r\n")).await;
    assert_eq!(first.status, 200);
    assert_eq!(first.get("x-cache"), Some("miss"));
    assert_eq!(first.get("set-cookie"), None, "cacheable responses lose their cookies");
    assert_eq!(first.all("vary"), ["Accept-Encoding"]);
    assert!(first.get("date").is_some());
    assert_eq!(first.body, b"render 0");

    let second = exchange(server.http, &get_request("/public?", "Accept-Encoding: identity\r\n")).await;
    assert_eq!(second.get("x-cache"), Some("hit"));
    assert_eq!(second.body, b"render 0");
    assert_eq!(second.get("etag"), Some("\"v1\""));

    // Another Accept-Encoding is another variant.
    let other = exchange(server.http, &get_request("/public", "Accept-Encoding: br\r\n")).await;
    assert_eq!((other.get("x-cache"), other.body.as_slice()), (Some("miss"), b"render 1".as_slice()));
    let again = exchange(server.http, &get_request("/public", "Accept-Encoding: br\r\n")).await;
    assert_eq!((again.get("x-cache"), again.body.as_slice()), (Some("hit"), b"render 1".as_slice()));

    let revalidated = exchange(server.http, &get_request("/public", "Accept-Encoding: identity\r\nIf-None-Match: \"v1\"\r\n")).await;
    assert_eq!((revalidated.status, revalidated.get("x-cache")), (304, Some("hit")));
    assert!(revalidated.body.is_empty());

    let ranged = exchange(server.http, &get_request("/public", "Accept-Encoding: identity\r\nRange: bytes=0-1\r\n")).await;
    assert_eq!(ranged.get("x-cache"), Some("bypass"), "ranges go to the app");
    assert_eq!(renders.load(Ordering::SeqCst), 3);

    // Long URIs aren't cached.
    let long = format!("/public?pad={}", "x".repeat(4096));
    for _ in 0..2 {
        let reply = exchange(server.http, &get_request(&long, "Accept-Encoding: identity\r\n")).await;
        assert_eq!(reply.get("x-cache"), Some("bypass"));
    }
    assert_eq!(renders.load(Ordering::SeqCst), 5);

    for _ in 0..2 {
        let private = exchange(server.http, &get_request("/private", "")).await;
        assert_eq!(private.get("x-cache"), Some("miss"));
        assert_eq!(private.get("set-cookie"), Some("session=1; path=/"));
        assert_eq!(private.all("vary"), ["Accept-Encoding"]);
    }
    server.stop().await;
}

#[tokio::test]
async fn caches_streamed_responses_of_declared_length() {
    let (app, _) = test_app();
    let server = Server::start(&[], app).await;
    for (encoding, expected) in [("identity", "miss"), ("identity", "hit"), ("gzip", "miss"), ("gzip", "hit")] {
        let reply = exchange(server.http, &get_request("/streamed", &format!("Accept-Encoding: {encoding}\r\n"))).await;
        assert_eq!((reply.get("x-cache"), reply.body.as_slice()), (Some(expected), b"streamed".as_slice()), "{encoding}");
    }
    server.stop().await;
}

#[tokio::test]
async fn bypassed_requests_repeat_vary() {
    let (app, _) = test_app();
    let server = Server::start(&[], app).await;
    let request = "POST /headers HTTP/1.1\r\nHost: chat.test\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";
    let reply = exchange(server.http, request).await;
    assert_eq!(reply.get("x-cache"), Some("bypass"));
    assert_eq!(reply.all("vary"), ["Accept-Encoding"]);

    let (app, _) = test_app();
    let with_vary = app.layer(axum::middleware::map_response(|mut response: Response<Body>| async move {
        response.headers_mut().insert("vary", "Accept-Encoding".parse().unwrap());
        response
    }));
    let server2 = Server::start(&[], with_vary).await;
    let reply = exchange(server2.http, request).await;
    assert_eq!(reply.all("vary"), ["Accept-Encoding", "Accept-Encoding"]);
    server.stop().await;
    server2.stop().await;
}

#[tokio::test]
async fn compresses_what_the_app_left_unencoded() {
    let (app, _) = test_app();
    let server = Server::start(&[], app).await;
    let zstd = exchange(server.http, &get_request("/page", "Accept-Encoding: zstd\r\n")).await;
    assert_eq!(zstd.get("content-encoding"), Some("zstd"));
    assert_eq!(zstd::stream::decode_all(&zstd.body[..]).unwrap(), "<p>matchbox</p>".repeat(200).as_bytes());
    let plain = exchange(server.http, &get_request("/page", "")).await;
    assert_eq!(plain.get("content-encoding"), None);
    let head = exchange(server.http, "HEAD /page HTTP/1.1\r\nHost: chat.test\r\nConnection: close\r\nAccept-Encoding: gzip\r\n\r\n").await;
    assert_eq!(head.get("content-encoding"), None);
    server.stop().await;
}

#[tokio::test]
async fn forwards_client_addresses() {
    let (app, _) = test_app();
    let server = Server::start(&[], app).await;
    let reply = exchange(
        server.http,
        &get_request("/headers", "X-Forwarded-For: 203.0.113.9\r\nX-Forwarded-Proto: https\r\nForwarded: for=1.2.3.4\r\n"),
    )
    .await;
    assert_eq!(
        String::from_utf8(reply.body).unwrap(),
        "for=203.0.113.9, 127.0.0.1 host=chat.test proto=https forwarded=- start=true peer=127.0.0.1"
    );

    let (app, _) = test_app();
    let untrusting = Server::start(&[("FORWARD_HEADERS", "false")], app).await;
    let reply = exchange(untrusting.http, &get_request("/headers", "X-Forwarded-For: 203.0.113.9\r\nX-Forwarded-Proto: https\r\n")).await;
    assert_eq!(String::from_utf8(reply.body).unwrap(), "for=127.0.0.1 host=chat.test proto=http forwarded=- start=true peer=127.0.0.1");
    server.stop().await;
    untrusting.stop().await;
}

#[tokio::test]
async fn the_app_still_listens_on_the_target_port() {
    let (app, _) = test_app();
    let server = Server::start(&[], app).await;
    let reply = exchange(server.target, &get_request("/public", "")).await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.get("x-cache"), None);
    assert_eq!(reply.get("date"), None);
    assert_eq!(reply.get("set-cookie"), Some("tracked=1; path=/"));
    server.stop().await;
}

#[tokio::test]
async fn the_target_port_is_loopback_only_and_limited() {
    let (app, _) = test_app();
    let server = Server::start(&[("HTTP_READ_TIMEOUT", "1"), ("MAX_REQUEST_BODY", "10")], app).await;
    if let Some(address) = non_loopback_address() {
        assert!(TcpStream::connect((address, server.target)).await.is_err(), "reachable on {address}");
    }

    let large =
        exchange(server.target, "POST /upload HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 11\r\n\r\nhello world").await;
    assert_eq!(large.status, 413);

    let mut stream = TcpStream::connect(("127.0.0.1", server.target)).await.unwrap();
    stream.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n").await.unwrap();
    let mut buffer = vec![0; 4096];
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer)).await.expect("closed by the read timeout").unwrap_or(0);
    assert!(n == 0 || buffer.starts_with(b"HTTP/1.1 408"));
    server.stop().await;
}

/// This machine's address on its default route, if it has one.
fn non_loopback_address() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip()).filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
}

#[tokio::test]
async fn limits_request_bodies() {
    let (app, _) = test_app();
    let server = Server::start(&[("MAX_REQUEST_BODY", "10")], app).await;
    let small = exchange(server.http, "POST /upload HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 5\r\n\r\nhello").await;
    assert_eq!((small.status, small.body.as_slice()), (200, b"5 bytes".as_slice()));
    let large =
        exchange(server.http, "POST /upload HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 11\r\n\r\nhello world").await;
    assert_eq!(large.status, 413);
    let chunked =
        "POST /upload HTTP/1.1\r\nHost: x\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n6\r\nhello \r\n5\r\nworld\r\n0\r\n\r\n";
    assert_eq!(exchange(server.http, chunked).await.status, 413);
    server.stop().await;
}

#[tokio::test]
async fn closes_idle_and_slow_connections() {
    let (app, _) = test_app();
    let server = Server::start(&[("HTTP_IDLE_TIMEOUT", "3"), ("HTTP_READ_TIMEOUT", "1"), ("HTTP_WRITE_TIMEOUT", "2")], app).await;

    // Idle keep-alive connection: over HTTP/1 it closes after the read timeout, the shorter of the
    // two (see "The front server is stricter than Thruster" in README.md).
    let mut stream = TcpStream::connect(("127.0.0.1", server.http)).await.unwrap();
    stream.write_all(b"GET /private HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
    let mut buffer = vec![0; 4096];
    let n = stream.read(&mut buffer).await.unwrap();
    assert!(n > 0 && buffer.starts_with(b"HTTP/1.1 200"));
    let started = std::time::Instant::now();
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer)).await.unwrap().unwrap_or(0);
    assert_eq!(n, 0, "closed");
    let idle = started.elapsed();
    assert!(idle >= Duration::from_millis(800) && idle < Duration::from_millis(2500), "closed after {idle:?}");

    // A request that never finishes its headers.
    let mut stream = TcpStream::connect(("127.0.0.1", server.http)).await.unwrap();
    stream.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n").await.unwrap();
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer)).await.unwrap().unwrap_or(0);
    assert!(n == 0 || buffer.starts_with(b"HTTP/1.1 408"), "closed");

    // A response that takes longer than the write timeout: no response at all.
    let mut stream = TcpStream::connect(("127.0.0.1", server.http)).await.unwrap();
    stream.write_all(b"GET /slow HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer)).await.unwrap().unwrap_or(0);
    assert_eq!(n, 0, "dropped without a response");
    server.stop().await;
}

#[tokio::test]
async fn http2_connections_close_after_the_idle_timeout() {
    let (app, _) = test_app();
    let server = Server::start(&[("H2C_ENABLED", "true"), ("HTTP_IDLE_TIMEOUT", "2"), ("HTTP_READ_TIMEOUT", "1")], app).await;
    let stream = TcpStream::connect(("127.0.0.1", server.http)).await.unwrap();
    let (mut sender, connection) =
        hyper::client::conn::http2::handshake(hyper_util::rt::TokioExecutor::new(), hyper_util::rt::TokioIo::new(stream)).await.unwrap();
    let connection = tokio::spawn(connection);
    let request = axum::http::Request::get(format!("http://127.0.0.1:{}/private", server.http)).body(Body::empty()).unwrap();
    let reply = sender.send_request(request).await.unwrap();
    http_body_util::BodyExt::collect(reply.into_body()).await.unwrap();
    let started = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(5), connection).await.unwrap().unwrap().unwrap();
    let idle = started.elapsed();
    assert!(idle >= Duration::from_millis(1800) && idle < Duration::from_millis(3500), "closed after {idle:?}");
    server.stop().await;
}

#[tokio::test]
async fn websockets_pass_through_and_outlive_the_timeouts() {
    let (app, _) = test_app();
    let server = Server::start(&[("HTTP_IDLE_TIMEOUT", "1"), ("HTTP_READ_TIMEOUT", "1"), ("HTTP_WRITE_TIMEOUT", "1")], app).await;
    let (mut socket, response) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/cable", server.http)).await.unwrap();
    assert_eq!(response.status(), 101);
    assert_eq!(response.headers()["x-cache"], "bypass");
    assert_eq!(response.headers()["vary"], "Accept-Encoding");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    socket.send(tokio_tungstenite::tungstenite::Message::Text("ping".into())).await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
    assert_eq!(reply.into_text().unwrap().as_str(), "echo ping");
    drop(socket);
    server.stop().await;
}

#[tokio::test]
async fn speaks_h2c_only_when_enabled() {
    let (app, _) = test_app();
    let server = Server::start(&[("H2C_ENABLED", "true")], app).await;
    let reply = h2_get(server.http, "/private").await.unwrap();
    assert_eq!(reply.status(), 200);
    assert_eq!(reply.headers()["x-cache"], "miss");
    // The app gets it as Thruster's HTTP/1.1 request: a Host, an origin-form path, one Cookie.
    let reply = h2_get(server.http, "/headers?h2").await.unwrap();
    let body = http_body_util::BodyExt::collect(reply.into_body()).await.unwrap().to_bytes();
    assert_eq!(
        std::str::from_utf8(&body).unwrap(),
        format!("host=127.0.0.1:{} cookie=a=1; b=2 version=HTTP/1.1 uri=/headers?h2", server.http)
    );
    server.stop().await;

    let (app, _) = test_app();
    let server = Server::start(&[], app).await;
    assert!(h2_get(server.http, "/private").await.is_err());
    server.stop().await;
}

async fn h2_get(port: u16, path: &str) -> Result<Response<hyper::body::Incoming>, hyper::Error> {
    let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let (mut sender, connection) =
        hyper::client::conn::http2::handshake(hyper_util::rt::TokioExecutor::new(), hyper_util::rt::TokioIo::new(stream)).await?;
    tokio::spawn(connection);
    let request = axum::http::Request::get(format!("http://127.0.0.1:{port}{path}"))
        .header("cookie", "a=1")
        .header("cookie", "b=2")
        .body(Body::empty())
        .unwrap();
    sender.send_request(request).await
}

// --- ACME (Pebble) -------------------------------------------------------------------------------

fn pebble() -> Option<std::path::PathBuf> {
    std::env::var_os("PEBBLE_MINICA").map(Into::into)
}

fn acme_options(domain: &str, storage: &std::path::Path, root: &std::path::Path, types: Vec<instant_acme::ChallengeType>) -> AcmeOptions {
    AcmeOptions {
        directory_url: "https://localhost:14000/dir".into(),
        external_account: None,
        storage_path: storage.into(),
        domains: vec![domain.into()],
        challenge_types: types,
        directory_root: Some(root.into()),
    }
}

/// A TLS handshake with SNI `domain` on 127.0.0.1:`port`: the negotiated protocol and the leaf's
/// issuer.
async fn handshake(port: u16, domain: &str) -> (Option<Vec<u8>>, String) {
    #[derive(Debug)]
    struct AnyCertificate(Arc<rustls::crypto::CryptoProvider>);
    impl rustls::client::danger::ServerCertVerifier for AnyCertificate {
        fn verify_server_cert(
            &self,
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &[rustls::pki_types::CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &rustls::pki_types::CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
        }
        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &rustls::pki_types::CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.0.signature_verification_algorithms.supported_schemes()
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let name = rustls::pki_types::ServerName::try_from(domain.to_string()).unwrap();
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, stream).await.unwrap();
    let (_, connection) = tls.get_ref();
    let leaf = connection.peer_certificates().unwrap()[0].clone();
    let (_, parsed) = x509_parser::parse_x509_certificate(leaf.as_ref()).unwrap();
    (connection.alpn_protocol().map(<[u8]>::to_vec), parsed.issuer().to_string())
}

#[tokio::test]
async fn acme_tls_alpn_certificate_cached_and_reused() {
    let Some(root) = pebble() else { return eprintln!("skipped: PEBBLE_MINICA isn't set") };
    let storage = tempfile::tempdir().unwrap();
    let domain = "campfire.test";
    let (app, _) = test_app();
    let vars = [("TLS_DOMAIN", domain)];
    let acme = acme_options(domain, storage.path(), &root, vec![instant_acme::ChallengeType::TlsAlpn01]);
    let server = Server::start_with(&vars, app, Some(acme), 5002, 5001).await;

    let (alpn, issuer) = handshake(5001, domain).await;
    assert_eq!(alpn.as_deref(), Some(b"h2".as_slice()));
    assert!(issuer.contains("Pebble"), "issued by Pebble: {issuer}");
    let cached = std::fs::read_to_string(storage.path().join(domain)).unwrap();
    assert!(cached.starts_with("-----BEGIN EC PRIVATE KEY-----") && cached.contains("-----BEGIN CERTIFICATE-----"));
    assert!(std::fs::read_to_string(storage.path().join("acme_account+key")).unwrap().starts_with("-----BEGIN EC PRIVATE KEY-----"));

    let redirect = exchange(5002, "GET /rooms?x=1 HTTP/1.1\r\nHost: campfire.test\r\nConnection: close\r\n\r\n").await;
    assert_eq!((redirect.status, redirect.get("location")), (301, Some("https://campfire.test/rooms?x=1")));
    let misdirected = exchange(5002, "GET / HTTP/1.1\r\nHost: other.test\r\nConnection: close\r\n\r\n").await;
    assert_eq!(misdirected.status, 421);
    server.stop().await;

    // A restart serves the cached certificate without asking the CA.
    let (app, _) = test_app();
    let mut offline = acme_options(domain, storage.path(), &root, vec![instant_acme::ChallengeType::TlsAlpn01]);
    offline.directory_url = "https://127.0.0.1:9/dir".into();
    let server = Server::start_with(&vars, app, Some(offline), 5002, 5001).await;
    let (_, again) = handshake(5001, domain).await;
    assert_eq!(again, issuer);
    server.stop().await;
}
