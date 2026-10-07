//! `UnfurlLinksController` (reference/app/controllers/unfurl_links_controller.rb): the composer
//! asks for a pasted URL's OpenGraph metadata.

use matchbox_kit::{Ctx, Error, Result, StatusCode};

use crate::concerns::{Before, before_actions};
use crate::integrations::net::Network;
use crate::integrations::opengraph::{self, Unfurl};

pub async fn create(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    // A hash or array passes `require`, but `URI.parse` can't take it (`InvalidURIError`,
    // rescued), so the metadata has no title and isn't valid.
    let Some(url) = url_param(c)? else { return Ok(c.head(StatusCode::NO_CONTENT)) };
    match opengraph_json(c, &url).await? {
        // `render json: opengraph`
        Some(json) => Ok(c.render_as(StatusCode::OK, matchbox_kit::response::JSON_UTF8, json)),
        None => Ok(c.head(StatusCode::NO_CONTENT)),
    }
}

/// `params.require(:url)`
fn url_param(c: &Ctx) -> Result<Option<String>> {
    Ok(c.params.require("url")?.as_str().map(str::to_string))
}

/// `Opengraph::Metadata.from_url(url)`, as JSON when `valid?` (`crate::integrations::opengraph`).
/// The network is the system's unless the request carries another one (tests).
async fn opengraph_json(c: &Ctx, url: &str) -> Result<Option<String>> {
    let net = c.current::<Network>().cloned().unwrap_or_else(Network::system);
    match opengraph::unfurl(&net, url).await.map_err(Error::internal)? {
        Unfurl::Json(json) => Ok(Some(json)),
        Unfurl::NoContent => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{Method, StatusCode};

    use crate::controllers::presenters::test_support::*;

    #[tokio::test]
    async fn unfurls_nothing_from_private_addresses_and_needs_a_url() {
        let Some(app) = TestApp::boot().await else { return };
        let mut david = app.david();
        let private = david.write(Req::new(Method::POST, "/unfurl_link").form(&[("url", "http://127.0.0.1/secret")])).await;
        assert_eq!(private.status, StatusCode::NO_CONTENT);
        let missing = david.write(Req::new(Method::POST, "/unfurl_link").form(&[("url", "")])).await;
        assert_eq!(missing.status, StatusCode::BAD_REQUEST);
        let hash = david.write(Req::new(Method::POST, "/unfurl_link").form(&[("url[a]", "http://example.com")])).await;
        assert_eq!(hash.status, StatusCode::NO_CONTENT);
        let array = david.write(Req::new(Method::POST, "/unfurl_link").form(&[("url[]", "http://example.com")])).await;
        assert_eq!(array.status, StatusCode::NO_CONTENT);
    }
}
