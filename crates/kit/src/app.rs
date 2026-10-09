//! The shared, per-process part of the HTTP layer: configuration, secrets, clock and app state.

use std::any::Any;
use std::sync::Arc;

use axum::http::{HeaderName, HeaderValue};
use rails_compat::Secrets;

use crate::clock::SharedClock;
use crate::exceptions::ErrorPages;
use crate::request::ProxyConfig;
use crate::session::SessionConfig;

#[derive(Debug, Clone)]
pub struct KitConfig {
    pub proxy: ProxyConfig,
    /// `config.force_ssl`: redirect plain HTTP to HTTPS, send HSTS, flag cookies `secure`.
    pub force_ssl: bool,
    /// The HSTS header value sent with `force_ssl` (`ssl_options = { hsts: { subdomains: true } }`).
    pub hsts: String,
    pub session: SessionConfig,
    /// `forgery_protection_origin_check` (on since `load_defaults 5.0`).
    pub forgery_protection_origin_check: bool,
    /// `action_dispatch.default_headers` (`load_defaults 7.1`).
    pub default_headers: Vec<(HeaderName, HeaderValue)>,
    /// `public/404.html`, `422.html`, `500.html`, ... (`ActionDispatch::PublicExceptions`).
    pub error_pages: ErrorPages,
    /// Largest request body accepted; `None` is unlimited, like Puma.
    pub max_body_bytes: Option<usize>,
}

impl Default for KitConfig {
    fn default() -> Self {
        Self {
            proxy: ProxyConfig::default(),
            force_ssl: false,
            hsts: "max-age=63072000; includeSubDomains".into(),
            session: SessionConfig::default(),
            forgery_protection_origin_check: true,
            default_headers: rails_default_headers(),
            error_pages: ErrorPages::default(),
            max_body_bytes: None,
        }
    }
}

impl KitConfig {
    /// Ember's production settings: `assume_ssl` and `force_ssl` unless `DISABLE_SSL` is set
    /// (`reference/config/environments/production.rb`).
    pub fn production(disable_ssl: bool) -> Self {
        let mut config = Self::default();
        config.proxy.assume_ssl = !disable_ssl;
        config.force_ssl = !disable_ssl;
        config
    }
}

pub fn rails_default_headers() -> Vec<(HeaderName, HeaderValue)> {
    [
        ("x-frame-options", "SAMEORIGIN"),
        ("x-xss-protection", "0"),
        ("x-content-type-options", "nosniff"),
        ("x-permitted-cross-domain-policies", "none"),
        ("referrer-policy", "strict-origin-when-cross-origin"),
    ]
    .into_iter()
    .map(|(k, v)| (HeaderName::from_static(k), HeaderValue::from_static(v)))
    .collect()
}

/// Axum router state: everything a request needs that outlives it. Cheap to clone.
#[derive(Clone)]
pub struct Kit {
    pub(crate) inner: Arc<KitInner>,
}

pub(crate) struct KitInner {
    pub config: KitConfig,
    pub secrets: Arc<Secrets>,
    pub clock: SharedClock,
    pub state: Arc<dyn Any + Send + Sync>,
}

impl std::fmt::Debug for Kit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kit").field("config", &self.inner.config).finish()
    }
}

impl Kit {
    /// `state` is the application's own state (database handles etc.), reachable from actions
    /// with [`crate::Ctx::state`].
    pub fn new<S: Send + Sync + 'static>(config: KitConfig, secrets: Arc<Secrets>, clock: SharedClock, state: S) -> Self {
        Self { inner: Arc::new(KitInner { config, secrets, clock, state: Arc::new(state) }) }
    }

    pub fn config(&self) -> &KitConfig {
        &self.inner.config
    }

    /// What cookies are signed and encrypted with.
    pub fn secrets(&self) -> &Arc<Secrets> {
        &self.inner.secrets
    }

    pub fn clock(&self) -> &SharedClock {
        &self.inner.clock
    }

    pub(crate) fn error_pages(&self) -> &ErrorPages {
        &self.inner.config.error_pages
    }

    pub fn state<S: Send + Sync + 'static>(&self) -> &S {
        self.inner.state.downcast_ref::<S>().expect("Kit state has a different type")
    }
}
