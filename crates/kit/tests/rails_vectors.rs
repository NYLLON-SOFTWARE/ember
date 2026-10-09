//! Old-browser-tab continuity: a session cookie and a signed cookie issued by the reference Rails
//! app (`vectors/rails_compat.json`) are accepted by the kit, and what the kit writes back decodes
//! to the same session. Forgery protection is by `Sec-Fetch-Site` rather
//! than Rails' tokens, so a tab opened before an upgrade keeps working without one.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, header};
use ember_kit::{Ctx, Kit, KitConfig, Result, StatusCode, TestClock, action};
use serde_json::{Value, json};
use tower::ServiceExt;

fn vectors() -> Value {
    serde_json::from_str(include_str!("../../../vectors/rails_compat.json")).unwrap()
}

async fn create_session(c: &mut Ctx) -> Result {
    c.verify_authenticity_token()?;
    let session = c.session();
    let body = json!({ "id": session.id(), "csrf": session.get("_csrf_token") });
    session.insert("return_to_after_authenticating", "/rooms/1");
    c.json(StatusCode::OK, &body)
}

async fn whoami(c: &mut Ctx) -> Result {
    let token = c.cookies.signed("session_token");
    c.json(StatusCode::OK, &json!({ "token": token }))
}

fn app(vectors: &Value) -> (Router, Arc<rails_compat::Secrets>) {
    let secrets = Arc::new(rails_compat::Secrets::new(vectors["secret_key_base"].as_str().unwrap()));
    let now = vectors["now"].as_str().unwrap().parse().unwrap();
    let kit = Kit::new(KitConfig::default(), secrets.clone(), Arc::new(TestClock::frozen_at(now)), ());
    let router = Router::new().route("/session", action_post()).route("/whoami", ember_kit::get(whoami));
    (ember_kit::app(router, kit), secrets)
}

fn action_post() -> axum::routing::MethodRouter<Kit> {
    axum::routing::post(action(create_session))
}

async fn post_session(app: &Router, cookie: &str, site: &str, token: Option<&str>, origin: Option<&str>) -> axum::response::Response {
    let mut request =
        Request::post("/session").header(header::HOST, "localhost:3000").header(header::COOKIE, cookie).header("sec-fetch-site", site);
    if let Some(origin) = origin {
        request = request.header(header::ORIGIN, origin);
    }
    let body = match token {
        Some(token) => {
            request = request.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            format!("authenticity_token={}", ember_kit::cookies::escape(token))
        }
        None => String::new(),
    };
    app.clone().oneshot(request.body(Body::from(body)).unwrap()).await.unwrap()
}

#[tokio::test]
async fn rails_sessions_carry_over() {
    let vectors = vectors();
    let session = &vectors["session"];
    let (app, secrets) = app(&vectors);
    let cookie = format!("_campfire_session={}", ember_kit::cookies::escape(session["session_cookie_raw"].as_str().unwrap()));
    let form_token = session["session_form_token"].as_str().unwrap();

    // A form from a page Rails rendered still posts its token; it's ignored, not required.
    let from_old_tab = post_session(&app, &cookie, "same-origin", Some(form_token), None).await;
    assert_eq!(from_old_tab.status(), StatusCode::OK);
    let set_cookie = from_old_tab.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().to_string();
    let body = axum::body::to_bytes(from_old_tab.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["id"], session["session"]["session_id"]);
    assert_eq!(body["csrf"], session["session"]["_csrf_token"], "Rails' token stays in the session, unused");

    // What we write back after a change is the same session plus the change, in Rails' format.
    let raw = set_cookie.strip_prefix("_campfire_session=").unwrap().split(';').next().unwrap();
    let raw = rails_compat::cookies::unescape(raw);
    let now = vectors["now"].as_str().unwrap().parse().unwrap();
    let decoded = rails_compat::cookies::decrypt(&secrets, "_campfire_session", &raw, now).unwrap();
    let mut expected = session["session"].clone();
    expected["return_to_after_authenticating"] = "/rooms/1".into();
    assert_eq!(decoded, expected);
    assert!(set_cookie.ends_with("; path=/; expires=Mon, 01 Jan 2046 12:00:00 GMT; httponly; samesite=lax"));

    assert_eq!(post_session(&app, &cookie, "same-origin", None, None).await.status(), StatusCode::OK);

    // A valid Rails token doesn't make a cross-site request acceptable.
    let cross_site = post_session(&app, &cookie, "cross-site", Some(form_token), None).await;
    assert_eq!(cross_site.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let cross_origin = post_session(&app, &cookie, "same-origin", None, Some("https://evil.example")).await;
    assert_eq!(cross_origin.status().as_u16(), session["post_with_cross_origin_status"].as_u64().unwrap() as u16);
}

#[tokio::test]
async fn rails_signed_session_token_cookie_is_read() {
    let vectors = vectors();
    let session = &vectors["session"];
    let (app, _) = app(&vectors);
    let cookie = format!("session_token={}", ember_kit::cookies::escape(session["session_token_raw"].as_str().unwrap()));
    let request = Request::get("/whoami").header(header::COOKIE, cookie).body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["token"], session["session_token_value"]);
}
