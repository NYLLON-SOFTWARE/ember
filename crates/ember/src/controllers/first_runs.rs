//! `FirstRunsController` (reference/app/controllers/first_runs_controller.rb): set up the account
//! and its first administrator.

use ember_db::{Account, FirstRun, PasswordDigest};
use ember_kit::{Ctx, Error, Result, StatusCode, format, halt};
use ember_views::first_runs;

use super::presenters::attachments::{self, Assignment, Record};
use crate::app::AppCtx;
use crate::concerns::{self, Before};
use crate::controllers::presenters::page::framed_page;

/// `allow_unauthenticated_access`, `before_action :prevent_repeats`
pub async fn show(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default().allow_unauthenticated_access()).await?;
    prevent_repeats(c).await?;
    render(c, StatusCode::OK, first_runs::FormValues::default()).await
}

async fn render(c: &mut Ctx, status: StatusCode, values: first_runs::FormValues<'_>) -> Result {
    c.respond_to(&[&format::HTML])?;
    let profile = c
        .request
        .header("X-Ember-Style-Profile")
        .or_else(|| c.request.header("X-Matchbox-Style-Profile"))
        .or_else(|| c.request.header("X-Campfire-Style-Profile"));
    let reload_frame = c.is_turbo_frame_request() && profile != Some("basecoat");
    framed_page!(c, status, ember_assets::StyleProfile::Basecoat, |ctx| first_runs::Show { ctx, values, reload_frame }).await
}

pub async fn create(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default().allow_unauthenticated_access()).await?;
    prevent_repeats(c).await?;

    // params.require(:user).permit(:name, :avatar, :email_address, :password)
    let user = c.params.require("user")?.permit(&ember_kit::permit_keys(&["name", "avatar", "email_address", "password"]));
    let name = user.get("name").and_then(|p| p.to_s());
    let email_address = user.get("email_address").and_then(|p| p.to_s()).unwrap_or_default();
    let password = user.get("password").and_then(|p| p.to_s()).unwrap_or_default();
    // KRO setup enforces the form's minimum before hashing or staging uploaded files.
    if password.chars().count() < 8 {
        let values = first_runs::FormValues { name: name.as_deref(), email_address: Some(&email_address), password_error: true };
        return render(c, StatusCode::UNPROCESSABLE_ENTITY, values).await;
    }
    let avatar = Assignment::from_params(&user, "avatar")?;
    // users.name is NOT NULL: Rails raises ActiveRecord::NotNullViolation.
    let Some(name) = name else { return Err(Error::internal(anyhow::anyhow!("NOT NULL constraint failed: users.name"))) };
    let avatar = avatar.stage(c.app()).await?;
    let password_digest = PasswordDigest::hash(password, c.app().db.env().bcrypt_cost).await.map_err(Error::internal)?;

    let result = c
        .app()
        .db
        .write(move |tx| {
            let administrator = FirstRun::create(tx, &name, &email_address, password_digest)?;
            let pending = attachments::assign(tx, Record::user(administrator.id), "avatar", avatar)?;
            Ok((administrator, pending))
        })
        .await;

    let root = c.url_for(&ember_routes::root());
    match result {
        Ok((administrator, pending)) => {
            attachments::analyze_later(c.app(), pending);
            concerns::start_new_session_for(c, administrator).await?;
            c.redirect_to(&root)
        }
        // rescue ActiveRecord::RecordNotUnique
        Err(error) if error.is_record_not_unique() => c.redirect_to(&root),
        Err(error) => Err(Error::internal(error)),
    }
}

/// `redirect_to root_url if Account.any?`
async fn prevent_repeats(c: &mut Ctx) -> Result<()> {
    let any = c.app().read(|conn| Ok(Account::count(conn)? > 0)).await?;
    if any {
        let root = c.url_for(&ember_routes::root());
        return halt(c.redirect_to(&root)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use sha2::{Digest, Sha256};
    use tower::ServiceExt;

    use crate::app::{Booted, boot};
    use crate::config::Config;

    async fn empty_app() -> (Booted, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::from_lookup(|name| match name {
            "SECRET_KEY_BASE" => Some("first-run-assets-test-secret".repeat(4)),
            "DISABLE_SSL" => Some("1".into()),
            "EMBER_STORAGE_PATH" => Some(dir.path().to_string_lossy().into_owned()),
            _ => None,
        })
        .unwrap();
        (boot(config).await.unwrap(), dir)
    }

    fn setup_request(password: Option<&str>) -> Request<Body> {
        let mut body = "user[name]=Ada+%3CAdmin%3E&user[email_address]=ada%40example.test".to_string();
        if let Some(password) = password {
            body.push_str(&format!("&user[password]={}", ruby_compat::cgi_escape(password)));
        }
        Request::builder()
            .method("POST")
            .uri("/first_run")
            .header("host", "campfire.test")
            .header("sec-fetch-site", "same-origin")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "text/vnd.turbo-stream.html, text/html, application/xhtml+xml")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn new_installs_use_ember_icons_before_and_after_setup() {
        let (app, _dir) = empty_app().await;
        let icons = [
            ("192x192", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../assets/overrides/logos/app-icon-192.png")).as_slice()),
            ("512x512", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../assets/overrides/logos/app-icon.png")).as_slice()),
        ];
        for installed in [false, true] {
            let response = app
                .router
                .clone()
                .oneshot(Request::builder().uri("/webmanifest.json").header("host", "campfire.test").body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let manifest: serde_json::Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
            assert_eq!(manifest["name"], "Ember");
            let legacy_etag = app
                .app
                .db
                .read(|conn| {
                    Ok(ember_db::Account::first(conn)?.map(|account| {
                        let key = crate::controllers::presenters::cache_key_with_version("accounts", account.id, account.updated_at.jiff());
                        format!("W/\"{}\"", hex::encode(&Sha256::digest(key.as_bytes())[..16]))
                    }))
                })
                .await
                .unwrap();
            for (icon, (size, expected)) in manifest["icons"].as_array().unwrap().iter().zip(icons) {
                assert_eq!(icon["sizes"], size);
                let url = icon["src"].as_str().unwrap();
                let response = app
                    .router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri(url)
                            .header("host", "campfire.test")
                            .header("if-none-match", legacy_etag.as_deref().unwrap_or_default())
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(response.headers()["content-type"], "image/png");
                let etag = response.headers()["etag"].clone();
                assert_eq!(to_bytes(response.into_body(), usize::MAX).await.unwrap().as_ref(), expected);
                let cached = app
                    .router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri(url)
                            .header("host", "campfire.test")
                            .header("if-none-match", etag)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
            }
            if !installed {
                let response = app.router.clone().oneshot(setup_request(Some("12345678"))).await.unwrap();
                assert_eq!(response.status(), StatusCode::FOUND);
            }
        }
    }

    #[tokio::test]
    async fn password_minimum_is_enforced_before_setup_even_without_browser_validation() {
        let (app, _dir) = empty_app().await;
        for password in [None, Some(""), Some("short"), Some("1234567"), Some("éééé"), Some("🔐🔐🔐🔐")] {
            let response = app.router.clone().oneshot(setup_request(password)).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let html = String::from_utf8(to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
            assert!(html.contains("Password must be at least 8 characters."));
            assert!(html.contains("value=\"Ada &lt;Admin&gt;\""));
            assert!(html.contains("value=\"ada@example.test\""));
            assert!(html.contains("aria-invalid=\"true\""));
            let password_input = html.split('<').find(|tag| tag.starts_with("input ") && tag.contains("id=\"user_password\"")).unwrap();
            assert!(!password_input.contains("value="), "never echo a submitted password");
        }
        let counts = app
            .app
            .db
            .read(|conn| {
                Ok(conn.query_row(
                    "SELECT (SELECT COUNT(*) FROM accounts), (SELECT COUNT(*) FROM users), (SELECT COUNT(*) FROM sessions), (SELECT COUNT(*) FROM active_storage_blobs)",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(counts, (0, 0, 0, 0), "rejected requests have no setup side effects");

        let response = app.router.clone().oneshot(setup_request(Some("12345678"))).await.unwrap();
        assert_eq!(response.status(), StatusCode::FOUND, "exactly eight characters can complete setup");
        assert_eq!(app.app.db.read(ember_db::Account::count).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn setup_uses_the_same_basecoat_assets_in_html_and_preloads() {
        let (app, _dir) = empty_app().await;
        let response = app
            .router
            .oneshot(Request::builder().uri("/first_run").header("host", "campfire.test").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-ember-style-profile"], "basecoat");
        assert!(!response.headers().contains_key("x-matchbox-style-profile"));
        assert!(!response.headers().contains_key("x-campfire-style-profile"));
        let preload = response.headers()["link"].to_str().unwrap().to_owned();
        let html = String::from_utf8(to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
        let css = ember_assets::asset_path("basecoat/app.css");
        assert!(html.contains(&css));
        assert!(preload.contains(&css));
        assert!(!html.contains(&ember_assets::asset_path("signup.css")));
        assert!(!preload.contains(&ember_assets::asset_path("signup.css")));
        assert!(html.contains("enctype=\"multipart/form-data\""));
        assert!(!html.contains("name=\"turbo-visit-control\""));
    }

    #[tokio::test]
    async fn cross_profile_frames_promote_to_full_navigation() {
        let (app, _dir) = empty_app().await;
        for header in ["x-ember-style-profile", "x-matchbox-style-profile", "x-campfire-style-profile"] {
            for (profile, reload) in [("legacy", true), ("basecoat", false)] {
                let response = app
                    .router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri("/first_run")
                            .header("host", "campfire.test")
                            .header("turbo-frame", "setup")
                            .header(header, profile)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(response.headers()["x-ember-style-profile"], "basecoat");
                if header != "x-ember-style-profile" {
                    assert_eq!(response.headers()[header], "basecoat");
                }
                assert!(!response.headers().contains_key("link"));
                let html = String::from_utf8(to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
                assert_eq!(html.contains("name=\"turbo-visit-control\" content=\"reload\""), reload);
            }
        }
    }

    #[tokio::test]
    async fn current_style_header_takes_precedence_over_legacy_headers() {
        let (app, _dir) = empty_app().await;
        let response = app
            .router
            .oneshot(
                Request::builder()
                    .uri("/first_run")
                    .header("host", "campfire.test")
                    .header("turbo-frame", "setup")
                    .header("x-ember-style-profile", "basecoat")
                    .header("x-matchbox-style-profile", "legacy")
                    .header("x-campfire-style-profile", "legacy")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let html = String::from_utf8(to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
        assert!(!html.contains("name=\"turbo-visit-control\" content=\"reload\""));
    }
}
