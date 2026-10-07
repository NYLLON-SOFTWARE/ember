//! Boot-level tests: the whole stack over a copy of the reference-built `default` parity seed
//! (`parity/bin/seed build default`), with the parity `SECRET_KEY_BASE`. Skipped (with a note)
//! when the seed hasn't been built.
//!
//! `vectors/campfire_sessions.json` holds session cookies *issued by Rails* for the seed's
//! sessions (`reference-tools/campfire/session_cookies.rb`).

use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use matchbox_kit::{Ctx, Kit, KitConfig, Result};
use tower::ServiceExt;

use super::*;
use crate::concerns::{Before, before_actions, current_user};
use crate::test_support::seed_dir;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

fn parity_env(name: &str) -> Option<String> {
    let env = std::fs::read_to_string(Path::new(ROOT).join("parity/.env.reference")).ok()?;
    env.lines().find_map(|line| line.strip_prefix(&format!("{name}=")).map(str::to_string))
}

#[derive(serde::Deserialize)]
struct Vectors {
    sessions: Vec<SessionVector>,
    blobs: Vec<BlobVector>,
    forged: CookieVector,
}

#[derive(serde::Deserialize)]
struct SessionVector {
    user_name: String,
    cookie_header: String,
}

#[derive(serde::Deserialize)]
struct BlobVector {
    redirect_path: String,
}

#[derive(serde::Deserialize)]
struct CookieVector {
    cookie_header: String,
}

fn vectors() -> Vectors {
    serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../vectors/campfire_sessions.json"))).unwrap()
}

/// A booted app over a private copy of the seed.
struct Test {
    booted: Booted,
    _dir: tempfile::TempDir,
}

async fn boot_seeded() -> Option<Test> {
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
    Some(Test { booted: boot(config).await.unwrap(), _dir: dir })
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

struct Reply {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

async fn send(router: &axum::Router, request: Request<Body>) -> Reply {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec();
    Reply { status, headers, body }
}

fn get(path: &str) -> Request<Body> {
    Request::get(path).header(header::HOST, "campfire.test").body(Body::empty()).unwrap()
}

fn get_with_cookie(path: &str, cookie: &str) -> Request<Body> {
    Request::get(path).header(header::HOST, "campfire.test").header(header::COOKIE, cookie).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn health_check() {
    let Some(test) = boot_seeded().await else { return };
    let up = send(&test.booted.router, get("/up")).await;
    assert_eq!(up.status, StatusCode::OK);
    assert_eq!(up.text(), r#"<!DOCTYPE html><html><body style="background-color: green"></body></html>"#);
    assert_eq!(up.header("content-type"), Some("text/html; charset=utf-8"));

    let json = send(&test.booted.router, get("/up.json")).await;
    assert_eq!(json.status, StatusCode::OK);
    let value: serde_json::Value = serde_json::from_slice(&json.body).unwrap();
    assert_eq!(value["status"], "up");
}

#[tokio::test]
async fn public_files_are_served_before_routing() {
    let Some(test) = boot_seeded().await else { return };
    let css = matchbox_assets::stylesheet_path(matchbox_assets::all_stylesheet_paths()[0]);
    let reply = send(&test.booted.router, get(&css)).await;
    assert_eq!(reply.status, StatusCode::OK, "{css}");
    assert_eq!(reply.header("cache-control"), Some("public, max-age=2592000"));

    let robots = send(&test.booted.router, get("/robots.txt")).await;
    assert_eq!(robots.status, StatusCode::OK);
}

/// Every file `ActionDispatch::Static` serves: `public/` and the digested assets.
fn static_corpus() -> Vec<String> {
    let public = ["/robots.txt", "/404.html", "/422.html", "/500.html", "/502.html", "/assets/.manifest.json"];
    let assets = matchbox_assets::manifest().iter().map(|(_, digested)| format!("{}/{digested}", matchbox_assets::PREFIX));
    public.into_iter().map(str::to_string).chain(assets).collect()
}

fn static_request(method: &str, path: &str, range: Option<&str>, if_modified_since: Option<&str>) -> Request<Body> {
    let mut request = Request::builder().method(method).uri(path).header(header::HOST, "campfire.test");
    for (name, value) in [(header::RANGE, range), (header::IF_MODIFIED_SINCE, if_modified_since)] {
        if let Some(value) = value {
            request = request.header(name, value);
        }
    }
    request.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn static_responses_send_the_embedded_bytes_without_copying_them() {
    let built_at = matchbox_assets::serve(&matchbox_assets::StaticRequest { method: "GET", path: "/robots.txt", ..Default::default() })
        .unwrap()
        .header("last-modified")
        .unwrap()
        .to_string();
    let variants = [
        ("GET", None, None),
        ("HEAD", None, None),
        ("GET", Some("bytes=1-10"), None),
        ("GET", Some("bytes=0-1, 4-5"), None),
        ("GET", Some("bytes=999999999-"), None),
        ("GET", None, Some(built_at.as_str())),
    ];
    for path in static_corpus() {
        for (method, range, if_modified_since) in variants {
            let request = static_request(method, &path, range, if_modified_since);
            let served = matchbox_assets::serve(&matchbox_assets::StaticRequest {
                method,
                path: &path,
                range,
                if_modified_since,
                ..Default::default()
            })
            .unwrap();
            let response = static_response(&request).unwrap();
            let case = format!("{method} {path} {range:?} {if_modified_since:?}");

            assert_eq!(response.status().as_u16(), served.status, "{case}");
            let headers: Vec<(String, &str)> = response.headers().iter().map(|(n, v)| (n.to_string(), v.to_str().unwrap())).collect();
            let expected: Vec<(String, &str)> = served.headers.iter().map(|(n, v)| (n.to_ascii_lowercase(), v.as_str())).collect();
            assert_eq!(headers, expected, "{case}");
            let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert_eq!(body[..], served.body[..], "{case}");
            if matches!(served.body, matchbox_assets::Body::Borrowed(_)) && !body.is_empty() {
                assert_eq!(body.as_ptr(), served.body.as_ptr(), "{case}: sent from the embedded bytes");
            }
        }
    }
}

/// Times `static_response` (ActionDispatch::Static plus building and dropping the response) for
/// an identity GET of a small public file, the first stylesheet, the largest script and the
/// largest file; per call, median and range of 7 runs. In release:
/// `cargo test --release -p matchbox --bin matchbox static_response_timing -- --ignored --nocapture`
#[test]
#[ignore = "timing harness (bench/results/static-body-20260930)"]
fn static_response_timing() {
    const ITERATIONS: u32 = 20_000;
    let size = |path: &str| {
        matchbox_assets::serve(&matchbox_assets::StaticRequest { method: "GET", path, ..Default::default() }).map_or(0, |r| r.body.len())
    };
    let largest = |suffix: &str| static_corpus().into_iter().filter(|path| path.ends_with(suffix)).max_by_key(|path| size(path)).unwrap();
    let stylesheet = matchbox_assets::stylesheet_path(matchbox_assets::all_stylesheet_paths()[0]);
    for path in ["/robots.txt".to_string(), stylesheet, largest(".js"), largest("")] {
        let request = static_request("GET", &path, None, None);
        let mut runs: Vec<std::time::Duration> = (0..7)
            .map(|_| {
                let started = std::time::Instant::now();
                for _ in 0..ITERATIONS {
                    drop(std::hint::black_box(static_response(std::hint::black_box(&request))));
                }
                started.elapsed() / ITERATIONS
            })
            .collect();
        runs.sort();
        println!("{path} ({} bytes): median {:?}, range {:?}..{:?}", size(&path), runs[3], runs[0], runs[6]);
    }
}

#[tokio::test]
async fn unknown_and_unported_routes() {
    let Some(test) = boot_seeded().await else { return };
    let missing = send(&test.booted.router, get("/nope")).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert!(!missing.body.is_empty(), "renders public/404.html");

    let settings = send(&test.booted.router, get("/rooms/1/settings")).await;
    assert_eq!(settings.status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[test]
fn database_errors_answer_as_active_record_rescues_them() {
    assert_eq!(db_error(matchbox_db::Error::RecordNotFound("Room")).status(), StatusCode::NOT_FOUND);
    assert_eq!(db_error(matchbox_db::Error::WriterGone).status(), StatusCode::INTERNAL_SERVER_ERROR);

    let mut errors = matchbox_db::Errors::default();
    errors.add("endpoint", "must use HTTPS");
    let invalid = db_error(errors.into_result().unwrap_err());
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(format!("{invalid:#}"), "422 Unprocessable Entity: Validation failed: Endpoint must use HTTPS");
}

/// An action behind `ApplicationController`'s chain that answers with `Current.user`.
async fn whoami(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let name = current_user(c).map(|user| user.name.clone()).unwrap_or_default();
    Ok(c.html(name))
}

/// An action behind the chain that answers with whether `allow_browser` kept a platform, and the
/// browser and operating system the layout sees.
async fn platform(c: &mut Ctx) -> Result {
    before_actions(c, Before::default().allow_unauthenticated_access()).await?;
    platform_reply(c)
}

/// The same answer from an action that skips the chain, so `allow_browser` never runs.
async fn platform_without_the_chain(c: &mut Ctx) -> Result {
    platform_reply(c)
}

fn platform_reply(c: &mut Ctx) -> Result {
    let kept = c.current::<crate::concerns::platform::ApplicationPlatform>().is_some();
    let view = crate::concerns::platform(c).to_view();
    Ok(c.html(format!("{kept} {:?} {:?}", view.browser, view.operating_system)))
}

fn whoami_router(app: &App) -> axum::Router {
    let kit = Kit::new(KitConfig::production(true), app.secrets.clone(), app.clock.clone(), app.clone());
    matchbox_kit::app(axum::Router::new().route("/whoami", matchbox_kit::get(whoami).post(matchbox_kit::action(whoami))), kit)
}

#[tokio::test]
async fn a_rails_issued_session_cookie_authenticates() {
    let Some(test) = boot_seeded().await else { return };
    let vectors = vectors();
    let router = whoami_router(&test.booted.app);
    let session = &vectors.sessions[0];

    let signed_in = send(&router, get_with_cookie("/whoami", &session.cookie_header)).await;
    assert_eq!(signed_in.status, StatusCode::OK);
    assert_eq!(signed_in.text(), session.user_name);
    assert_eq!(signed_in.header("x-version"), Some("parity"));
    assert_eq!(signed_in.header("x-rev"), Some("parity"));
    let cookies: Vec<&str> = signed_in.headers.get_all(header::SET_COOKIE).iter().map(|v| v.to_str().unwrap()).collect();
    assert!(cookies.iter().any(|c| c.starts_with("session_token=") && c.contains("httponly") && c.contains("samesite=lax")), "{cookies:?}");

    // That request refreshed the seed's stale session; for the next hour it isn't touched again,
    // so the cookie isn't re-sent (Rails re-signs it on every request).
    let again = send(&router, get_with_cookie("/whoami", &session.cookie_header)).await;
    assert_eq!(again.text(), session.user_name);
    assert!(again.headers.get(header::SET_COOKIE).is_none(), "{:?}", again.headers);

    for cookie in [None, Some(vectors.forged.cookie_header.as_str()), Some("session_token=tampered--0000")] {
        let request = match cookie {
            Some(cookie) => get_with_cookie("/whoami", cookie),
            None => get("/whoami"),
        };
        let anonymous = send(&router, request).await;
        assert_eq!(anonymous.status, StatusCode::FOUND, "{cookie:?}");
        assert_eq!(anonymous.header("location"), Some("http://campfire.test/session/new"));
    }
}

#[tokio::test]
async fn the_application_chain_blocks_banned_ips_forgeries_and_old_browsers() {
    let Some(test) = boot_seeded().await else { return };
    let vectors = vectors();
    let router = whoami_router(&test.booted.app);
    let cookie = &vectors.sessions[0].cookie_header;
    let post = |extra: (&str, &str)| {
        Request::post("/whoami")
            .header(header::HOST, "campfire.test")
            .header(header::COOKIE, cookie)
            .header(extra.0, extra.1)
            .body(Body::empty())
            .unwrap()
    };

    // `ips.banned` in the seed's labels.
    let banned = send(&router, post(("x-forwarded-for", "203.0.113.9"))).await;
    assert_eq!(banned.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(banned.header("content-type"), Some("text/html"));

    // A post another site's page makes: the browser says so in `Sec-Fetch-Site`.
    let forged = send(&router, post(("sec-fetch-site", "cross-site"))).await;
    assert_eq!(forged.status, StatusCode::UNPROCESSABLE_ENTITY);

    let outdated = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/100.0.0.0 Safari/537.36";
    let request =
        Request::get("/whoami").header(header::HOST, "campfire.test").header(header::COOKIE, cookie).header(header::USER_AGENT, outdated);
    let old_browser = send(&router, request.body(Body::empty()).unwrap()).await;
    assert_eq!(old_browser.status, StatusCode::OK);
    assert_ne!(old_browser.text(), vectors.sessions[0].user_name);

    // The incompatible-browser page is an explicit `render template:`: HTML whatever the format.
    for (path, accept) in [("/webmanifest.json", "*/*"), ("/service-worker.js", "*/*"), ("/session/new", "application/json")] {
        let request =
            Request::get(path).header(header::HOST, "campfire.test").header(header::USER_AGENT, outdated).header(header::ACCEPT, accept);
        let blocked = send(&test.booted.router, request.body(Body::empty()).unwrap()).await;
        assert_eq!((blocked.status, blocked.header("content-type")), (StatusCode::OK, Some("text/html; charset=utf-8")), "{path} {accept}");
    }
    // In a Live controller (`include ActiveStorage::Streaming`) Rack::ETag can't digest the body.
    let request = Request::get("/account/logo").header(header::HOST, "campfire.test").header(header::USER_AGENT, outdated);
    let blocked = send(&test.booted.router, request.body(Body::empty()).unwrap()).await;
    assert_eq!((blocked.status, blocked.header("cache-control"), blocked.header("etag")), (StatusCode::OK, Some("no-cache"), None));
}

#[tokio::test]
async fn allow_browser_keeps_the_platform_it_parsed_for_the_layout() {
    let Some(test) = boot_seeded().await else { return };
    let app = &test.booted.app;
    let kit = Kit::new(KitConfig::production(true), app.secrets.clone(), app.clock.clone(), app.clone());
    let routes = axum::Router::new()
        .route("/platform", matchbox_kit::get(platform))
        .route("/platform/without_the_chain", matchbox_kit::get(platform_without_the_chain));
    let router = matchbox_kit::app(routes, kit);

    let chrome = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
    // Without a User-Agent to check, the layout parses the gem's default, "Mozilla/4.0 (compatible)".
    // Where `allow_browser` didn't run, the layout parses the header itself.
    let cases = [
        ("/platform", Some(chrome), r#"true "Chrome" "Windows""#),
        ("/platform", None, r#"false "Mozilla" """#),
        ("/platform", Some(" "), r#"false "Mozilla" """#),
        ("/platform/without_the_chain", Some(chrome), r#"false "Chrome" "Windows""#),
        ("/platform/without_the_chain", None, r#"false "Mozilla" """#),
    ];
    for (path, user_agent, expected) in cases {
        let mut request = Request::get(path).header(header::HOST, "campfire.test");
        if let Some(user_agent) = user_agent {
            request = request.header(header::USER_AGENT, user_agent);
        }
        let reply = send(&router, request.body(Body::empty()).unwrap()).await;
        assert_eq!((reply.status, reply.text().as_str()), (StatusCode::OK, expected), "{path} {user_agent:?}");
    }
}

#[tokio::test]
async fn blob_redirects_to_a_signed_disk_url() {
    let Some(test) = boot_seeded().await else { return };
    let vectors = vectors();
    let router = &test.booted.router;
    let redirect_path = &vectors.blobs[0].redirect_path;
    let filename = redirect_path.rsplit('/').next().unwrap();

    let redirect = send(router, get(redirect_path)).await;
    assert_eq!(redirect.status, StatusCode::FOUND);
    assert_eq!(redirect.header("cache-control"), Some("max-age=300, private"));
    let location = redirect.header("location").unwrap().to_string();
    assert!(location.starts_with("http://campfire.test/rails/active_storage/disk/"), "{location}");
    assert!(location.ends_with(&format!("/{filename}")), "{location}");

    let disk_path = location.strip_prefix("http://campfire.test").unwrap();
    let file = send(router, get(disk_path)).await;
    assert_eq!(file.status, StatusCode::OK);
    assert_eq!(file.header("cache-control"), Some("max-age=3600, public"));
    assert_eq!(file.header("content-type"), Some("image/jpeg"));
    assert_eq!(file.header("content-disposition").map(|d| d.starts_with("inline;")), Some(true));
    assert!(!file.body.is_empty());

    let tampered = redirect_path.replacen("--", "--0", 1);
    let bad = send(router, get(&tampered)).await;
    assert_eq!(bad.status, StatusCode::NOT_FOUND);
    assert!(bad.body.is_empty());
    // A before-action's `head` ignores the request format.
    assert_eq!(bad.header("content-type"), Some("text/html"));
}

#[tokio::test]
async fn disk_uploads_require_a_session() {
    let Some(test) = boot_seeded().await else { return };
    let put = Request::put("/rails/active_storage/disk/anything").header(header::HOST, "campfire.test").body(Body::from("x")).unwrap();
    let reply = send(&test.booted.router, put).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn cable_handshake_with_a_rails_session_cookie() {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let Some(test) = boot_seeded().await else { return };
    let vectors = vectors();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = test.booted.router.clone();
    let service = matchbox_kit::front::app_service(router);
    let shutdown = matchbox_kit::front::Shutdown::when(std::future::pending());
    tokio::spawn(matchbox_kit::front::serve_plain(listener, service, matchbox_kit::front::Protocol::Http1, Default::default(), shutdown));

    let connect = |cookie: Option<String>| async move {
        let mut request = format!("ws://{address}/cable").into_client_request().unwrap();
        request.headers_mut().insert("origin", format!("http://{address}").parse().unwrap());
        request.headers_mut().insert("sec-websocket-protocol", "actioncable-v1-json".parse().unwrap());
        if let Some(cookie) = cookie {
            request.headers_mut().insert("cookie", cookie.parse().unwrap());
        }
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let first = socket.next().await.unwrap().unwrap();
        first.into_text().unwrap().to_string()
    };

    assert_eq!(connect(Some(vectors.sessions[0].cookie_header.clone())).await, r#"{"type":"welcome"}"#);
    assert_eq!(connect(None).await, r#"{"type":"disconnect","reason":"unauthorized","reconnect":false}"#);
}

#[tokio::test]
async fn backup_snapshots_the_live_database() {
    let Some(test) = boot_seeded().await else { return };
    let config = &test.booted.app.config;
    backup(config).unwrap();
    let snapshot = rusqlite::Connection::open(config.storage.backup_file()).unwrap();
    let users: i64 = snapshot.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0)).unwrap();
    assert!(users > 0);
    let leftovers: Vec<_> = std::fs::read_dir(&config.storage.backups)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".backup-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[tokio::test]
async fn jobs_run_ad_hoc_work_and_purge_unattached_blobs() {
    use matchbox_db::EventSink;

    let Some(test) = boot_seeded().await else { return };
    let app = test.booted.app.clone();

    let (done, finished) = tokio::sync::oneshot::channel();
    app.jobs.perform_later("Test", async move {
        let _ = done.send(());
        Ok(())
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), finished).await.unwrap().unwrap();

    let storage = app.storage.clone();
    let now = app.clock.now();
    let blob = app
        .db
        .write(move |tx| {
            storage
                .create_and_upload(tx.conn(), b"hello", matchbox_storage::Filename::new("hello.txt"), None, now)
                .map_err(matchbox_db::Error::other)
        })
        .await
        .unwrap();
    let path = app.storage.path_for(&blob);
    assert!(path.exists());

    // Blob 5 is attached to a message: purging it is refused (`InvalidForeignKey`).
    app.jobs.emit(matchbox_db::Event::PurgeBlob { blob_id: 5 });
    app.jobs.emit(matchbox_db::Event::PurgeBlob { blob_id: blob.id });
    let blob_id = blob.id;
    for _ in 0..50 {
        let gone = app.db.read(move |conn| Ok(matchbox_storage::Blob::find(conn, blob_id).unwrap().is_none())).await.unwrap();
        if gone && !path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(!path.exists());
    let attached = app.db.read(|conn| Ok(matchbox_storage::Blob::find(conn, 5).unwrap().is_some())).await.unwrap();
    assert!(attached);

    let Booted { jobs, .. } = test.booted;
    jobs.shutdown(std::time::Duration::from_secs(5)).await;
}

/// Regression: every message create runs rich text (plain text for the search index, mentions)
/// inside the writer's transaction. It once checked out a pooled reader for that, so with as many
/// concurrent posts as readers, the writer waited on a reader while the readers' holders waited
/// on the writer, and the server stopped answering for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_message_posts_all_complete() {
    let Some(test) = boot_seeded().await else { return };
    let router = test.booted.router.clone();
    let session = &vectors().sessions[0];
    let user_name = session.user_name.clone();
    let room_id: i64 = test
        .booted
        .app
        .db
        .read(move |conn| {
            Ok(conn.query_row(
                r#"SELECT "memberships"."room_id" FROM "memberships" JOIN "users" ON "users"."id" = "memberships"."user_id"
                   JOIN "rooms" ON "rooms"."id" = "memberships"."room_id"
                   WHERE "users"."name" = ? AND "rooms"."type" = 'Rooms::Open' ORDER BY "rooms"."id" LIMIT 1"#,
                [user_name],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();

    let page = send(&router, get_with_cookie(&format!("/rooms/{room_id}"), &session.cookie_header)).await;
    assert_eq!(page.status, StatusCode::OK);
    let cookie = session.cookie_header.clone();

    let posts = (0..32).map(|n| {
        let router = router.clone();
        let request = Request::post(format!("/rooms/{room_id}/messages"))
            .header(header::HOST, "campfire.test")
            .header(header::COOKIE, &cookie)
            .header("sec-fetch-site", "same-origin")
            .header(header::ACCEPT, "text/vnd.turbo-stream.html, text/html, application/xhtml+xml")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(format!("message%5Bbody%5D=%3Cp%3EHello+{n}%3C%2Fp%3E&message%5Bclient_message_id%5D=concurrent-{n}")))
            .unwrap();
        tokio::spawn(async move { send(&router, request).await.status })
    });
    let statuses = tokio::time::timeout(std::time::Duration::from_secs(60), futures_util::future::join_all(posts))
        .await
        .expect("concurrent message posts deadlocked");
    for status in statuses {
        assert_eq!(status.unwrap(), StatusCode::OK);
    }

    let after = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        send(&router, get_with_cookie(&format!("/rooms/{room_id}"), &session.cookie_header)),
    )
    .await
    .expect("the server stopped answering");
    assert_eq!(after.status, StatusCode::OK);
}

/// Variants are transformed off the database writer: other writes go through while one is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writes_proceed_while_a_variant_is_transformed() {
    use matchbox_storage::{Blob, Variation};

    let Some(test) = boot_seeded().await else { return };
    let app = test.booted.app.clone();
    let blob = app.db.read(|conn| Ok(Blob::find(conn, 5).unwrap().unwrap())).await.unwrap();
    let variation = app.storage.variation_for(&blob, &Variation::resize_to_limit(37, 37, None)).unwrap();

    let (entered, transforming) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let processing = tokio::spawn({
        let (app, blob, variation) = (app.clone(), blob.clone(), variation.clone());
        async move {
            crate::active_storage::processed_variant_with(&app, blob, variation, move |storage, blob, variation| {
                let _ = entered.send(());
                let _ = released.recv();
                storage.transform_variant(blob, variation)
            })
            .await
        }
    });
    transforming.await.unwrap();

    let write = app.db.write(|tx| Ok(tx.conn().execute("UPDATE accounts SET name = name", [])?));
    tokio::time::timeout(std::time::Duration::from_secs(5), write).await.expect("the write waited on the transform").unwrap();

    release.send(()).unwrap();
    let image = processing.await.unwrap().unwrap();
    assert!(app.storage.path_for(&image).exists());
    let storage = app.storage.clone();
    let recorded = app.db.read(move |conn| Ok(storage.existing_variant(conn, &blob, &variation).unwrap())).await.unwrap();
    assert_eq!(recorded.map(|b| b.id), Some(image.id));
}

#[tokio::test]
async fn blob_byte_ranges_are_served_from_the_file() {
    let Some(test) = boot_seeded().await else { return };
    let router = &test.booted.router;
    let proxy_path = vectors().blobs[0].redirect_path.replacen("/redirect/", "/proxy/", 1);
    let blob = test.booted.app.db.read(|conn| Ok(matchbox_storage::Blob::find(conn, 5).unwrap().unwrap())).await.unwrap();
    let file = std::fs::read(test.booted.app.storage.path_for(&blob)).unwrap();
    let ranged =
        |range: &str| Request::get(&proxy_path).header(header::HOST, "campfire.test").header("range", range).body(Body::empty()).unwrap();

    let single = send(router, ranged("bytes=100-")).await;
    assert_eq!(single.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(single.header("content-range"), Some(format!("bytes 100-{}/{}", file.len() - 1, file.len()).as_str()));
    assert_eq!(single.header("content-length"), Some((file.len() - 100).to_string().as_str()));
    assert_eq!(single.body, &file[100..]);

    let multiple = send(router, ranged("bytes=0-9,20-29")).await;
    assert_eq!(multiple.status, StatusCode::PARTIAL_CONTENT);
    let boundary = multiple.header("content-type").unwrap().strip_prefix("multipart/byteranges; boundary=").unwrap().to_string();
    let part = |start: usize, end: usize| {
        let mut part = format!("\r\n--{boundary}\r\nContent-Type: image/jpeg\r\nContent-Range: bytes {start}-{end}/{}\r\n\r\n", file.len())
            .into_bytes();
        part.extend_from_slice(&file[start..=end]);
        part
    };
    let expected = [part(0, 9), part(20, 29), format!("\r\n--{boundary}--\r\n").into_bytes()].concat();
    assert_eq!(multiple.header("content-length"), Some(expected.len().to_string().as_str()));
    assert_eq!(multiple.body, expected);

    let unsatisfiable = send(router, ranged(&format!("bytes={}-", file.len() + 10))).await;
    assert_eq!(unsatisfiable.status, StatusCode::RANGE_NOT_SATISFIABLE);
}

/// `ContentDisposition.format` transliterates the ASCII `filename=` with I18n's whole table, not
/// just Latin-1.
#[tokio::test]
async fn proxied_blobs_name_their_file_like_rails() {
    let Some(test) = boot_seeded().await else { return };
    let router = &test.booted.router;
    test.booted
        .app
        .db
        .write(|tx| Ok(tx.conn().execute("UPDATE active_storage_blobs SET filename = 'Łódź.jpg' WHERE id = 5", [])?))
        .await
        .unwrap();
    let proxy_path = vectors().blobs[0].redirect_path.replacen("/redirect/", "/proxy/", 1);
    let request = |range: Option<&str>| {
        let mut request = Request::get(&proxy_path).header(header::HOST, "campfire.test");
        if let Some(range) = range {
            request = request.header("range", range);
        }
        request.body(Body::empty()).unwrap()
    };

    let expected = Some("inline; filename=\"Lodz.jpg\"; filename*=UTF-8''%C5%81%C3%B3d%C5%BA.jpg");
    let whole = send(router, request(None)).await;
    assert_eq!((whole.status, whole.header("content-disposition")), (StatusCode::OK, expected));
    let ranged = send(router, request(Some("bytes=0-9"))).await;
    assert_eq!((ranged.status, ranged.header("content-disposition")), (StatusCode::PARTIAL_CONTENT, expected));
}

async fn content_dispositions(router: &axum::Router, path: &str) -> (StatusCode, Vec<String>) {
    let reply = send(router, get(path)).await;
    let values = reply.headers.get_all(header::CONTENT_DISPOSITION).iter().map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
    (reply.status, values.collect())
}

/// `send_stream` puts `?disposition=` into Content-Disposition as it is, and Puma writes a value
/// with line breaks line by line, leaving out lines with control characters (probed in the
/// reference, where Thruster answers a DEL with a 502).
#[tokio::test]
async fn proxied_blobs_write_odd_dispositions_as_puma_does() {
    let Some(test) = boot_seeded().await else { return };
    let router = &test.booted.router;
    let proxy_path = vectors().blobs[0].redirect_path.replacen("/redirect/", "/proxy/", 1);
    let with = |disposition: &str| format!("{proxy_path}?disposition={disposition}");

    let (_, inline) = content_dispositions(router, &with("inline")).await;
    let filename = inline[0].strip_prefix("inline").unwrap();
    assert_eq!(content_dispositions(router, &with("x%0Ay")).await, (StatusCode::OK, vec!["x".into(), format!("y{filename}")]));
    assert_eq!(content_dispositions(router, &with("%0Aa")).await, (StatusCode::OK, vec!["".into(), format!("a{filename}")]));
    assert_eq!(content_dispositions(router, &with("x%09y")).await, (StatusCode::OK, vec![format!("x\ty{filename}")]));
    assert_eq!(content_dispositions(router, &with("%C3%A9")).await, (StatusCode::OK, vec![format!("é{filename}")]));
    for dropped in ["x%0Dy", "x%01y", "x%00y", "x%7Fy"] {
        assert_eq!(content_dispositions(router, &with(dropped)).await, (StatusCode::OK, vec![]), "{dropped}");
    }
    assert_eq!(content_dispositions(router, &with("%FF")).await.0, StatusCode::BAD_REQUEST);
}
