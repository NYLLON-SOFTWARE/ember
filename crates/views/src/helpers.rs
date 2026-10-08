//! Ports of `reference/app/helpers` and the Rails built-in helpers Matchbox's views use.
//!
//! Helpers return [`Html`] (askama's `Safe<String>`, the SafeBuffer equivalent), which templates
//! print unescaped; everything else is escaped by [`ErbEscaper`] exactly like ERB. Block helpers
//! (`link_to ... do`) live in [`filters`] and are used as askama filter blocks.
//!
//! Templates reach these as `h::name(...)` after `use crate::helpers as h;` in the view module.

pub mod application;
pub mod assets;
pub mod filters;
pub mod forms;
pub mod html;
pub mod icons;
pub mod links;
pub mod rooms;
pub mod tag;
pub mod translations;
#[rustfmt::skip]
mod translations_table;
pub mod turbo;
pub mod ui;
pub mod url;
pub mod users;

pub use application::*;
pub use assets::*;
pub use forms::*;
pub use html::*;
pub use icons::*;
pub use links::*;
pub use rooms::*;
pub use tag::*;
pub use translations::*;
pub use turbo::*;
pub use url::*;
pub use users::*;

/// Path helpers, re-exported so templates can write `h::routes::user_profile()`.
pub use matchbox_routes as routes;
