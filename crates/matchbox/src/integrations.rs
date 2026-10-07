//! Matchbox's outbound HTTP, and its search query preprocessing, each matching the Rails app:
//!
//! - [`web_push`]: `WebPush::Pool`/`WebPush::Notification` (reference/lib/web_push,
//!   reference/config/initializers/web_push.rb) and `Room::MessagePusher`'s delivery: permitted
//!   push services only, the endpoint resolved through the private network guard and pinned.
//! - [`opengraph`]: `UnfurlLinksController#create` over `Opengraph::*`: every address guarded
//!   and pinned, every redirect re-checked, 10 responses and 5MB at most.
//! - [`webhook`]: `Webhook#deliver` for bots: intentionally unguarded, 7-second timeouts.
//! - [`search`]: the query sanitizing in `SearchesController#query`.
//! - [`register_jobs`]: `Room::PushMessageJob` and `Bot::WebhookJob` for the job runner.
//!
//! The three HTTP clients share only plumbing ([`net`]); each keeps its own policy (see
//! plans/rust-conversion.md, "HTTP clients: three distinct policies"). Oracles for the tests
//! (Ruby scripts run in the reference) live in testdata/oracle.

mod jobs;
pub mod net;
pub mod opengraph;
pub mod search;
pub mod web_push;
pub mod webhook;

#[cfg(test)]
mod test_support;

pub use jobs::{register_jobs, web_push_pool};
