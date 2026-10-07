//! ActionDispatch::Static (actionpack/lib/action_dispatch/middleware/static.rb) over Rack::Files,
//! serving the embedded reference/public plus the precompiled public/assets, with the headers
//! from reference/config/environments/production.rb (`public_file_server.headers`).

use ruby_compat::rack::byte_ranges;
use std::borrow::Cow;

use crate::embedded;

/// The last `config.public_file_server.headers` assignment in production.rb wins.
const CACHE_CONTROL: &str = "public, max-age=2592000";

/// Rack::Files::MULTIPART_BOUNDARY
const MULTIPART_BOUNDARY: &str = "AaB03x";

/// The parts of a request ActionDispatch::Static looks at.
#[derive(Debug, Clone, Copy, Default)]
pub struct StaticRequest<'a> {
    pub method: &'a str,
    /// The raw (still percent-encoded) request path, without the query string.
    pub path: &'a str,
    pub accept_encoding: Option<&'a str>,
    pub range: Option<&'a str>,
    pub if_modified_since: Option<&'a str>,
}

pub type Body = Cow<'static, [u8]>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticResponse {
    pub status: u16,
    /// In the order Rack builds them. Names are as Rack spells them ("Cache-Control" comes
    /// from the app's config); HTTP/1.1 and HTTP/2 treat them case-insensitively.
    pub headers: Vec<(&'static str, String)>,
    /// Empty for HEAD requests.
    pub body: Body,
}

impl StaticResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// FileHandler#attempt: Some(response) when a public file matches a GET or HEAD request,
/// None to hand the request to the app.
pub fn serve(request: &StaticRequest) -> Option<StaticResponse> {
    if request.method != "GET" && request.method != "HEAD" {
        return None;
    }
    let (body, content_headers) = find_file(request.path, request.accept_encoding.unwrap_or(""))?;
    Some(serve_file(request, body, content_headers))
}

type ContentHeaders = Vec<(&'static str, String)>;

fn find_file(path_info: &str, accept_encoding: &str) -> Option<(&'static [u8], ContentHeaders)> {
    let path = clean_path(path_info)?;
    let extname = file_extname(&path);
    let content_type = mime_type(extname);

    let mut candidates = vec![(path.clone(), content_type.unwrap_or("text/plain"))];
    // Only paths without a resolvable extension also try .html and /index.html.
    if content_type.is_none() && extname != b".html" {
        candidates.push(([path.as_slice(), b".html"].concat(), "text/html"));
        candidates.push(([path.as_slice(), b"/index.html"].concat(), "text/html"));
    }

    candidates.into_iter().find_map(|(path, content_type)| try_files(&path, content_type, accept_encoding))
}

fn try_files(path: &[u8], content_type: &'static str, accept_encoding: &str) -> Option<(&'static [u8], ContentHeaders)> {
    let mut headers: ContentHeaders = vec![("content-type", content_type.to_string())];

    if !compressible(content_type) {
        return file(path).map(|body| (body, headers));
    }

    for (encoding, extension) in [("br", ".br"), ("gzip", ".gz")] {
        if let Some(body) = file(&[path, extension.as_bytes()].concat()) {
            headers.push(("vary", "accept-encoding".to_string()));
            if accepts(accept_encoding, encoding) {
                headers.push(("content-encoding", encoding.to_string()));
                return Some((body, headers));
            }
        }
    }
    file(path).map(|body| (body, headers))
}

/// Rack::Files#serving, then FileHandler#serve's `headers.update(content_headers)`.
fn serve_file(request: &StaticRequest, file: &'static [u8], content_headers: ContentHeaders) -> StaticResponse {
    let last_modified = embedded::BUILT_AT;
    if request.if_modified_since == Some(last_modified) {
        return StaticResponse { status: 304, headers: Vec::new(), body: Body::Borrowed(b"") };
    }

    let size = file.len();
    let mut headers: Vec<(&'static str, String)> = vec![
        ("last-modified", last_modified.to_string()),
        ("content-type", String::new()), // replaced by the content headers below
        ("Cache-Control", CACHE_CONTROL.to_string()),
    ];
    let mut status = 200;
    let mut body: Body = Body::Borrowed(file);

    // Within the file, so each end fits a usize.
    let ranges: Option<Vec<(usize, usize)>> = byte_ranges(request.range, size as u64)
        .map(|ranges| ranges.into_iter().map(|(start, end)| (start as usize, end as usize)).collect());
    match ranges {
        None => {}
        Some(ranges) if ranges.is_empty() => {
            let message = "Byte range unsatisfiable\n";
            headers = vec![
                ("content-type", String::new()),
                ("content-length", message.len().to_string()),
                ("x-cascade", "pass".to_string()),
                ("content-range", format!("bytes */{size}")),
            ];
            status = 416;
            body = Body::Borrowed(message.as_bytes());
        }
        Some(ranges) if ranges.len() == 1 => {
            let (start, end) = ranges[0];
            headers.push(("content-range", format!("bytes {start}-{end}/{size}")));
            status = 206;
            body = Body::Borrowed(&file[start..=end]);
        }
        Some(ranges) => {
            let content_type = content_headers[0].1.clone();
            let mut multipart = Vec::new();
            for &(start, end) in &ranges {
                multipart.extend_from_slice(
                    format!(
                        "\r\n--{MULTIPART_BOUNDARY}\r\ncontent-type: {content_type}\r\ncontent-range: bytes {start}-{end}/{size}\r\n\r\n"
                    )
                    .as_bytes(),
                );
                multipart.extend_from_slice(&file[start..=end]);
            }
            multipart.extend_from_slice(format!("\r\n--{MULTIPART_BOUNDARY}--\r\n").as_bytes());
            headers[1].1 = format!("multipart/byteranges; boundary={MULTIPART_BOUNDARY}");
            status = 206;
            body = Body::Owned(multipart);
        }
    }

    if status != 416 {
        headers.push(("content-length", body.len().to_string()));
    }

    for (name, value) in content_headers {
        // A multipart content-type is kept only if Static doesn't override it; it always does.
        match headers.iter_mut().find(|(n, _)| *n == name) {
            Some(existing) => existing.1 = value,
            None => headers.push((name, value)),
        }
    }

    if request.method == "HEAD" {
        body = Body::Borrowed(b"");
    }

    StaticResponse { status, headers, body }
}

/// FileHandler#clean_path: chomp("/"), percent-decode, reject NUL, then
/// Rack::Utils.clean_path_info. Works on bytes, as Rack does with a binary PATH_INFO.
fn clean_path(path_info: &str) -> Option<Vec<u8>> {
    let path = unescape_path(path_info.strip_suffix('/').unwrap_or(path_info));
    if path.contains(&0) {
        return None;
    }
    let parts: Vec<&[u8]> = path.split(|&b| b == b'/').collect();
    let mut clean: Vec<&[u8]> = Vec::new();
    for part in &parts {
        match *part {
            b"" | b"." => {}
            b".." => {
                clean.pop();
            }
            part => clean.push(part),
        }
    }
    let mut cleaned = clean.join(&b'/');
    if parts.first().is_none_or(|p| p.is_empty()) {
        cleaned.insert(0, b'/');
    }
    Some(cleaned)
}

/// URI::RFC2396_Parser#unescape: %XX sequences become bytes, anything else is kept.
fn unescape_path(path: &str) -> Vec<u8> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn hex_value(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

fn file(path: &[u8]) -> Option<&'static [u8]> {
    let path = std::str::from_utf8(path).ok()?;
    embedded::FILES.binary_search_by(|(url, _)| (*url).cmp(path)).ok().map(|index| embedded::FILES[index].1)
}

/// FileHandler's compressible_content_types: /\A(?:text\/|application\/javascript|image\/svg\+xml)/
fn compressible(content_type: &str) -> bool {
    content_type.starts_with("text/") || content_type.starts_with("application/javascript") || content_type.starts_with("image/svg+xml")
}

/// `accept_encoding.any? { |enc, _| /\b#{encoding}\b/i.match?(enc) }` over Rack's parsed header.
fn accepts(accept_encoding: &str, encoding: &str) -> bool {
    accept_encoding.split(',').map(|part| part.split(';').next().unwrap_or("").trim().to_ascii_lowercase()).any(|value| {
        value.match_indices(encoding).any(|(i, _)| {
            let word = |b: Option<&u8>| b.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
            !word(i.checked_sub(1).and_then(|j| value.as_bytes().get(j))) && !word(value.as_bytes().get(i + encoding.len()))
        })
    })
}

/// File.extname on bytes.
fn file_extname(path: &[u8]) -> &[u8] {
    let base = path.rsplit(|&b| b == b'/').next().unwrap_or(path);
    let start = base.iter().position(|&b| b != b'.').unwrap_or(base.len());
    let trimmed = &base[start..];
    match trimmed.iter().rposition(|&b| b == b'.') {
        Some(dot) => &trimmed[dot..],
        None => b"",
    }
}

/// Rack::Mime.mime_type(ext, nil) for the extensions a Matchbox deploy serves (and the other
/// common web ones); lookups are case-insensitive.
fn mime_type(extname: &[u8]) -> Option<&'static str> {
    let ext = String::from_utf8_lossy(extname).to_ascii_lowercase();
    Some(match ext.as_str() {
        ".avif" => "image/avif",
        ".css" => "text/css",
        ".csv" => "text/csv",
        ".gif" => "image/gif",
        ".gz" => "application/x-gzip",
        ".htm" | ".html" => "text/html",
        ".ico" => "image/vnd.microsoft.icon",
        ".jpeg" | ".jpg" => "image/jpeg",
        ".js" | ".mjs" => "text/javascript",
        ".json" => "application/json",
        ".m4a" => "audio/mp4a-latm",
        ".mp3" => "audio/mpeg",
        ".mp4" => "video/mp4",
        ".ogg" => "application/ogg",
        ".otf" => "font/otf",
        ".pdf" => "application/pdf",
        ".png" => "image/png",
        ".svg" => "image/svg+xml",
        ".ttf" => "font/ttf",
        ".txt" => "text/plain",
        ".wav" => "audio/x-wav",
        ".webm" => "video/webm",
        ".webp" => "image/webp",
        ".woff" => "font/woff",
        ".woff2" => "font/woff2",
        ".xml" => "application/xml",
        ".zip" => "application/zip",
        _ => return None,
    })
}
