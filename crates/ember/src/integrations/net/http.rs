//! One HTTP/1.1 request over a fresh connection, the way `Net::HTTP` makes it: the connection
//! goes to a pinned address when the caller has one (`http.ipaddr = ip`), TLS verifies the
//! peer against the host name, `open_timeout` covers the connect and TLS handshake,
//! `read_timeout` covers each read, and a body the client asked to be compressed
//! (`Accept-Encoding: gzip;q=1.0,deflate;q=0.6,identity;q=0.3`) is inflated as it's read.

use std::io::{self, Write as _};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;

use super::Network;

/// `Net::HTTP`'s default open and read timeouts.
pub const NET_HTTP_DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// The `Accept-Encoding` `Net::HTTP` adds to requests whose responses have bodies, and then
/// decodes itself.
pub const NET_HTTP_ACCEPT_ENCODING: &str = "gzip;q=1.0,deflate;q=0.6,identity;q=0.3";

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// `Net::OpenTimeout`
    #[error("execution expired")]
    OpenTimeout,
    /// `Net::ReadTimeout`
    #[error("Net::ReadTimeout")]
    ReadTimeout,
    /// The host has no addresses (`SocketError`).
    #[error("getaddrinfo: {0}")]
    Unresolvable(String),
    /// `OpenSSL::SSL::SSLError`: the TLS handshake or a TLS read failed.
    #[error("SSL error: {0}")]
    Tls(String),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Http(String),
    /// `Zlib::Error` while inflating a compressed body.
    #[error("{0}")]
    Inflate(String),
}

impl HttpError {
    fn from_hyper(error: hyper::Error) -> Self {
        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
        while let Some(e) = source {
            if let Some(tls) = e.downcast_ref::<rustls::Error>() {
                return HttpError::Tls(tls.to_string());
            }
            source = e.source();
        }
        HttpError::Http(error.to_string())
    }

    fn from_io(error: io::Error) -> Self {
        match error.get_ref().and_then(|e| e.downcast_ref::<rustls::Error>()) {
            Some(tls) => HttpError::Tls(tls.to_string()),
            None => HttpError::Io(error),
        }
    }
}

/// Where the request goes: `Net::HTTP.new(host, port)` plus `use_ssl`.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub https: bool,
    /// The host as `URI#host` gives it (IPv6 literals in brackets): the TLS server name.
    pub host: String,
    pub port: u16,
    /// `http.ipaddr`: connect here instead of resolving `host`.
    pub pinned_ip: Option<IpAddr>,
}

impl Endpoint {
    /// `Net::HTTP#addr_port`: the `Host` header when the request doesn't carry one.
    pub fn host_header(&self) -> String {
        let host = if self.host.contains(':') && !self.host.starts_with('[') { format!("[{}]", self.host) } else { self.host.clone() };
        let default_port = if self.https { 443 } else { 80 };
        if self.port == default_port { host } else { format!("{host}:{}", self.port) }
    }

    fn bare_host(&self) -> &str {
        self.host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(&self.host)
    }
}

pub struct Timeouts {
    pub open: Duration,
    pub read: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self { open: NET_HTTP_DEFAULT_TIMEOUT, read: NET_HTTP_DEFAULT_TIMEOUT }
    }
}

pub struct Request {
    pub method: hyper::Method,
    /// `request_uri`: path and query.
    pub target: String,
    /// In the order `Net::HTTP` writes them, `Host` included.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// We sent `NET_HTTP_ACCEPT_ENCODING`, so a gzip or deflate body is inflated on read.
    pub decode_content: bool,
}

impl Request {
    /// `Net::HTTP::Get.new(uri)` / `Head.new(uri)` / `Post.new(uri_or_path, headers)`: the
    /// caller's headers, then `Accept-Encoding` (decoded when the response has a body),
    /// `Accept`, `User-Agent`, and `Host` when built from a URI (`uri_host`).
    pub fn net_http(method: hyper::Method, target: String, uri_host: Option<String>, headers: Vec<(String, String)>) -> Self {
        let response_has_body = method != hyper::Method::HEAD;
        let mut request = Self { method, target, headers, body: Vec::new(), decode_content: false };
        if !request.has("accept-encoding") && !request.has("range") {
            request.decode_content = response_has_body;
            request.headers.push(("Accept-Encoding".into(), NET_HTTP_ACCEPT_ENCODING.into()));
        }
        request.default("Accept", "*/*");
        request.default("User-Agent", "Ruby");
        if let Some(host) = uri_host {
            request.default("Host", &host);
        }
        request
    }

    /// `Net::HTTP#request` on a connection that wasn't started (`Connection: close`), then
    /// `begin_transport` (`Host` from `addr_port` unless the request has one).
    pub fn transport(mut self, unstarted: bool, endpoint: &Endpoint) -> Self {
        if unstarted {
            self.default("Connection", "close");
        }
        self.default("Host", &endpoint.host_header());
        self
    }

    fn has(&self, name: &str) -> bool {
        self.headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    fn default(&mut self, name: &str, value: &str) {
        if !self.has(name) {
            self.headers.push((name.to_string(), value.to_string()));
        }
    }
}

trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

/// Connects (within `open`), sends the request, and returns once the response head arrives
/// (within `read`).
pub async fn exchange(net: &Network, endpoint: &Endpoint, request: Request, timeouts: &Timeouts) -> Result<Response, HttpError> {
    let io = timeout(timeouts.open, connect(net, endpoint)).await.map_err(|_| HttpError::OpenTimeout)??;
    let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
        .title_case_headers(true)
        .handshake::<_, Full<Bytes>>(TokioIo::new(io))
        .await
        .map_err(HttpError::from_hyper)?;
    tokio::spawn(async move {
        let _ = connection.await;
    });

    let mut builder = hyper::Request::builder().method(request.method).uri(request.target);
    for (name, value) in &request.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let http_request = builder.body(Full::new(Bytes::from(request.body))).map_err(|e| HttpError::Http(e.to_string()))?;
    let response = timeout(timeouts.read, sender.send_request(http_request))
        .await
        .map_err(|_| HttpError::ReadTimeout)?
        .map_err(HttpError::from_hyper)?;

    let reason = response.extensions().get::<hyper::ext::ReasonPhrase>().map(|r| String::from_utf8_lossy(r.as_bytes()).into_owned());
    let (parts, body) = response.into_parts();
    let reason = reason.unwrap_or_else(|| parts.status.canonical_reason().unwrap_or("").to_string());
    Ok(Response {
        status: parts.status.as_u16(),
        reason,
        headers: parts.headers,
        body,
        read_timeout: timeouts.read,
        decode_content: request.decode_content,
    })
}

async fn connect(net: &Network, endpoint: &Endpoint) -> Result<Box<dyn Io>, HttpError> {
    let tcp = match endpoint.pinned_ip {
        Some(ip) => net.dialer.connect(SocketAddr::new(ip, endpoint.port)).await?,
        None => connect_resolving(net, endpoint).await?,
    };
    if !endpoint.https {
        return Ok(Box::new(tcp));
    }
    let server_name = ServerName::try_from(endpoint.bare_host().to_string()).map_err(|e| HttpError::Tls(e.to_string()))?;
    let tls = tokio_rustls::TlsConnector::from(net.tls.clone()).connect(server_name, tcp).await.map_err(HttpError::from_io)?;
    Ok(Box::new(tls))
}

/// `TCPSocket.open(host, port)`: each resolved address in turn.
async fn connect_resolving(net: &Network, endpoint: &Endpoint) -> Result<tokio::net::TcpStream, HttpError> {
    let addresses = match endpoint.bare_host().parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => net.resolver.lookup(endpoint.bare_host()).await.map_err(|e| HttpError::Unresolvable(e.to_string()))?,
    };
    let mut last_error = None;
    for ip in addresses {
        match net.dialer.connect(SocketAddr::new(ip, endpoint.port)).await {
            Ok(stream) => return Ok(stream),
            Err(e) => last_error = Some(e),
        }
    }
    Err(match last_error {
        Some(e) => HttpError::Io(e),
        None => HttpError::Unresolvable(endpoint.host.clone()),
    })
}

pub struct Response {
    pub status: u16,
    /// The status line's reason phrase (`response.message`).
    pub reason: String,
    pub headers: http::HeaderMap,
    body: Incoming,
    read_timeout: Duration,
    decode_content: bool,
}

/// What reading a body with a size limit produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Body {
    Complete(Vec<u8>),
    /// It ran past the limit; reading stopped there.
    TooLarge,
}

impl Response {
    /// `response[name]`: every value of the header, joined with ", ".
    pub fn header(&self, name: &str) -> Option<String> {
        let values: Vec<String> = self.headers.get_all(name).iter().map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).collect();
        if values.is_empty() { None } else { Some(values.join(", ")) }
    }

    /// `Net::HTTPHeader#content_type`: the media type before any `;`, main and sub type each
    /// stripped (not downcased), or just the main type when there's no `/`.
    pub fn content_type(&self) -> Option<String> {
        let header = self.header("content-type")?;
        let media = header.split(';').next().unwrap_or("");
        let mut parts = media.split('/');
        let main = ruby_compat::strip(parts.next().unwrap_or(""));
        match parts.next() {
            Some(sub) => Some(format!("{main}/{}", ruby_compat::strip(sub))),
            None => Some(main.to_string()),
        }
    }

    /// `Net::HTTPHeader#content_length`: the first run of digits, or `HTTPHeaderSyntaxError`.
    pub fn content_length(&self) -> Result<Option<u64>, HttpError> {
        let Some(header) = self.header("content-length") else { return Ok(None) };
        let digits: String = header.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return Err(HttpError::Http("wrong Content-Length format".into()));
        }
        Ok(Some(digits.parse().unwrap_or(u64::MAX)))
    }

    /// Reads the body (inflating it when `Net::HTTP` would), stopping once it would exceed
    /// `limit` bytes. `Net::HTTP` reads any size.
    pub async fn read_body(mut self, limit: usize) -> Result<Body, HttpError> {
        let mut inflater = self.inflater();
        let mut body = Vec::new();
        loop {
            let frame = timeout(self.read_timeout, self.body.frame()).await.map_err(|_| HttpError::ReadTimeout)?;
            let Some(frame) = frame else { break };
            let frame = frame.map_err(HttpError::from_hyper)?;
            let Ok(chunk) = frame.into_data() else { continue };
            let room = limit - body.len();
            let chunk = match &mut inflater {
                Some(inflater) => inflater.inflate(&chunk, room)?,
                None => Some(chunk.to_vec()),
            };
            let Some(chunk) = chunk.filter(|chunk| chunk.len() <= room) else { return Ok(Body::TooLarge) };
            body.extend_from_slice(&chunk);
        }
        if let Some(inflater) = inflater {
            let room = limit - body.len();
            let rest = inflater.finish()?;
            if rest.len() > room {
                return Ok(Body::TooLarge);
            }
            body.extend_from_slice(&rest);
        }
        Ok(Body::Complete(body))
    }

    /// `Net::HTTPResponse#inflater`: gzip, x-gzip or deflate, unless it's a range.
    fn inflater(&self) -> Option<Inflater> {
        if !self.decode_content || self.headers.contains_key("content-range") {
            return None;
        }
        match self.header("content-encoding")?.to_ascii_lowercase().as_str() {
            "gzip" | "x-gzip" | "deflate" => Some(Inflater::new()),
            _ => None,
        }
    }
}

/// `Zlib::Inflate.new(32 + Zlib::MAX_WBITS)`: gzip or zlib, detected from the header.
struct Inflater {
    decoder: Option<Decoder>,
    pending: Vec<u8>,
}

enum Decoder {
    Gzip(flate2::write::MultiGzDecoder<Vec<u8>>),
    Zlib(flate2::write::ZlibDecoder<Vec<u8>>),
}

impl Inflater {
    fn new() -> Self {
        Self { decoder: None, pending: Vec::new() }
    }

    /// The chunk inflated, or `None` once the output would pass `room` bytes.
    fn inflate(&mut self, chunk: &[u8], room: usize) -> Result<Option<Vec<u8>>, HttpError> {
        if self.decoder.is_none() {
            self.pending.extend_from_slice(chunk);
            if self.pending.len() < 2 {
                return Ok(Some(Vec::new()));
            }
            let pending = std::mem::take(&mut self.pending);
            self.decoder = Some(if pending.starts_with(&[0x1f, 0x8b]) {
                Decoder::Gzip(flate2::write::MultiGzDecoder::new(Vec::new()))
            } else {
                Decoder::Zlib(flate2::write::ZlibDecoder::new(Vec::new()))
            });
            return self.write(&pending, room);
        }
        self.write(chunk, room)
    }

    /// Feeds `data` to the decoder a step at a time (a step writes at most the decoder's 32 KB
    /// buffer) and stops as soon as the output passes `room`: deflate expands about 1000×, so
    /// inflating a whole network chunk before checking could allocate hundreds of megabytes.
    fn write(&mut self, mut data: &[u8], room: usize) -> Result<Option<Vec<u8>>, HttpError> {
        let decoder = self.decoder.as_mut().expect("decoder chosen");
        while !data.is_empty() {
            match decoder.write(data).map_err(inflate_error)? {
                0 => return Err(HttpError::Inflate("failed to write whole buffer".into())),
                written => data = &data[written..],
            }
            if decoder.output().len() > room {
                return Ok(None);
            }
        }
        Ok(Some(std::mem::take(decoder.output())))
    }

    /// The rest of the output: at most what the decoder still buffers.
    fn finish(mut self) -> Result<Vec<u8>, HttpError> {
        if self.decoder.is_none() {
            if self.pending.is_empty() {
                return Ok(Vec::new());
            }
            let pending = std::mem::take(&mut self.pending);
            self.decoder = Some(Decoder::Zlib(flate2::write::ZlibDecoder::new(Vec::new())));
            self.write(&pending, usize::MAX)?;
        }
        match self.decoder.expect("decoder chosen") {
            Decoder::Gzip(d) => d.finish(),
            Decoder::Zlib(d) => d.finish(),
        }
        .map_err(inflate_error)
    }
}

impl Decoder {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        match self {
            Decoder::Gzip(d) => d.write(data),
            Decoder::Zlib(d) => d.write(data),
        }
    }

    /// What's been inflated so far and not yet taken.
    fn output(&mut self) -> &mut Vec<u8> {
        match self {
            Decoder::Gzip(d) => d.get_mut(),
            Decoder::Zlib(d) => d.get_mut(),
        }
    }
}

fn inflate_error(error: io::Error) -> HttpError {
    HttpError::Inflate(error.to_string())
}

/// `URI::HTTP#request_uri`: path (at least "/") and query.
pub fn request_uri(url: &ember_richtext::uri::Uri) -> String {
    let path = url.path.as_deref().unwrap_or("");
    let target = match &url.query {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    };
    if target.starts_with('/') { target } else { format!("/{target}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::test_support::gzip_bomb;

    const LIMIT: usize = 5 * 1024 * 1024;

    /// A gigabyte packed into a megabyte stops inflating just past the limit, in one chunk.
    #[test]
    fn stops_inflating_a_gzip_bomb_at_the_limit() {
        let mut inflater = Inflater::new();
        let started = std::time::Instant::now();
        assert_eq!(inflater.inflate(&gzip_bomb(1024), LIMIT).unwrap(), None);
        // Inflating the whole gigabyte would take minutes; stopping at the limit takes about a
        // second in an unoptimized build on a CI runner.
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        let inflated = inflater.decoder.as_mut().unwrap().output().len();
        assert!(inflated <= LIMIT + 64 * 1024, "{inflated}");
    }

    #[test]
    fn inflates_bodies_within_the_limit() {
        let body = gzip_bomb(3);
        let mut inflater = Inflater::new();
        let (head, tail) = body.split_at(body.len() / 2);
        let mut out = inflater.inflate(head, LIMIT).unwrap().unwrap();
        out.extend(inflater.inflate(tail, LIMIT - out.len()).unwrap().unwrap());
        out.extend(inflater.finish().unwrap());
        assert_eq!(out, vec![0; 3 * 1024 * 1024]);
    }
}
