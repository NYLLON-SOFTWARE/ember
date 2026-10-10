//! `Ctx`: everything a controller action touches, in place of a Rails controller instance.

use std::path::Path;
use std::sync::Arc;

use axum::body::Bytes;
use axum::http::header::{self, HeaderName, HeaderValue};
use axum::http::{Extensions, HeaderMap, Method, StatusCode};
use jiff::Timestamp;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::app::Kit;
use crate::clock::{self, SharedClock};
use crate::cookies::CookieJar;
use crate::deflater::BodyDigest;
use crate::format::{self, Format, InvalidMimeType, NegotiationInput};
use crate::params::{Param, ParamMap};
use crate::request::Request;
use crate::response::{self, Body, CacheControl, ExpiresIn, Response, SendBody, SendOptions};
use crate::session::{Flash, Session};
use crate::{Error, Result};

pub struct Ctx {
    pub request: Request,
    /// `params`: body params, then query params, then path params, merged like Rails.
    pub params: ParamMap,
    pub query_params: ParamMap,
    pub request_params: ParamMap,
    pub path_params: ParamMap,
    pub cookies: CookieJar,
    /// Response headers set before the response exists (e.g. `X-Version` in a before-action).
    /// Merged into the final response without overriding what it already sets.
    pub headers: HeaderMap,
    /// `response.cache_control`, applied to the final response.
    pub cache_control: CacheControl,
    session: Session,
    flash: Option<Flash>,
    kit: Kit,
    extensions: Extensions,
    marked_for_same_origin_verification: bool,
    formats: Option<std::result::Result<Vec<Format>, InvalidMimeType>>,
    rendered_format: Option<Format>,
    live: bool,
}

/// Options for `redirect_to`.
#[derive(Debug, Clone, Default)]
pub struct Redirect {
    /// Defaults to 302 Found, like Rails.
    pub status: Option<StatusCode>,
    pub notice: Option<String>,
    pub alert: Option<String>,
    pub allow_other_host: bool,
}

/// Validators for `fresh_when` / `stale?`.
#[derive(Debug, Clone, Default)]
pub struct Freshness {
    /// The weak ETag validator, already expanded to a cache key (e.g. `record.cache_key_with_version`).
    pub etag: Option<String>,
    pub strong_etag: Option<String>,
    pub last_modified: Option<Timestamp>,
    pub public: bool,
    /// The template digest `ETagWithTemplateDigest` would add.
    pub template: Option<String>,
}

impl Freshness {
    pub fn etag(validator: impl Into<String>) -> Self {
        Self { etag: Some(validator.into()), ..Self::default() }
    }
}

impl Ctx {
    pub(crate) fn new(
        kit: Kit,
        request: Request,
        path_params: ParamMap,
        query_params: ParamMap,
        request_params: ParamMap,
        cookies: CookieJar,
    ) -> Self {
        let mut params = request_params.clone();
        params.merge(&query_params);
        params.merge(&path_params);
        let session = Session::new(kit.config().session.clone());
        Self {
            request,
            params,
            query_params,
            request_params,
            path_params,
            cookies,
            headers: HeaderMap::new(),
            cache_control: CacheControl::default(),
            session,
            flash: None,
            kit,
            extensions: Extensions::new(),
            marked_for_same_origin_verification: false,
            formats: None,
            rendered_format: None,
            live: false,
        }
    }

    pub fn kit(&self) -> &Kit {
        &self.kit
    }

    /// The application state given to [`Kit::new`].
    pub fn state<S: Send + Sync + 'static>(&self) -> &S {
        self.kit.state::<S>()
    }

    pub fn clock(&self) -> &SharedClock {
        self.kit.clock()
    }

    pub fn now(&self) -> Timestamp {
        self.kit.clock().now()
    }

    // --- Current attributes ----------------------------------------------------------------

    /// Store a per-request value such as the current user or session (`Current.user = ...`).
    pub fn set_current<T: Clone + Send + Sync + 'static>(&mut self, value: T) {
        self.extensions.insert(value);
    }

    pub fn current<T: Clone + Send + Sync + 'static>(&self) -> Option<&T> {
        self.extensions.get::<T>()
    }

    pub fn current_mut<T: Clone + Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.extensions.get_mut::<T>()
    }

    pub fn take_current<T: Clone + Send + Sync + 'static>(&mut self) -> Option<T> {
        self.extensions.remove::<T>()
    }

    // --- Params ------------------------------------------------------------------------------

    pub fn param(&self, key: &str) -> Option<&Param> {
        self.params.get(key)
    }

    pub fn param_str(&self, key: &str) -> Option<&str> {
        self.params.str(key)
    }

    /// `wrap_parameters format: [:json]`: for a JSON request, nest the body params under `key`
    /// unless it's already there. `include` is the model's attribute names when it has a model.
    pub fn wrap_parameters(&mut self, key: &str, include: Option<&[&str]>) {
        let is_json = format::content_mime_type(self.request.content_type()).ok().flatten() == Some(&format::JSON);
        if !is_json || self.params.contains_key(key) {
            return;
        }
        let wrapped: ParamMap = self
            .request_params
            .iter()
            .filter(|(k, _)| match include {
                Some(include) => include.contains(&k.as_str()),
                None => !["authenticity_token", "_method", "utf8"].contains(&k.as_str()),
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.params.insert(key, Param::Hash(wrapped.clone()));
        self.request_params.insert(key, Param::Hash(wrapped));
    }

    // --- Session and flash ---------------------------------------------------------------------

    /// `session`, loaded from the cookie on first use.
    pub fn session(&mut self) -> &mut Session {
        self.session.load(&self.cookies)
    }

    /// `reset_session`: new session id, no data, no flash.
    pub fn reset_session(&mut self) {
        self.session.reset();
        self.flash = None;
    }

    /// `flash`, loaded from the session on first use.
    pub fn flash(&mut self) -> &mut Flash {
        if self.flash.is_none() {
            let stored = self.session().get("flash").cloned();
            self.flash = Some(Flash::from_session_value(stored.as_ref()));
        }
        self.flash.as_mut().unwrap()
    }

    // --- Forgery protection ---------------------------------------------------------------------

    /// The `verify_authenticity_token` before-action, by `Sec-Fetch-Site` rather than tokens (Rails
    /// main's `protect_from_forgery using: :header_only`): pages carry no per-request token, so they
    /// render the same until what they show changes. Browsers send the header on every request to a
    /// secure origin; without it (an old browser, or plain HTTP where browsers don't send it) a
    /// write is only allowed when neither the request nor the app uses SSL, where the
    /// `SameSite=Lax` session cookie and the `Origin` check are the protection.
    pub fn verify_authenticity_token(&mut self) -> Result<()> {
        self.marked_for_same_origin_verification = self.request.is_get();
        if self.request.is_get() || self.request.is_head() {
            return Ok(());
        }
        if !self.valid_request_origin()? {
            let message = format!(
                "HTTP Origin header ({}) didn't match request.base_url ({})",
                self.request.origin().unwrap_or(""),
                self.request.base_url()
            );
            return Err(Error::InvalidAuthenticityToken(message));
        }
        match self.request.header("sec-fetch-site") {
            Some("same-origin" | "same-site") => Ok(()),
            None if !self.request.is_ssl() && !self.kit.config().force_ssl => Ok(()),
            Some("cross-site") => {
                Err(Error::InvalidAuthenticityToken("Sec-Fetch-Site header (cross-site) indicates a cross-site request".into()))
            }
            other => Err(Error::InvalidAuthenticityToken(format!("Sec-Fetch-Site header is missing or invalid ({other:?})"))),
        }
    }

    fn valid_request_origin(&self) -> Result<bool> {
        if !self.kit.config().forgery_protection_origin_check {
            return Ok(true);
        }
        match self.request.origin() {
            Some("null") => Err(Error::InvalidAuthenticityToken("The browser returned a 'null' origin".into())),
            Some(origin) => Ok(origin == self.request.base_url()),
            None => Ok(true),
        }
    }

    // --- Formats -------------------------------------------------------------------------------

    /// `request.formats`; an invalid `Accept` header is a 406 like Rails' `InvalidType`.
    pub fn formats(&mut self) -> Result<Vec<Format>> {
        if self.formats.is_none() {
            self.formats = Some(format::formats(&self.negotiation_input()));
        }
        self.formats.clone().unwrap().map_err(|_| Error::UnknownFormat)
    }

    fn negotiation_input(&self) -> NegotiationInput<'_> {
        NegotiationInput {
            format_param: self.params.str("format"),
            accept: self.request.header("accept"),
            content_type: self.request.content_type(),
            path: self.request.path(),
            xhr: self.request.is_xhr(),
        }
    }

    /// `_set_vary_header`, which every `render` runs (not `head` or `redirect_to`): `Vary: Accept`
    /// when the format came from the `Accept` header, unless the response already varies.
    pub fn set_vary_header(&self, response: Response) -> Response {
        if response.headers.contains_key(header::VARY) || !format::should_apply_vary_header(&self.negotiation_input()) {
            return response;
        }
        response.header(header::VARY, "Accept")
    }

    /// `request.format`: the first format; `None` is Rails' `Mime::NullType`.
    pub fn format(&mut self) -> Result<Option<Format>> {
        Ok(self.formats()?.first().copied())
    }

    /// `respond_to do |format| ... end`: pick the first format the client accepts among
    /// `offered` (in the order the block declares them), or 406 via `UnknownFormat`.
    pub fn respond_to(&mut self, offered: &[Format]) -> Result<Format> {
        let formats = self.formats()?;
        let chosen = format::negotiate(&formats, offered).ok_or(Error::UnknownFormat)?;
        let chosen = if *chosen == format::ALL { offered.first().copied().unwrap_or(&format::HTML) } else { chosen };
        self.rendered_format = Some(chosen);
        Ok(chosen)
    }

    /// The format a render will use: the `respond_to` choice, else the first request format.
    pub fn rendered_format(&mut self) -> Format {
        if let Some(format) = self.rendered_format {
            return format;
        }
        match self.formats().ok().and_then(|f| f.first().copied()) {
            Some(format) if *format != format::ALL => format,
            _ => &format::HTML,
        }
    }

    /// `turbo_frame_request?`
    pub fn is_turbo_frame_request(&self) -> bool {
        self.turbo_frame_request_id().is_some_and(|id| !id.trim().is_empty())
    }

    pub fn turbo_frame_request_id(&self) -> Option<&str> {
        self.request.header("turbo-frame")
    }

    // --- Rendering -----------------------------------------------------------------------------

    /// `render html:` / a template, as `text/html; charset=utf-8`.
    pub fn render_html(&mut self, status: StatusCode, html: impl Into<Bytes>) -> Response {
        self.render_as(status, response::HTML_UTF8, html)
    }

    /// `render` with an explicit content type (`render json:`, `send_data`, ...).
    pub fn render_as(&mut self, status: StatusCode, content_type: &str, body: impl Into<Bytes>) -> Response {
        self.set_vary_header(Response::with_body(status, content_type, body))
    }

    pub fn html(&mut self, html: impl Into<Bytes>) -> Response {
        self.render_html(StatusCode::OK, html)
    }

    /// A rendered template, labelled with *the template's* format: Rails sets `rendered_format`
    /// from the template the lookup found, not from the request, so an `.html.erb`-only action
    /// answers `text/html` even when the `Accept` header prefers `text/vnd.turbo-stream.html`
    /// (every Turbo form submission, and the redirect fetch follows). Pick the template with
    /// [`Ctx::respond_to`] (the implicit render's lookup) when an action has several.
    pub fn render(&mut self, status: StatusCode, template: Format, body: impl Into<Bytes>) -> Response {
        self.rendered_format = Some(template);
        self.render_as(status, &format!("{}; charset=utf-8", template.string), body)
    }

    /// [`Ctx::render`] for a template that recorded where its cached fragments went: its `text`
    /// with each of `fragments` spliced in at its byte offset (see [`Body::spliced`]).
    ///
    /// # Panics
    ///
    /// On offsets [`Body::spliced`] rejects.
    pub fn render_spliced(&mut self, status: StatusCode, template: Format, text: String, fragments: Vec<(usize, Arc<String>)>) -> Response {
        let mut response = self.render(status, template, Bytes::new());
        response.body = Body::spliced(text, fragments);
        response
    }

    /// `render turbo_stream:` (`text/vnd.turbo-stream.html`).
    pub fn turbo_stream(&mut self, html: impl Into<Bytes>) -> Response {
        self.render_as(StatusCode::OK, response::TURBO_STREAM_UTF8, html)
    }

    /// `render json:`
    pub fn json<T: Serialize + ?Sized>(&mut self, status: StatusCode, value: &T) -> Result<Response> {
        let body = serde_json::to_vec(value).map_err(Error::internal)?;
        Ok(self.render_as(status, response::JSON_UTF8, body))
    }

    /// `head status`: no body; a bare content type (no charset) unless the status has no content.
    pub fn head(&mut self, status: StatusCode) -> Response {
        let mut response = Response::new(status);
        if !matches!(status.as_u16(), 100..=199 | 204 | 205 | 304) {
            let format = self.rendered_format();
            response = response.content_type(format.string);
        }
        response
    }

    /// `head status, location: url`
    pub fn head_with_location(&mut self, status: StatusCode, location: &str) -> Result<Response> {
        let location = self.compute_location(location)?;
        Ok(self.head(status).header(header::LOCATION, &location))
    }

    /// `redirect_to location` (302).
    pub fn redirect_to(&mut self, location: &str) -> Result<Response> {
        self.redirect_to_with(location, Redirect::default())
    }

    /// `redirect_to location, status:, notice:, alert:, allow_other_host:`
    pub fn redirect_to_with(&mut self, location: &str, options: Redirect) -> Result<Response> {
        if let Some(notice) = options.notice {
            self.flash().set_notice(notice);
        }
        if let Some(alert) = options.alert {
            self.flash().set_alert(alert);
        }
        let location = self.compute_location(location)?;
        if location.bytes().any(|b| matches!(b, 0x00..=0x08 | 0x0A..=0x1F)) {
            return Err(Error::UnsafeRedirect(format!("The redirect URL {location} contains illegal characters")));
        }
        if !options.allow_other_host && !self.url_host_allowed(&location) {
            return Err(Error::UnsafeRedirect(format!("Unsafe redirect to {location:?}")));
        }
        Ok(Response::new(options.status.unwrap_or(StatusCode::FOUND)).content_type(response::HTML_UTF8).header(header::LOCATION, &location))
    }

    /// `redirect_back_or_to fallback`: the referer when it's on this host.
    pub fn redirect_back_or_to(&mut self, fallback: &str) -> Result<Response> {
        match self.request.referer().map(str::to_string) {
            Some(referer) if self.url_host_allowed(&referer) => self.redirect_to(&referer),
            _ => self.redirect_to(fallback),
        }
    }

    /// `_compute_redirect_to_location` for strings: absolute URLs pass through, paths get the
    /// request's protocol and host, path-relative URLs raise (`action_on_path_relative_redirect`).
    fn compute_location(&self, location: &str) -> Result<String> {
        let is_absolute = {
            let bytes = location.as_bytes();
            location.starts_with("//")
                || (bytes.first().is_some_and(u8::is_ascii_alphabetic)
                    && location.find(':').is_some_and(|i| location[..i].bytes().all(|b| b.is_ascii_alphanumeric() || b"-+.".contains(&b))))
        };
        let url = if is_absolute {
            location.to_string()
        } else {
            if !location.is_empty() && !location.starts_with('/') && !location.starts_with('?') {
                return Err(Error::UnsafeRedirect(format!("Path relative URL redirect detected: {location:?}")));
            }
            format!("{}{}{location}", self.request.protocol(), self.request.host_with_port())
        };
        Ok(url.replace(['\0', '\r', '\n'], ""))
    }

    /// `_url_host_allowed?`
    fn url_host_allowed(&self, url: &str) -> bool {
        match url_host(url) {
            Some(host) => host.eq_ignore_ascii_case(&self.request.host()),
            None => url.starts_with('/') && !url.starts_with("//"),
        }
    }

    /// An absolute URL for `path` on this request's host (what `*_url` helpers produce with
    /// `default_url_options` from `SetCurrentRequest`).
    pub fn url_for(&self, path: &str) -> String {
        format!("{}{}{path}", self.request.protocol(), self.request.host_with_port())
    }

    /// `send_file path, type:, disposition:, filename:`
    pub fn send_file(&mut self, path: impl AsRef<Path>, mut options: SendOptions) -> Result<Response> {
        let path = path.as_ref();
        let metadata = std::fs::metadata(path).ok().filter(|m| m.is_file());
        let Some(metadata) = metadata else {
            return Err(Error::internal(anyhow::anyhow!("Cannot read file {}", path.display())));
        };
        if options.filename.is_none() {
            options.filename = path.file_name().map(|n| n.to_string_lossy().into_owned());
        }
        Ok(response::send(&options, SendBody::File(path.to_path_buf(), metadata.len())))
    }

    /// `send_data data, type:, disposition:, filename:`
    pub fn send_data(&mut self, data: impl Into<Bytes>, options: SendOptions) -> Response {
        response::send(&options, SendBody::Bytes(data.into()))
    }

    // --- Conditional GET -----------------------------------------------------------------------

    /// `fresh_when`: sets ETag/Last-Modified and returns `Some(304)` when the request is fresh.
    pub fn fresh_when(&mut self, freshness: Freshness) -> Option<Response> {
        self.cache_control.no_store = false;
        let etagged = freshness.strong_etag.is_some() || freshness.etag.is_some() || freshness.template.is_some();
        if etagged {
            let (validator, weak) = match &freshness.strong_etag {
                Some(strong) => (Some(strong.clone()), false),
                None => (freshness.etag.clone(), true),
            };
            let etag = self.combine_etags(validator, &freshness);
            let value = if weak { format!("W/\"{etag}\"") } else { format!("\"{etag}\"") };
            self.set_header(header::ETAG, &value);
        }
        if let Some(last_modified) = freshness.last_modified {
            self.set_header(header::LAST_MODIFIED, &clock::httpdate(last_modified));
        }
        if freshness.public {
            self.cache_control.public = true;
        }
        if self.is_fresh() { Some(self.head(StatusCode::NOT_MODIFIED)) } else { None }
    }

    /// `stale?`: the inverse of freshness, after setting the validators.
    pub fn stale(&mut self, freshness: Freshness) -> bool {
        self.fresh_when(freshness).is_none()
    }

    /// `[validator, *etaggers]` digested like `ActiveSupport::Digest.hexdigest(expand_cache_key(...))`.
    /// The etaggers are turbo-rails' frame etagger, `ETagWithTemplateDigest` and `ETagWithFlash`.
    fn combine_etags(&mut self, validator: Option<String>, freshness: &Freshness) -> String {
        let mut parts: Vec<String> = validator.into_iter().collect();
        if self.is_turbo_frame_request() {
            parts.push("frame".into());
        }
        if let Some(template) = &freshness.template {
            parts.push(template.clone());
        }
        let flash = self.flash();
        if !flash.is_empty() {
            let flashes: Vec<String> = flash.keys().map(|k| format!("{k}={:?}", flash.get(k))).collect();
            parts.push(flashes.join("&"));
        }
        hex::encode(&Sha256::digest(parts.join("/"))[..16])
    }

    fn is_fresh(&self) -> bool {
        let header = |name| self.headers.get(name).and_then(|v: &HeaderValue| v.to_str().ok());
        is_fresh(&self.request, header(header::ETAG), header(header::LAST_MODIFIED))
    }

    /// `expires_in seconds, public:, stale_while_revalidate:, ...`
    pub fn expires_in(&mut self, seconds: u64, options: ExpiresIn) {
        let cc = &mut self.cache_control;
        cc.no_store = false;
        cc.max_age = Some(seconds);
        cc.public = options.public;
        cc.must_revalidate = options.must_revalidate;
        cc.stale_while_revalidate = options.stale_while_revalidate;
        cc.stale_if_error = options.stale_if_error;
        cc.immutable = options.immutable;
        let now = clock::httpdate(self.now());
        if !self.headers.contains_key(header::DATE) {
            self.set_header(header::DATE, &now);
        }
    }

    /// `expires_now`
    pub fn expires_now(&mut self) {
        self.cache_control = CacheControl { no_cache: true, ..CacheControl::default() };
    }

    /// `no_store`
    pub fn no_store(&mut self) {
        self.cache_control = CacheControl { no_store: true, ..CacheControl::default() };
    }

    /// Set a response header ahead of the response (`response.headers[...] = ...`), as Puma would
    /// write the value (see `Response::header`).
    pub fn set_header(&mut self, name: impl TryInto<HeaderName>, value: &str) {
        let name = name.try_into().unwrap_or_else(|_| panic!("invalid header name"));
        crate::response::set_header_value(&mut self.headers, name, value);
    }

    /// A controller that includes `ActionController::Live` (`ActiveStorage::Streaming` does):
    /// its `make_response!` builds the response with `Live::Response.new`, which skips
    /// `ActionDispatch::Response.create` and so `config.action_dispatch.default_headers`.
    pub fn use_live_response(&mut self) {
        self.live = true;
    }

    fn default_headers(&self) -> &[(HeaderName, HeaderValue)] {
        if self.live { &[] } else { &self.kit.config().default_headers }
    }

    // --- Finishing -----------------------------------------------------------------------------

    /// Turn the action's result into the response Rails would send: halts and errors resolved,
    /// flash and session committed into cookies, cache headers, ETag and 304, HEAD bodies dropped.
    pub(crate) fn finish(mut self, result: Result<Response>) -> Response {
        let mut response = match result {
            Ok(response) => response,
            Err(Error::Halt(response)) => *response,
            Err(error) => return self.error_response(error),
        };

        for (name, value) in self.headers.iter() {
            if !response.headers.contains_key(name) {
                response.headers.insert(name.clone(), value.clone());
            }
        }

        if let Err(error) = self.verify_same_origin_request(&response) {
            return self.error_response(error);
        }
        if let Err(error) = self.commit(&mut response) {
            return self.error_response(error);
        }

        self.apply_cache_headers(&mut response);
        for (name, value) in self.default_headers() {
            if !response.headers.contains_key(name) {
                response.headers.insert(name.clone(), value.clone());
            }
        }
        if !response.headers.contains_key(header::CONTENT_TYPE) && !matches!(response.status.as_u16(), 100..=199 | 204 | 205 | 304) {
            response.headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(response::HTML_UTF8));
        }
        rack_etag(&mut response, !self.live);
        self.conditional_get(&mut response);
        response
    }

    /// `verify_same_origin_request`: a GET that renders JavaScript for a non-XHR request is a
    /// cross-origin `<script>` embed.
    fn verify_same_origin_request(&self, response: &Response) -> Result<()> {
        let javascript = response
            .get_header(header::CONTENT_TYPE)
            .is_some_and(|ct| ct.starts_with("text/javascript") || ct.starts_with("application/javascript"));
        if self.marked_for_same_origin_verification && javascript && !self.request.is_xhr() {
            return Err(Error::InvalidCrossOriginRequest);
        }
        Ok(())
    }

    /// `commit_flash`, `commit_session`, then the cookie jar's `write`.
    fn commit(&mut self, response: &mut Response) -> Result<()> {
        if let Some(flash) = self.flash.take() {
            let has_flash_key = self.session().contains_key("flash");
            if !flash.is_empty() || has_flash_key {
                match flash.to_session_value() {
                    Some(value) => self.session().insert("flash", value),
                    None => {
                        self.session().insert("flash", serde_json::Value::Null);
                    }
                }
            }
        }
        if self.session.is_loaded() && self.session.contains_key("flash") && self.session.get("flash").is_none() {
            self.session.remove("flash");
        }
        let now = self.now();
        self.session.commit(&mut self.cookies, now)?;

        for cookie in self.cookies.set_cookie_headers(self.request.is_ssl(), &self.request.host()) {
            response.headers.append(header::SET_COOKIE, HeaderValue::from_str(&cookie).map_err(Error::internal)?);
        }
        Ok(())
    }

    /// `handle_conditional_get!` and `merge_and_normalize_cache_control!`.
    fn apply_cache_headers(&self, response: &mut Response) {
        if response.headers.contains_key(header::CACHE_CONTROL) {
            return;
        }
        let mut cache_control = self.cache_control.clone();
        if cache_control.is_empty() && (response.headers.contains_key(header::ETAG) || response.headers.contains_key(header::LAST_MODIFIED))
        {
            cache_control = CacheControl { max_age: Some(0), must_revalidate: true, ..CacheControl::default() };
        }
        if let Some(value) = cache_control.to_header() {
            response.headers.insert(header::CACHE_CONTROL, HeaderValue::from_str(&value).unwrap());
        }
    }

    /// `Rack::ConditionalGet`
    fn conditional_get(&self, response: &mut Response) {
        if !matches!(self.request.method, Method::GET | Method::HEAD) || response.status != StatusCode::OK {
            return;
        }
        if is_fresh(&self.request, response.get_header(header::ETAG), response.get_header(header::LAST_MODIFIED)) {
            response.status = StatusCode::NOT_MODIFIED;
            response.headers.remove(header::CONTENT_TYPE);
            response.headers.remove(header::CONTENT_LENGTH);
            response.body = Body::Empty;
        }
    }

    /// What `ShowExceptions` + `PublicExceptions` render for an error raised in the action. The
    /// log line has the error's sources too (`{:#}`: "outer: cause: ...").
    fn error_response(&mut self, error: Error) -> Response {
        // Parameter parsing errors may include the original input, including a setup bearer or
        // administrator password. These routes retain their status and path without that detail.
        let detail = if crate::private_setup_path(self.request.path()) { error.status().to_string() } else { format!("{error:#}") };
        if error.status().is_server_error() {
            tracing::error!(error = %detail, path = self.request.path(), "request failed");
        } else {
            tracing::info!(error = %detail, path = self.request.path(), "request rejected");
        }
        let formats = self.formats().unwrap_or_default();
        crate::exceptions::render(self.kit.error_pages(), error.status(), formats.first().copied(), self.request.is_head())
    }
}

/// `request.fresh?(response)` with `strict_freshness` (the 8.0 default), which `fresh_when` and
/// `Rack::ConditionalGet` both go by here: an `If-None-Match` list naming the ETag (or `*`), or
/// else an `If-Modified-Since` no earlier than `Last-Modified`. (Rack's own check wants the whole
/// `If-None-Match` to equal the ETag.)
fn is_fresh(request: &Request, etag: Option<&str>, last_modified: Option<&str>) -> bool {
    if let Some(if_none_match) = request.header("if-none-match") {
        let Some(etag) = etag else { return false };
        if_none_match.split(',').map(str::trim).any(|v| v == etag || v == "*")
    } else if let Some(since) = request.header("if-modified-since").and_then(clock::parse_httpdate) {
        last_modified.and_then(clock::parse_httpdate).is_some_and(|last_modified| since >= last_modified)
    } else {
        false
    }
}

/// `Rack::ETag` (installed as `Rack::ETag, "no-cache"`): weak SHA-256 ETags for 200/201 bodies
/// without validators, and a default `Cache-Control`. A Live response's body is a
/// `Live::Buffer`, which doesn't respond to `to_ary`, so it's never digested: whatever such a
/// controller renders goes out with `no-cache` and no ETag.
fn rack_etag(response: &mut Response, digestible: bool) {
    let mut digested = false;
    let skip = !digestible || response.headers.contains_key(header::ETAG) || response.headers.contains_key(header::LAST_MODIFIED);
    if matches!(response.status.as_u16(), 200 | 201)
        && !skip
        && let Some(hex) = body_etag(response)
    {
        response.headers.insert(header::ETAG, HeaderValue::from_str(&format!("W/\"{}\"", &hex[..32])).unwrap());
        digested = true;
    }
    if !response.headers.contains_key(header::CACHE_CONTROL) {
        let value = if digested { "max-age=0, private, must-revalidate" } else { "no-cache" };
        response.headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
    }
}

/// The hex of `Rack::ETag`'s digest of a non-empty body. A page of cached fragments hashes its
/// parts' digests rather than the whole body; another body keeps its SHA-256 for the gzip cache.
fn body_etag(response: &mut Response) -> Option<String> {
    match &response.body {
        Body::Parts(parts) => Some(parts.etag()),
        Body::Bytes(bytes) if !bytes.is_empty() => {
            let digest = BodyDigest(Sha256::digest(bytes).into());
            response.body_digest = Some(digest);
            Some(hex::encode(digest.0))
        }
        _ => None,
    }
}

/// The host of an absolute or protocol-relative URL, `None` for paths.
fn url_host(url: &str) -> Option<String> {
    let rest = if let Some(rest) = url.strip_prefix("//") {
        rest
    } else {
        let (scheme, rest) = url.split_once("://")?;
        if scheme.is_empty() || !scheme.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b)) {
            return None;
        }
        rest
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        format!("[{}]", v6.split(']').next().unwrap_or(""))
    } else {
        host_port.split(':').next().unwrap_or("").to_string()
    };
    Some(host)
}
