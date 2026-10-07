//! `Opengraph::Metadata` and its `Fetching` concern (reference/app/models/opengraph/metadata.rb,
//! reference/app/models/opengraph/metadata/fetching.rb).

use matchbox_richtext::dom::Dom;
use matchbox_richtext::sanitizer::{self, SafeList};
use matchbox_richtext::uri::{self, UriError};
use rails_compat::json;

use super::document::{self, is_blank};
use super::location::Location;
use super::{UnfurlError, off_the_runtime};
use crate::integrations::net::Network;

const TWITTER_HOSTS: [&str; 4] = ["twitter.com", "www.twitter.com", "x.com", "www.x.com"];
const FX_TWITTER_HOST: &str = "fxtwitter.com";
const ALLOWED_IMAGE_CONTENT_TYPES: [&str; 4] = ["image/jpeg", "image/png", "image/gif", "image/webp"];

/// The model's attributes as instance variables, in the order they were first assigned (which
/// is the order `render json:` emits them).
#[derive(Debug, Clone, PartialEq)]
pub struct Metadata {
    attributes: Vec<(&'static str, Option<String>)>,
}

impl Metadata {
    /// `Metadata.from_url(url)`
    pub async fn from_url(net: &Network, url: &str) -> Result<Self, UnfurlError> {
        let body = fetch_document(net, url).await?;
        let found = off_the_runtime(move || document::opengraph_attributes(body.as_deref())).await;
        let og = |key: &str| found.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone());

        let canonical_url = valid_canonical_url(net, og("url"), url).await;
        let image = valid_image_content_type(net, og("image")).await;

        let mut attributes: Vec<(&'static str, Option<String>)> = found.iter().map(|(k, v)| (*k, Some(v.clone()))).collect();
        assign(&mut attributes, "url", Some(canonical_url));
        assign(&mut attributes, "image", image);
        Ok(Self { attributes })
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.attributes.iter().find(|(k, _)| *k == key).and_then(|(_, v)| v.as_deref())
    }

    /// `valid?`: sanitizes the title and description first (`before_validation`), then checks
    /// presence and, when there's an image, that it's a valid location.
    pub async fn validate(&mut self, net: &Network) -> Result<bool, UnfurlError> {
        const SANITIZED: [&str; 2] = ["title", "description"];
        let values = SANITIZED.map(|key| self.get(key).map(str::to_string));
        let sanitized = off_the_runtime(move || values.map(|value| value.map(|v| sanitize(&strip_tags(&v)?)).transpose())).await;
        for (key, value) in SANITIZED.into_iter().zip(sanitized) {
            assign(&mut self.attributes, key, value?);
        }
        let present = |key: &str| self.get(key).is_some_and(|v| !is_blank(v));
        let mut valid = present("title") && present("url") && present("description");
        if let Some(image) = self.get("image").filter(|i| !is_blank(i)).map(str::to_string) {
            valid &= Location::new(net, Some(&image)).is_valid().await;
        }
        Ok(valid)
    }

    /// `render json: opengraph` after `valid?`: `instance_values`, which by then include the
    /// validation context and the (empty) errors.
    pub fn to_json(&self) -> String {
        let mut json = String::from("{");
        for (key, value) in &self.attributes {
            json.push_str(&format!("{}:{},", json::encode(key), json::encode(value)));
        }
        json.push_str(r#""context_for_validation":{"context":null},"errors":{}}"#);
        json
    }
}

fn assign(attributes: &mut Vec<(&'static str, Option<String>)>, key: &'static str, value: Option<String>) {
    match attributes.iter_mut().find(|(k, _)| *k == key) {
        Some(existing) => existing.1 = value,
        None => attributes.push((key, value)),
    }
}

/// `fetch_document(untrusted_url)`: tweets are read through fxtwitter.com. A tweet whose
/// fxtwitter page can't be read raises (`nil.force_encoding`), as does a URL that `URI.parse`
/// rejects with `URI::InvalidComponentError`.
async fn fetch_document(net: &Network, url: &str) -> Result<Option<Vec<u8>>, UnfurlError> {
    if tweet_url(url)? {
        let fxtwitter_url = replace_twitter_domain(url);
        let html = Location::new(net, fxtwitter_url.as_deref()).read_html().await;
        return html.map(Some).ok_or(UnfurlError::Raised("NoMethodError"));
    }
    Ok(Location::new(net, Some(url)).read_html().await)
}

/// `tweet_url?`
fn tweet_url(url: &str) -> Result<bool, UnfurlError> {
    match uri::parse(url) {
        Ok(uri) => Ok(uri.host.as_deref().is_some_and(|h| TWITTER_HOSTS.contains(&h))
            && uri.path.as_deref().is_some_and(|p| !is_blank(p) && p != "/")),
        Err(UriError::InvalidUri) => Ok(false),
        Err(UriError::InvalidComponent) => Err(UnfurlError::Raised("URI::InvalidComponentError")),
    }
}

/// `replace_twitter_domain_for_opengraph_support`
fn replace_twitter_domain(url: &str) -> Option<String> {
    let mut uri = uri::parse(url).ok()?;
    if uri.host.as_deref().is_some_and(|h| TWITTER_HOSTS.contains(&h)) {
        uri.host = Some(FX_TWITTER_HOST.into());
    }
    Some(uri.to_s())
}

/// `valid_canonical_url(url, fallback)`
async fn valid_canonical_url(net: &Network, url: Option<String>, fallback: &str) -> String {
    match url {
        Some(url) if Location::new(net, Some(&url)).is_valid().await => url,
        _ => fallback.to_string(),
    }
}

/// `valid_image_content_type(image)`: kept only when a HEAD says it's a JPEG, PNG, GIF or WebP.
async fn valid_image_content_type(net: &Network, image: Option<String>) -> Option<String> {
    let image = image.filter(|i| !is_blank(i))?;
    if uri::parse(&image).is_err() {
        tracing::warn!("Failed to fetch image content tpye: {image} (bad URI(is not URI?): {image:?})");
        return None;
    }
    let content_type = Location::new(net, Some(&image)).fetch_content_type().await?.to_lowercase();
    ALLOWED_IMAGE_CONTENT_TYPES.contains(&content_type.as_str()).then_some(image)
}

/// `strip_tags` (Rails::HTML5::FullSanitizer): the text of the HTML5 fragment, serialized.
fn strip_tags(html: &str) -> Result<String, UnfurlError> {
    if html.is_empty() {
        return Ok(String::new());
    }
    let mut dom = Dom::new();
    let fragment = dom.parse_fragment(html).map_err(|_| UnfurlError::Raised("ArgumentError"))?;
    let text: String = dom.descendants(fragment).into_iter().filter_map(|node| dom.text(node).map(str::to_string)).collect();
    let out = dom.new_fragment();
    if !text.is_empty() {
        let node = dom.create_text(&text);
        dom.append(out, node);
    }
    Ok(dom.to_html(out))
}

/// `sanitize` (Rails::HTML5::SafeListSanitizer with its default allowlist).
fn sanitize(html: &str) -> Result<String, UnfurlError> {
    sanitizer::sanitize(html, SafeList::defaults()).map_err(|_| UnfurlError::Raised("ArgumentError"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Probed against the reference (`strip_tags` then `sanitize`).
    #[test]
    fn strips_tags_like_rails() {
        for (input, expected) in [
            ("Tom & Jerry", "Tom &amp; Jerry"),
            ("a < b", "a &lt; b"),
            ("x&nbsp;y", "x&nbsp;y"),
            ("\u{a0}nb", "&nbsp;nb"),
            ("Hey!<script>alert('hi')</script>", "Hey!alert('hi')"),
            ("<!-- c -->t", "t"),
            ("a &lt;b&gt; c", "a &lt;b&gt; c"),
            ("<p>one</p><p>two</p>", "onetwo"),
            ("\"q\" 'a'", "\"q\" 'a'"),
            ("<style>x</style>y", "xy"),
            ("&amp;amp;", "&amp;amp;"),
            ("<b>bold</b>", "bold"),
            ("</script><img src=a onerror=prompt(1)>", ""),
            (" sp  ", " sp  "),
            ("<textarea>t<b>x</b></textarea>", "t&lt;b&gt;x&lt;/b&gt;"),
        ] {
            assert_eq!(sanitize(&strip_tags(input).unwrap()).unwrap(), expected, "{input}");
        }
    }
}
