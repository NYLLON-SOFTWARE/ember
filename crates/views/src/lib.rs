//! Askama templates mirroring `reference/app/views`, one template file per ERB file at the same
//! relative path under `templates/`. Views take plain view-model structs defined here, never
//! database rows, so this crate doesn't depend on `matchbox_db`. Rich text arrives pre-rendered
//! as sanitized HTML. Every template renders with the per-request [`ViewContext`] below.

pub mod accounts;
pub mod autocompletable;
pub mod first_runs;
pub mod fragment_cache;
pub mod helpers;
pub mod layouts;
pub mod messages;
pub mod pwa;
pub mod recorded;
pub mod rooms;
pub mod searches;
pub mod sessions;
pub mod sized;
pub mod users;
pub mod welcome;

/// Per-request state every page needs: what `ApplicationController`, the layout and the
/// helpers read from `Current`, `request`, `flash` and the session.
pub struct ViewContext<'a> {
    pub current_user: Option<CurrentUser>,
    pub account: AccountSummary,
    pub flash_notice: Option<String>,
    pub flash_alert: Option<String>,
    /// `ApplicationPlatform` facts derived from the user agent.
    pub platform: Platform,
    /// `Rails.configuration.x.vapid.public_key`; `None` omits the meta tag's content attribute.
    pub vapid_public_key: Option<String>,
    /// Resolves a logical asset path ("matchbox-icon.png") to its digested URL.
    pub asset_path: &'a dyn Fn(&str) -> String,
    /// The `<script type="importmap">` + modulepreload tags (`javascript_importmap_tags`).
    pub importmap_tags: &'a str,
    /// `<link rel="stylesheet">` tags for `stylesheet_link_tag :all, "data-turbo-track": "reload"`.
    pub stylesheet_tags: &'a str,
    /// The account's custom CSS, if any (`custom_styles_tag`).
    pub custom_styles: Option<String>,
    /// `script_aware_action_cable_meta_tag` content: script_name + "/cable".
    pub cable_url: String,
    /// `request.base_url` ("http://campfire.test"), for the `*_url` helpers.
    pub base_url: String,
    /// `request.url`, compared against the referrer by `link_back`.
    pub request_url: String,
    /// `request.referrer`.
    pub referrer: Option<String>,
    /// Id of `last_room_visited` (`TrackedRoomVisit`): the `last_room` cookie's room if the user
    /// is a member, else `Current.user.rooms.original`. `None` links back to the root.
    pub last_room_visited_id: Option<i64>,
    /// `Rails.application.config.app_version` (APP_VERSION, GIT_REVISION or "0").
    pub app_version: String,
}

impl ViewContext<'_> {
    pub fn asset(&self, logical_path: &str) -> String {
        (self.asset_path)(logical_path)
    }

    /// `root_url`, `session_url`, `join_url(...)`: base URL + path.
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    pub fn can_administer(&self) -> bool {
        self.current_user.as_ref().is_some_and(|user| user.administrator)
    }

    pub fn current_user_id(&self) -> Option<i64> {
        self.current_user.as_ref().map(|user| user.id)
    }

    /// `Current.user == user`.
    pub fn is_current_user(&self, user_id: impl std::borrow::Borrow<i64>) -> bool {
        self.current_user_id() == Some(*user_id.borrow())
    }
}

#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub id: i64,
    pub name: String,
    pub administrator: bool,
    pub bot: bool,
    /// `fresh_user_avatar_path(Current.user)`.
    pub avatar_url: String,
}

#[derive(Clone, Debug)]
pub struct AccountSummary {
    pub name: String,
    pub hide_translation_buttons: bool,
    /// `fresh_account_logo_path` (no size).
    pub logo_url: String,
    /// `Current.account.logo.attached?` (adds the `account-has-logo` body class).
    pub has_logo: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Platform {
    pub ios: bool,
    pub android: bool,
    pub mac: bool,
    pub windows: bool,
    pub chrome: bool,
    pub firefox: bool,
    pub safari: bool,
    pub edge: bool,
    pub mobile: bool,
    pub desktop: bool,
    /// `ApplicationPlatform#apple_messages?`.
    pub apple_messages: bool,
    /// `user_agent.browser` from the useragent gem ("Chrome", "Safari", "Firefox", "Edge", ...).
    pub browser: String,
    /// `ApplicationPlatform#operating_system` ("macOS", "Windows", "iPhone", ...).
    pub operating_system: String,
}
