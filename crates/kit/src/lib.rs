//! The Rails-compatibility HTTP layer.
//!
//! Axum does dispatch and WebSocket upgrades; everything a Rails controller would touch goes
//! through [`Ctx`]. Actions are plain `async fn action(c: &mut Ctx) -> Result<Response>`, and
//! before-actions are ordered calls at the top of them that return early with [`halt`]:
//!
//! ```ignore
//! async fn update(c: &mut Ctx) -> Result {
//!     require_authentication(c).await?;
//!     c.verify_authenticity_token()?;
//!     ensure_can_administer(c)?;             // halt(c.head(StatusCode::FORBIDDEN))
//!     let room = c.params.require("room")?.permit(&permit_keys(&["name"]));
//!     // ...
//!     c.redirect_to("/rooms/1")
//! }
//!
//! let router = Router::new().route("/rooms/{id}", kit::get(show).patch(action(update)));
//! let app = kit::app(router, kit);
//! ```

pub mod adapter;
pub mod app;
pub mod body;
pub mod clock;
pub mod cookies;
pub mod ctx;
pub mod deflater;
pub mod error;
pub mod exceptions;
pub mod format;
pub mod front;
pub mod params;
pub mod request;
pub mod response;
pub mod server;
pub mod session;

pub use adapter::{ActionFn, OriginalMethod, RequestId, action, app, delete, get, patch, post, put};
pub use app::{Kit, KitConfig};
pub use clock::{Clock, SharedClock, SystemClock, TestClock};
pub use cookies::{Cookie, CookieJar, SameSite};
pub use ctx::{Ctx, Freshness, Redirect};
pub use error::{Error, Result, halt};
pub use format::{Format, Mime};
pub use params::{Param, ParamMap, Permit, UploadedFile, parse_nested, permit_keys};
pub use request::Request;
pub use response::{Body, CacheControl, ExpiresIn, Response, SendOptions};
pub use session::{Flash, Session};

pub use axum::http::{self, HeaderMap, Method, StatusCode};

/// First-run credentials must stay private even when a malformed request fails before routing.
pub(crate) fn private_setup_path(path: &str) -> bool {
    path.trim_start_matches('/').strip_prefix("first_run").is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(['/', '.']))
}
