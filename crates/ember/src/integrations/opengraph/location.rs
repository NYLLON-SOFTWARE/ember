//! `Opengraph::Location` (reference/app/models/opengraph/location.rb): a URL that is valid when
//! it parses as http(s) and its host resolves to a public address, which is memoized and pinned
//! for the fetch.

use std::net::IpAddr;
use std::sync::LazyLock;

use ember_richtext::uri::{self, Uri};
use regex::Regex;

use super::fetch;
use crate::integrations::net::{Network, guard};

/// `FILES_AND_MEDIA_URL_REGEX`
static FILES_AND_MEDIA_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bhttps?://\S+\.(?:zip|tar|tar\.gz|tar\.bz2|tar\.xz|gz|bz2|rar|7z|dmg|exe|msi|pkg|deb|iso|jpg|jpeg|png|gif|bmp|mp4|mov|avi|mkv|wmv|flv|heic|heif|mp3|wav|ogg|aac|wma|webm|ogv|mpg|mpeg)\b").unwrap()
});

pub struct Location<'n> {
    net: &'n Network,
    url: Option<String>,
    parsed_url: Option<Uri>,
    resolved_ip: Option<Option<IpAddr>>,
}

impl<'n> Location<'n> {
    /// `Location.new(url)`: `parsed_url` is `URI.parse(url) rescue nil`.
    pub fn new(net: &'n Network, url: Option<&str>) -> Self {
        let parsed_url = url.and_then(|url| uri::parse(url).ok());
        Self { net, url: url.map(str::to_string), parsed_url, resolved_ip: None }
    }

    /// `valid?`: both validations run, so the host is resolved even for a non-http URL.
    pub async fn is_valid(&mut self) -> bool {
        let http = self.parsed_url.as_ref().is_some_and(Uri::is_http);
        let public = self.resolved_ip().await.is_some();
        http && public
    }

    /// `resolved_ip`: `PrivateNetworkGuard.resolve(parsed_url.host) rescue nil`, memoized.
    pub async fn resolved_ip(&mut self) -> Option<IpAddr> {
        if let Some(ip) = self.resolved_ip {
            return ip;
        }
        let ip = match self.parsed_url.as_ref() {
            Some(url) => match url.host.as_deref() {
                Some(host) => guard::resolve(self.net.resolver.as_ref(), host).await.ok(),
                None => None,
            },
            None => None,
        };
        self.resolved_ip = Some(ip);
        ip
    }

    /// `read_html`: nothing for invalid URLs or ones that look like files and media.
    pub async fn read_html(&mut self) -> Option<Vec<u8>> {
        if !self.is_valid().await || FILES_AND_MEDIA_URL.is_match(self.url.as_deref().unwrap_or("")) {
            return None;
        }
        let url = self.parsed_url.clone()?;
        let ip = self.resolved_ip().await?;
        match fetch::fetch_document(self.net, &url, ip).await {
            Ok(html) => html,
            Err(error) => {
                tracing::warn!("Failed to fetch {} at {ip} ({error})", url.to_s());
                None
            }
        }
    }

    /// `fetch_content_type`
    pub async fn fetch_content_type(&mut self) -> Option<String> {
        if !self.is_valid().await {
            return None;
        }
        let url = self.parsed_url.clone()?;
        let ip = self.resolved_ip().await?;
        match fetch::fetch_content_type(self.net, &url, ip).await {
            Ok(content_type) => content_type,
            Err(error) => {
                tracing::warn!("Failed to fetch {} at {ip} ({error})", url.to_s());
                None
            }
        }
    }
}
