//! What the HTTP layer logs. A binary of its own: tracing caches whether anything listens at each
//! log statement, and another test reaching one while this test's subscriber is being set up can
//! leave it silenced for good.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body as AxumBody;
use axum::http::{Request, header};
use ember_kit::{Ctx, Kit, KitConfig, Result, StatusCode, TestClock, action};
use rails_compat::Secrets;
use tower::ServiceExt;

async fn echo(c: &mut Ctx) -> Result {
    Ok(c.head(StatusCode::OK))
}

async fn fail(_: &mut Ctx) -> Result {
    let cause = std::io::Error::other("disk full");
    Err(ember_kit::Error::internal(anyhow::Error::new(cause).context("saving the upload")))
}

fn app() -> Router {
    let router = Router::new()
        .route("/echo/{id}", ember_kit::get(echo).post(action(echo)))
        .route("/first_run/access", ember_kit::get(echo).post(action(echo)))
        .route("/first_run", ember_kit::get(echo).post(action(echo)))
        .route("/fail", ember_kit::get(fail));
    let clock = Arc::new(TestClock::frozen_at("2024-06-01T12:00:00Z".parse().unwrap()));
    ember_kit::app(router, Kit::new(KitConfig::default(), Arc::new(Secrets::new("test-secret")), clock, ()))
}

/// One test, not two: see the note at the top.
#[tokio::test]
async fn errors_are_logged_as_they_were_raised() {
    let logs = Logs::default();
    let _guard = tracing::subscriber::set_default(logs.subscriber());
    let query = Request::get("/echo/1?a=%").body(AxumBody::empty()).unwrap();
    let form =
        Request::post("/echo/1").header(header::CONTENT_TYPE, "application/x-www-form-urlencoded").body(AxumBody::from("b=%")).unwrap();
    for request in [query, form] {
        assert_eq!(app().oneshot(request).await.unwrap().status(), StatusCode::BAD_REQUEST);
    }
    let failing = Request::get("/fail").body(AxumBody::empty()).unwrap();
    assert_eq!(app().oneshot(failing).await.unwrap().status(), StatusCode::INTERNAL_SERVER_ERROR);

    for path in ["/first_run", "/first_run/access"] {
        let query = Request::get(format!("{path}?token=PRIVATE_SETUP_CREDENTIAL%")).body(AxumBody::empty()).unwrap();
        let body = Request::post(path)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(AxumBody::from("token=PRIVATE_SETUP_CREDENTIAL%"))
            .unwrap();
        for request in [query, body] {
            assert_eq!(app().oneshot(request).await.unwrap().status(), StatusCode::BAD_REQUEST);
        }
    }
    let text = logs.text();
    assert!(!text.contains("PRIVATE_SETUP_CREDENTIAL"), "{text}");
    assert_eq!(text.matches("request rejected error=400 Bad Request").count(), 4, "{text}");
    // Malformed params, as they were rejected.
    assert_eq!(text.matches("request rejected error=bad request: invalid %-encoding (%)").count(), 2, "{text}");
    assert!(!text.contains("bad request: bad request"), "{text}");
    // A failure, with what caused it.
    assert!(text.contains("request failed error=saving the upload: disk full path=\"/fail\""), "{text}");
}

/// Log lines written on this thread (the test runtime's only one) while the guard is held.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl Logs {
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        let logs = self.clone();
        tracing_subscriber::fmt().with_ansi(false).with_writer(move || logs.clone()).finish()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
