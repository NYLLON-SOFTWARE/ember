//! Link unfurling: `UnfurlLinksController#create` (reference/app/controllers/unfurl_links_controller.rb)
//! over `Opengraph::Metadata`, `Location`, `Fetch` and `Document` (reference/app/models/opengraph).
//!
//! Every address is resolved through the private network guard and pinned, every redirect is
//! re-checked, and documents are capped at 5MB and 10 responses.
//!
//! Unlike Rails, which gives each connect and read 60 seconds, an unfurl has 10 seconds in all
//! and each connect or read 5, at most 16 run at once, and parsing runs off the async workers:
//! the endpoint is open to any signed-in user and fetches pages they choose.

mod document;
mod entities;
mod fetch;
mod html;
mod location;
mod metadata;

use std::sync::{Arc, LazyLock};
use std::time::Duration;

pub use metadata::Metadata;
use tokio::sync::Semaphore;

use crate::integrations::net::Network;

/// The most one unfurl may take, redirects and the image check included.
pub const UNFURL_DEADLINE: Duration = Duration::from_secs(10);

/// Unfurls in flight at once; more wait their turn, within their deadline.
const MAX_CONCURRENT_UNFURLS: usize = 16;

/// What `UnfurlLinksController#create` responds with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unfurl {
    /// `render json: opengraph` (200, `application/json`): the body.
    Json(String),
    /// `head :no_content`
    NoContent,
}

/// Where the Rails action raises (a 500).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UnfurlError {
    #[error("raised {0}")]
    Raised(&'static str),
}

/// The action after `params.require(:url)` (a missing or blank `url` is the controller's 400).
/// One that runs out of time unfurls nothing.
pub async fn unfurl(net: &Network, url: &str) -> Result<Unfurl, UnfurlError> {
    unfurl_within(net, url, UNFURL_DEADLINE).await
}

async fn unfurl_within(net: &Network, url: &str, deadline: Duration) -> Result<Unfurl, UnfurlError> {
    static SLOTS: Semaphore = Semaphore::const_new(MAX_CONCURRENT_UNFURLS);
    let unfurling = async {
        let _slot = SLOTS.acquire().await.expect("never closed");
        let mut opengraph = Metadata::from_url(net, url).await?;
        Ok(if opengraph.validate(net).await? { Unfurl::Json(opengraph.to_json()) } else { Unfurl::NoContent })
    };
    tokio::time::timeout(deadline, unfurling).await.unwrap_or_else(|_| {
        tracing::warn!("Gave up unfurling {url} after {deadline:?}");
        Ok(Unfurl::NoContent)
    })
}

/// Pages parsed at once. The blocking work outlives an unfurl that gives up at its deadline, so
/// it's bounded by permits the work itself holds, not by the unfurl's slot.
const MAX_CONCURRENT_PARSES: usize = 4;

/// Runs CPU-bound work (parsing and sanitizing pages of up to 5MB) on the blocking pool.
async fn off_the_runtime<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    static PARSES: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(MAX_CONCURRENT_PARSES)));
    let permit = PARSES.clone().acquire_owned().await.expect("never closed");
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .unwrap_or_else(|error| std::panic::resume_unwind(error.into_panic()))
}

#[cfg(test)]
mod tests;
