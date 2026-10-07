//! `Users::AvatarsController` (reference/app/controllers/users/avatars_controller.rb): a user's
//! avatar by its signed avatar token — the uploaded image's `:square` variant, the default bot
//! avatar, or an SVG of their initials.

use askama::Template;
use matchbox_db::User;
use matchbox_kit::{Ctx, Error, ExpiresIn, Freshness, Result, SendOptions, format, halt};
use matchbox_storage::Variation;
use matchbox_views::users::AvatarSvg;

use crate::app::AppCtx;
use crate::concerns::{self, Before};
use crate::controllers::presenters::attachments::{self, Record};
use crate::controllers::presenters::{self, cache_key_with_version};

/// `ActionView::Digestor.digest(name: "users/avatars/show", ...)`: the SHA256 (truncated) of
/// `show.svg.erb`'s source plus "-" (it renders nothing else). `EtagWithTemplateDigest` adds it
/// whenever the template can be found for the request's formats.
const TEMPLATE_DIGEST: &str = "d500db55e2a67222018ef0156839c3c9";

/// `expires_in 30.minutes, public: true, stale_while_revalidate: 1.week`
const MAX_AGE: u64 = 30 * 60;
const STALE_WHILE_REVALIDATE: u64 = 7 * 24 * 60 * 60;

pub async fn show(c: &mut Ctx) -> Result {
    c.use_live_response(); // `include ActiveStorage::Streaming`
    concerns::before_actions(c, Before::default()).await?;
    let user = from_avatar_token(c).await?;

    let freshness = Freshness {
        etag: Some(cache_key_with_version("users", user.id, user.updated_at.jiff())),
        template: template_found(c).then(|| TEMPLATE_DIGEST.to_string()),
        ..Freshness::default()
    };
    if let Some(not_modified) = c.fresh_when(freshness) {
        return Ok(not_modified);
    }
    c.expires_in(MAX_AGE, ExpiresIn { public: true, stale_while_revalidate: Some(STALE_WHILE_REVALIDATE), ..ExpiresIn::default() });

    if let Some(variant) = avatar_variant(c, &user).await? {
        let path = c.app().storage.service.path_for(&variant.key);
        c.send_file(path, SendOptions::inline("image/webp"))
    } else if user.is_bot() {
        render_default_bot(c)
    } else {
        render_initials(c, &user)
    }
}

/// `Current.user.avatar.destroy`, then back to the profile.
pub async fn destroy(c: &mut Ctx) -> Result {
    c.use_live_response(); // `include ActiveStorage::Streaming`
    concerns::before_actions(c, Before::default()).await?;
    let user_id = concerns::require_current_user(c)?.id;
    c.app().write(move |tx| attachments::destroy(tx, Record::user(user_id), "avatar")).await?;
    let location = c.url_for(&matchbox_routes::user_profile());
    c.redirect_to(&location)
}

/// `User.from_avatar_token(params[:user_id])`: a bad signature is `head :not_found`
/// (`rescue_from ActiveSupport::MessageVerifier::InvalidSignature`); a valid one for a missing
/// user is `ActiveRecord::RecordNotFound`.
async fn from_avatar_token(c: &mut Ctx) -> Result<User> {
    let token = c.param_str("user_id").unwrap_or_default().to_string();
    let Some(user_id) = presenters::accounts::user_id_from_avatar_token(&c.app().secrets, &token, c.now()) else {
        return halt(c.head(matchbox_kit::StatusCode::NOT_FOUND));
    };
    c.app().read(move |conn| User::find_by_id(conn, user_id)).await?.ok_or(Error::NotFound)
}

/// Whether `lookup_context.find_all("show", ["users/avatars", ...])` finds `show.svg.erb` for the
/// request's formats: `*/*` or svg, or no registered format at all (`Accept: image/*` parses to
/// none, and an empty `formats=` falls back to every format).
fn template_found(c: &mut Ctx) -> bool {
    let Ok(formats) = c.formats() else { return false };
    formats.is_empty() || formats.iter().any(|format| **format == format::ALL || **format == format::SVG)
}

/// `avatar.variant(:square).processed if avatar.variable?` (`resize_to_limit: [512, 512], format: :webp`).
async fn avatar_variant(c: &Ctx, user: &User) -> Result<Option<matchbox_storage::Blob>> {
    attachments::processed_variant(c.app(), Record::user(user.id), "avatar", Variation::resize_to_limit(512, 512, Some("webp"))).await
}

/// `send_file Rails.root.join("app/assets/images/default-bot-avatar.svg"), content_type: "image/svg+xml", disposition: :inline`
fn render_default_bot(c: &mut Ctx) -> Result {
    let path = asset_file("default-bot-avatar.svg")?;
    c.send_file(path, SendOptions::inline("image/svg+xml"))
}

/// `render formats: :svg` (`users/avatars/show.svg.erb`).
fn render_initials(c: &mut Ctx, user: &User) -> Result {
    let svg = AvatarSvg { user_id: user.id, initials: user.initials() }.render().map_err(Error::internal)?;
    // `Vary: Accept` like any render when the format came from a non-browser `Accept` (e.g.
    // `image/*`); a browser's image `Accept` ends in `*/*`, so it usually doesn't apply.
    Ok(c.render_as(matchbox_kit::StatusCode::OK, "image/svg+xml; charset=utf-8", svg))
}

/// A file under `app/assets/images` (embedded by matchbox_assets) on disk, for `send_file`: it's
/// written once per process to a private directory under its own name, so the response carries
/// the same filename and, like Rails' file bodies, gets no `Rack::ETag` digest. The lock makes the
/// first write race-free: concurrent first requests would otherwise see a half-written file.
pub fn asset_file(logical_path: &str) -> Result<std::path::PathBuf> {
    static DIR: std::sync::Mutex<Option<tempfile::TempDir>> = std::sync::Mutex::new(None);
    let mut dir = DIR.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if dir.is_none() {
        *dir = Some(tempfile::Builder::new().prefix("matchbox-assets-").tempdir()?);
    }
    let path = dir.as_ref().expect("initialized above").path().join(logical_path);
    if !path.exists() {
        let url = matchbox_assets::asset_path(logical_path);
        let request = matchbox_assets::StaticRequest { method: "GET", path: &url, ..Default::default() };
        let data = matchbox_assets::serve(&request).ok_or_else(|| Error::internal(anyhow::anyhow!("missing asset {logical_path}")))?.body;
        let parent = path.parent().expect("joined onto a directory");
        std::fs::create_dir_all(parent)?;
        let mut partial = tempfile::NamedTempFile::new_in(parent)?;
        std::io::Write::write_all(&mut partial, &data)?;
        partial.persist(&path).map_err(|error| Error::internal(error.error))?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_first_requests_all_get_the_whole_file() {
        let threads: Vec<_> =
            (0..16).map(|_| std::thread::spawn(|| std::fs::read(asset_file("default-bot-avatar.svg").unwrap()).unwrap())).collect();
        let contents: Vec<Vec<u8>> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert!(!contents[0].is_empty());
        assert!(contents.iter().all(|c| c == &contents[0]));
    }
}
