//! Thruster's compression (`internal/compression_handler.go`, `compression_guard_handler.go`),
//! which is klauspost/compress v1.18.6's `gzhttp` with a 1 KB minimum size, gzip level 6, zstd
//! (preferred over gzip at equal quality) and 32 bytes of BREACH jitter.
//!
//! The app's own `Rack::Deflater` (crate::deflater) already gzips nearly every response for
//! clients that accept gzip, and `gzhttp` leaves encoded responses alone. What it adds is
//! `Vary: Accept-Encoding` on every response, and compression of what the app sent unencoded:
//! zstd for clients that accept zstd but not gzip, and bodies `Rack::Deflater` skipped.

use std::io::Write;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, header};
use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use futures_util::stream::{self, BoxStream};

/// `gzhttp.HeaderNoCompression`: set by the guard to veto compression, never sent.
const NO_COMPRESSION: &str = "no-gzip-compression";
const MIN_SIZE: usize = 1024;
const GZIP_LEVEL: u32 = 6;
/// `zstd.SpeedFastest`
const ZSTD_LEVEL: i32 = 1;
/// `RandomJitter(n, 0, false)`: jitter is derived from (at most) the first 64 KB of the body.
const JITTER_BUFFER: usize = 64 << 10;
/// net/http's `bufferBeforeChunkingSize`.
const GO_CHUNKING_BUFFER: usize = 2048;

#[derive(Debug, Clone)]
pub struct Compression {
    /// `strings.Repeat("Padding-", 1+(n/8))[:n+1]`, or empty without jitter.
    jitter: String,
    disable_on_auth: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    None,
    Gzip,
    Zstd,
}

/// How the response's own headers were combined with the `Vary: Accept-Encoding` gzhttp set
/// before calling on: Thruster's cache copies stored or recorded headers over it (a `Vary` of
/// the response's replaces it), while bypassed requests add theirs to it (so `Vary` repeats).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderMerge {
    Replace,
    Append,
}

/// What `gzhttp` decided about a request before calling on.
pub struct Negotiation {
    encoding: Encoding,
    user_specific_request: bool,
}

impl Compression {
    pub fn new(jitter: i64, disable_on_auth: bool) -> Self {
        let jitter = if jitter > 0 {
            let n = jitter as usize;
            "Padding-".repeat(1 + n / 8)[..n + 1].to_string()
        } else {
            String::new()
        };
        Self { jitter, disable_on_auth }
    }

    pub fn negotiate<B>(&self, request: &Request<B>) -> Negotiation {
        let accept_encoding = request.headers().get(header::ACCEPT_ENCODING).and_then(|v| v.to_str().ok());
        Negotiation {
            encoding: select_encoding(request.method(), accept_encoding),
            user_specific_request: self.disable_on_auth && has_user_specific_request_headers(request.headers()),
        }
    }

    pub async fn apply(&self, negotiation: Negotiation, mut response: Response<Body>, merge: HeaderMerge) -> Response<Body> {
        add_vary(response.headers_mut(), merge);
        if response.status().is_informational() {
            return response;
        }
        let guarded = negotiation.user_specific_request || (self.disable_on_auth && has_user_specific_response_headers(response.headers()));
        let vetoed = guarded || response.headers().contains_key(NO_COMPRESSION);
        response.headers_mut().remove(NO_COMPRESSION);
        if negotiation.encoding == Encoding::None || vetoed || !may_compress(response.headers()) {
            return response;
        }

        // Hold the start of the body until it's clear whether it's long enough (and, with jitter,
        // until there's enough of it to derive the jitter from).
        let want = if self.jitter.is_empty() { MIN_SIZE } else { JITTER_BUFFER.max(MIN_SIZE) };
        let (mut parts, body) = response.into_parts();
        let had_length = hyper::body::Body::size_hint(&body).exact().is_some();
        let mut stream = body.into_data_stream();
        let mut buffered = BytesMut::new();
        let mut ended = false;
        while buffered.len() < want {
            match stream.next().await {
                Some(Ok(chunk)) => buffered.extend_from_slice(&chunk),
                Some(Err(error)) => {
                    // Pass on what came and then the error, so the response is cut short rather
                    // than completed.
                    let body = stream::iter([Ok(buffered.freeze()), Err(error)]);
                    return Response::from_parts(parts, Body::from_stream(body));
                }
                None => {
                    ended = true;
                    break;
                }
            }
        }
        let buffered = buffered.freeze();

        let body_allowed = !matches!(parts.status, StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED);
        let mut content_type = parts.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        if content_type.is_empty() && body_allowed && !buffered.is_empty() {
            content_type = detect_content_type(&buffered).to_string();
            if !parts.headers.contains_key(header::CONTENT_TYPE) {
                parts.headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(&content_type).unwrap());
            }
        }
        let long_enough = buffered.len() >= MIN_SIZE;
        let rest: BoxStream<'static, Result<Bytes, axum::Error>> =
            if ended { stream::empty().boxed() } else { stream.map(|r| r.map_err(axum::Error::new)).boxed() };
        if !(long_enough && content_type_filter(&content_type)) {
            if ended && had_length {
                return Response::from_parts(parts, Body::from(buffered));
            }
            let body = stream::once(async move { Ok::<_, axum::Error>(buffered) }).chain(rest);
            return Response::from_parts(parts, Body::from_stream(body));
        }

        let encoding = negotiation.encoding;
        parts.headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static(if encoding == Encoding::Gzip { "gzip" } else { "zstd" }));
        parts.headers.remove(header::CONTENT_LENGTH);
        parts.headers.remove(header::ACCEPT_RANGES);
        let jitter = self.jitter_for(&buffered);
        let mut encoder = Encoder::new(encoding, jitter);
        if ended {
            let compressed = encoder.write(&buffered).and_then(|head| {
                let mut whole = BytesMut::from(&head[..]);
                whole.extend_from_slice(&encoder.finish()?);
                Ok(whole.freeze())
            });
            return match compressed {
                // Go's server gives a response a length when the handler returns with all of it
                // still in its 2 KB chunking buffer, and sends anything longer chunked.
                Ok(compressed) if compressed.len() <= GO_CHUNKING_BUFFER => Response::from_parts(parts, Body::from(compressed)),
                Ok(compressed) => {
                    Response::from_parts(parts, Body::from_stream(stream::once(async move { Ok::<_, axum::Error>(compressed) })))
                }
                Err(error) => {
                    Response::from_parts(parts, Body::from_stream(stream::once(async move { Err::<Bytes, _>(axum::Error::new(error)) })))
                }
            };
        }
        Response::from_parts(parts, Body::from_stream(compress_stream(encoder, buffered, rest)))
    }

    /// `startCompression`'s jitter: a prefix of the padding whose length (1..=n) comes from the
    /// CRC-32C of the start of the body.
    fn jitter_for(&self, buffered: &[u8]) -> Option<String> {
        if self.jitter.is_empty() {
            return None;
        }
        let sample = &buffered[..buffered.len().min(JITTER_BUFFER)];
        let rng = crc32c(sample).rotate_left(19) ^ 0xab0755de;
        let len = 1 + (rng % (self.jitter.len() as u32 - 1)) as usize;
        Some(self.jitter[..len].to_string())
    }
}

/// `gzhttp` adds `Vary: Accept-Encoding` to every response before the handler runs.
fn add_vary(headers: &mut HeaderMap, merge: HeaderMerge) {
    let existing: Vec<HeaderValue> = headers.get_all(header::VARY).iter().cloned().collect();
    if merge == HeaderMerge::Replace && !existing.is_empty() {
        return;
    }
    headers.remove(header::VARY);
    headers.append(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    for value in existing {
        headers.append(header::VARY, value);
    }
}

/// The checks `gzhttp` can make from the headers alone: not already encoded, not a range, and
/// (when the length and type are known) at least 1 KB of a compressible type.
fn may_compress(headers: &HeaderMap) -> bool {
    let get = |name| headers.get(name).and_then(|v: &HeaderValue| v.to_str().ok()).unwrap_or("");
    if !get(header::CONTENT_ENCODING).is_empty() || !get(header::CONTENT_RANGE).is_empty() {
        return false;
    }
    let content_length: usize = get(header::CONTENT_LENGTH).parse().unwrap_or(0);
    let content_type = get(header::CONTENT_TYPE);
    content_length == 0 || (content_length >= MIN_SIZE && (content_type.is_empty() || content_type_filter(content_type)))
}

/// `selectEncoding`: never for HEAD; zstd when its quality is at least gzip's.
pub fn select_encoding(method: &Method, accept_encoding: Option<&str>) -> Encoding {
    if method == Method::HEAD {
        return Encoding::None;
    }
    let Some(accept_encoding) = accept_encoding.filter(|ae| !ae.is_empty()) else { return Encoding::None };
    let gzip = quality(accept_encoding, "gzip");
    let zstd = quality(accept_encoding, "zstd");
    match (gzip > 0.0, zstd > 0.0) {
        (false, false) => Encoding::None,
        (true, false) => Encoding::Gzip,
        (false, true) => Encoding::Zstd,
        (true, true) if gzip > zstd => Encoding::Gzip,
        (true, true) => Encoding::Zstd,
    }
}

/// `parseEncodingQValue`: the first listed coding's quality (1 without a `q`), else 0.
fn quality(header: &str, encoding: &str) -> f64 {
    for part in header.trim().split(',') {
        let mut pieces = part.split(';');
        let coding = pieces.next().unwrap_or("").trim().to_ascii_lowercase();
        if coding != encoding {
            continue;
        }
        let mut q = 1.0;
        for piece in pieces {
            if let Some(value) = piece.trim().strip_prefix("q=") {
                q = value.parse::<f64>().unwrap_or(0.0).clamp(0.0, 1.0);
            }
        }
        return q;
    }
    0.0
}

/// Thruster's `contentTypeFilter`: gzhttp's default filter, minus already-compressed images.
pub fn content_type_filter(content_type: &str) -> bool {
    let content_type = content_type.trim().to_ascii_lowercase();
    if content_type.is_empty() {
        return true;
    }
    const EXCLUDE_CONTAINS: [&str; 8] = ["compress", "zip", "snappy", "lzma", "xz", "zstd", "brotli", "stuffit"];
    const EXCLUDE_PREFIX: [&str; 3] = ["video/", "audio/", "image/jp"];
    const COMPRESSED_IMAGES: [&str; 10] = [
        "image/jpeg", "image/jpg", "image/png", "image/apng", "image/webp", "image/gif", "image/avif", "image/heic", "image/heif",
        "image/jxl",
    ];
    !(EXCLUDE_CONTAINS.iter().any(|s| content_type.contains(s))
        || EXCLUDE_PREFIX.iter().chain(COMPRESSED_IMAGES.iter()).any(|p| content_type.starts_with(p)))
}

/// `hasUserSpecificRequestHeaders` (GZIP_COMPRESSION_DISABLE_ON_AUTH)
fn has_user_specific_request_headers(headers: &HeaderMap) -> bool {
    ["cookie", "authorization", "x-csrf-token"].iter().any(|name| headers.get(*name).is_some_and(|v| !v.is_empty()))
}

/// `hasUserSpecificResponseHeaders`
fn has_user_specific_response_headers(headers: &HeaderMap) -> bool {
    let get = |name| headers.get(name).and_then(|v: &HeaderValue| v.to_str().ok()).unwrap_or("");
    if !get(header::SET_COOKIE).is_empty() {
        return true;
    }
    let cache_control = get(header::CACHE_CONTROL).to_ascii_lowercase();
    if cache_control.split(',').map(|d| d.trim().split('=').next().unwrap_or("")).any(|d| d == "private" || d == "no-store") {
        return true;
    }
    get(header::VARY).split(',').any(|token| token.trim().eq_ignore_ascii_case("cookie"))
}

/// A streaming gzip or zstd encoder.
enum Encoder {
    Gzip(flate2::write::GzEncoder<Vec<u8>>),
    Zstd(zstd::stream::write::Encoder<'static, Vec<u8>>, Option<String>),
}

impl Encoder {
    fn new(encoding: Encoding, jitter: Option<String>) -> Self {
        match encoding {
            Encoding::Zstd => Encoder::Zstd(zstd::stream::write::Encoder::new(Vec::new(), ZSTD_LEVEL).expect("zstd encoder"), jitter),
            _ => {
                let mut builder = flate2::GzBuilder::new();
                if let Some(jitter) = jitter {
                    builder = builder.comment(jitter);
                }
                Encoder::Gzip(builder.write(Vec::new(), flate2::Compression::new(GZIP_LEVEL)))
            }
        }
    }

    fn write(&mut self, data: &[u8]) -> std::io::Result<Bytes> {
        let output = match self {
            Encoder::Gzip(encoder) => {
                encoder.write_all(data)?;
                encoder.get_mut()
            }
            Encoder::Zstd(encoder, _) => {
                encoder.write_all(data)?;
                encoder.get_mut()
            }
        };
        Ok(Bytes::from(std::mem::take(output)))
    }

    fn finish(self) -> std::io::Result<Bytes> {
        match self {
            Encoder::Gzip(encoder) => Ok(Bytes::from(encoder.finish()?)),
            Encoder::Zstd(encoder, jitter) => {
                let mut output = encoder.finish()?;
                // The jitter goes in a skippable frame (RFC 8878 3.1.2) after the data.
                if let Some(jitter) = jitter {
                    output.extend_from_slice(&0x184D2A50u32.to_le_bytes());
                    output.extend_from_slice(&(jitter.len() as u32).to_le_bytes());
                    output.extend_from_slice(jitter.as_bytes());
                }
                Ok(Bytes::from(output))
            }
        }
    }
}

fn compress_stream(
    encoder: Encoder,
    first: Bytes,
    rest: BoxStream<'static, Result<Bytes, axum::Error>>,
) -> impl futures_util::Stream<Item = Result<Bytes, axum::Error>> + Send + 'static {
    let chunks = stream::once(async move { Ok(first) }).chain(rest);
    stream::unfold((Some(encoder), chunks.boxed()), |(encoder, mut chunks)| async move {
        let mut encoder = encoder?;
        loop {
            match chunks.next().await {
                Some(Ok(chunk)) => match encoder.write(&chunk) {
                    Ok(output) if output.is_empty() => continue,
                    Ok(output) => return Some((Ok(output), (Some(encoder), chunks))),
                    Err(error) => return Some((Err(axum::Error::new(error)), (None, chunks))),
                },
                Some(Err(error)) => return Some((Err(error), (None, chunks))),
                None => {
                    let output = encoder.finish().map_err(axum::Error::new);
                    return Some((output, (None, chunks)));
                }
            }
        }
    })
}

/// A subset of Go's `http.DetectContentType`, for bodies sent without a `Content-Type` (the app
/// always sends one, so this is only a fallback).
fn detect_content_type(data: &[u8]) -> &'static str {
    let data = &data[..data.len().min(512)];
    let trimmed = {
        let start = data.iter().position(|b| !matches!(b, b'\t' | b'\n' | 0x0c | b'\r' | b' ')).unwrap_or(data.len());
        &data[start..]
    };
    const HTML: [&[u8]; 17] = [
        b"<!DOCTYPE HTML", b"<HTML", b"<HEAD", b"<SCRIPT", b"<IFRAME", b"<H1", b"<DIV", b"<FONT", b"<TABLE", b"<A", b"<STYLE", b"<TITLE",
        b"<B", b"<BODY", b"<BR", b"<P", b"<!--",
    ];
    for signature in HTML {
        if trimmed.len() > signature.len()
            && trimmed[..signature.len()].eq_ignore_ascii_case(signature)
            && matches!(trimmed[signature.len()], b' ' | b'>')
        {
            return "text/html; charset=utf-8";
        }
    }
    if trimmed.starts_with(b"<?xml") {
        return "text/xml; charset=utf-8";
    }
    let exact: [(&[u8], &'static str); 9] = [
        (b"%PDF-", "application/pdf"),
        (b"%!PS-Adobe-", "application/postscript"),
        (b"\xFE\xFF", "text/plain; charset=utf-16be"),
        (b"\xFF\xFE", "text/plain; charset=utf-16le"),
        (b"\xEF\xBB\xBF", "text/plain; charset=utf-8"),
        (b"GIF87a", "image/gif"),
        (b"GIF89a", "image/gif"),
        (b"\x89PNG\x0D\x0A\x1A\x0A", "image/png"),
        (b"\xFF\xD8\xFF", "image/jpeg"),
    ];
    for (signature, content_type) in exact {
        if data.starts_with(signature) {
            return content_type;
        }
    }
    if data.len() >= 14 && &data[..4] == b"RIFF" && &data[8..14] == b"WEBPVP" {
        return "image/webp";
    }
    if data.starts_with(b"\x1F\x8B\x08") {
        return "application/x-gzip";
    }
    if data.starts_with(b"PK\x03\x04") {
        return "application/zip";
    }
    let binary = data.iter().any(|&b| b <= 0x08 || b == 0x0B || (0x0E..=0x1A).contains(&b) || (0x1C..=0x1F).contains(&b));
    if binary { "application/octet-stream" } else { "text/plain; charset=utf-8" }
}

/// CRC-32C (Castagnoli), as `crc32.Update(0, castagnoliTable, b)`.
fn crc32c(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x82F6_3B78 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use std::io::Read;

    fn response(content_type: &str, body: impl Into<Bytes>) -> Response<Body> {
        let mut response = Response::new(Body::from(body.into()));
        if !content_type.is_empty() {
            response.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap());
        }
        response
    }

    fn negotiation(accept_encoding: &str) -> Negotiation {
        let request = Request::get("/").header("accept-encoding", accept_encoding).body(()).unwrap();
        Compression::new(32, false).negotiate(&request)
    }

    async fn apply(accept_encoding: &str, response: Response<Body>) -> (HeaderMap, Bytes) {
        let response = Compression::new(32, false).apply(negotiation(accept_encoding), response, HeaderMerge::Append).await;
        let (parts, body) = response.into_parts();
        (parts.headers, body.collect().await.unwrap().to_bytes())
    }

    fn vary(headers: &HeaderMap) -> Vec<&str> {
        headers.get_all(header::VARY).iter().map(|v| v.to_str().unwrap()).collect()
    }

    #[test]
    fn selects_zstd_at_equal_quality() {
        assert_eq!(select_encoding(&Method::GET, Some("gzip, deflate, br, zstd")), Encoding::Zstd);
        assert_eq!(select_encoding(&Method::GET, Some("gzip, zstd;q=0.5")), Encoding::Gzip);
        assert_eq!(select_encoding(&Method::GET, Some("gzip")), Encoding::Gzip);
        assert_eq!(select_encoding(&Method::GET, Some("zstd")), Encoding::Zstd);
        assert_eq!(select_encoding(&Method::GET, Some("gzip;q=0, br")), Encoding::None);
        assert_eq!(select_encoding(&Method::GET, None), Encoding::None);
        assert_eq!(select_encoding(&Method::HEAD, Some("gzip")), Encoding::None);
    }

    #[test]
    fn filters_compressed_types() {
        assert!(content_type_filter("text/html; charset=utf-8"));
        assert!(content_type_filter("image/svg+xml"));
        assert!(!content_type_filter("image/png"));
        assert!(!content_type_filter("image/jpeg"));
        assert!(!content_type_filter("video/mp4"));
        assert!(!content_type_filter("application/zip"));
        assert!(!content_type_filter("application/gzip"));
    }

    #[tokio::test]
    async fn compresses_unencoded_bodies_with_zstd() {
        let body = "hello ember ".repeat(200);
        let (headers, bytes) = apply("zstd", response("text/html; charset=utf-8", body.clone())).await;
        assert_eq!(headers[header::CONTENT_ENCODING], "zstd");
        assert_eq!(vary(&headers), ["Accept-Encoding"]);
        let decoded = zstd::stream::decode_all(&bytes[..]).unwrap();
        assert_eq!(decoded, body.as_bytes());
    }

    #[tokio::test]
    async fn compresses_with_gzip_and_a_jitter_comment() {
        let body = "hello ember ".repeat(200);
        let (headers, bytes) = apply("gzip", response("text/css", body.clone())).await;
        assert_eq!(headers[header::CONTENT_ENCODING], "gzip");
        let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
        let mut decoded = String::new();
        decoder.read_to_string(&mut decoded).unwrap();
        assert_eq!(decoded, body);
        let comment = decoder.header().unwrap().comment().unwrap();
        assert!(!comment.is_empty() && comment.len() <= 32 && b"Padding-Padding-Padding-Padding-P".starts_with(comment));
    }

    #[tokio::test]
    async fn a_failing_body_fails_the_response() {
        let chunks = [Ok(Bytes::from("hello ember ".repeat(100))), Err(std::io::Error::other("disk gone"))];
        let failing = Response::builder().header(header::CONTENT_TYPE, "text/html").body(Body::from_stream(stream::iter(chunks))).unwrap();
        let response = Compression::new(32, false).apply(negotiation("gzip"), failing, HeaderMerge::Append).await;
        assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
        assert!(response.into_body().collect().await.is_err());
    }

    #[tokio::test]
    async fn leaves_encoded_small_and_incompressible_bodies_alone() {
        let body = "x".repeat(2000);
        let mut encoded = response("text/html", body.clone());
        encoded.headers_mut().insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        encoded.headers_mut().insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        let (headers, bytes) = apply("gzip", encoded).await;
        assert_eq!(headers[header::CONTENT_ENCODING], "gzip");
        assert_eq!(vary(&headers), ["Accept-Encoding", "Accept-Encoding"]);
        assert_eq!(bytes.len(), 2000);

        let (headers, _) = apply("gzip", response("text/html", "small")).await;
        assert!(!headers.contains_key(header::CONTENT_ENCODING));
        let (headers, _) = apply("gzip", response("image/png", body.clone())).await;
        assert!(!headers.contains_key(header::CONTENT_ENCODING));
        let (headers, bytes) = apply("", response("text/html", body.clone())).await;
        assert!(!headers.contains_key(header::CONTENT_ENCODING));
        assert_eq!(bytes.len(), 2000);
    }

    #[tokio::test]
    async fn vary_is_replaced_by_the_cache_and_appended_otherwise() {
        let mut with_vary = response("text/html", "x");
        with_vary.headers_mut().insert(header::VARY, HeaderValue::from_static("Accept"));
        let replaced = Compression::new(32, false).apply(negotiation("gzip"), with_vary, HeaderMerge::Replace).await;
        assert_eq!(vary(replaced.headers()), ["Accept"]);
        let plain = Compression::new(32, false).apply(negotiation("gzip"), response("text/html", "x"), HeaderMerge::Replace).await;
        assert_eq!(vary(plain.headers()), ["Accept-Encoding"]);
    }

    #[tokio::test]
    async fn sniffs_a_missing_content_type() {
        let (headers, _) = apply("gzip", response("", "<!DOCTYPE html><p>".repeat(100))).await;
        assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(headers[header::CONTENT_ENCODING], "gzip");
    }

    #[tokio::test]
    async fn guard_vetoes_user_specific_responses() {
        let compression = Compression::new(32, true);
        let request = Request::get("/").header("accept-encoding", "gzip").header("cookie", "a=b").body(()).unwrap();
        let response =
            compression.apply(compression.negotiate(&request), response("text/html", "x".repeat(2000)), HeaderMerge::Append).await;
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
        let request = Request::get("/").header("accept-encoding", "gzip").body(()).unwrap();
        let mut private = response_with_cc("private");
        private.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
        let response = compression.apply(compression.negotiate(&request), private, HeaderMerge::Append).await;
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
    }

    fn response_with_cc(cache_control: &'static str) -> Response<Body> {
        let mut response = Response::new(Body::from("x".repeat(2000)));
        response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache_control));
        response
    }

    #[test]
    fn crc32c_matches_the_standard_check_value() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }
}
