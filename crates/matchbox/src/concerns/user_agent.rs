//! A port of the useragent gem (0.16.11), which Rails' `allow_browser` and platform_agent use to
//! read the User-Agent header: `UserAgent.parse` splits the header into products, and the first
//! of `UserAgent::Browsers::ALL` whose `extend?` accepts them decides how `browser`, `version`,
//! `platform`, `os`, `bot?` and `mobile?` are answered.
//!
//! Where the gem raises (a `NoMethodError` on nil, say), the `try_*` methods return `Err(Raised)`;
//! the plain methods fall back to nil. Ruby's `\d` and `\s` are ASCII-only and so are the
//! hand-written matchers here. `^` and `$` are treated as string anchors: a header value cannot
//! contain a newline.

use std::cmp::Ordering;
use std::fmt;

/// The gem raised (NoMethodError/ArgumentError) instead of answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Raised;

pub type Rb<T> = Result<T, Raised>;

const DEFAULT_USER_AGENT: &str = "Mozilla/4.0 (compatible)";

/// `UserAgent.parse`: blank strings parse as "Mozilla/4.0 (compatible)".
pub fn parse(user_agent: &str) -> Agent {
    let mut rest = if ruby_compat::strip(user_agent).is_empty() { DEFAULT_USER_AGENT } else { user_agent };

    let mut products = Vec::new();
    while let Some((length, product)) = match_product(rest) {
        products.push(product);
        rest = ruby_compat::strip(&rest[length..]);
    }

    let kind = Kind::ALL.into_iter().find(|kind| kind.extends(&products)).unwrap_or(Kind::Base);
    Agent { kind, products }
}

// ---------------------------------------------------------------------------------------------
// UserAgent::Version

/// `UserAgent::Version`. Equality is string equality; ordering is the gem's `<=>`, which is not a
/// total order (a non-numeric version sorts below everything it isn't equal to, from either side),
/// so there is no `Ord`.
#[derive(Debug, Clone)]
pub struct Version {
    string: String,
    blank: bool,
    comparable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// A run of digits, leading zeros stripped ("0" for zero), compared as an unbounded integer.
    Int(String),
    Str(String),
}

impl Segment {
    fn int(digits: &str) -> Self {
        let trimmed = digits.trim_start_matches('0');
        Segment::Int(if trimmed.is_empty() { "0".into() } else { trimmed.into() })
    }

    fn as_u64(&self) -> Option<u64> {
        match self {
            Segment::Int(digits) => digits.parse().ok(),
            Segment::Str(_) => None,
        }
    }
}

impl Version {
    pub fn new(string: &str) -> Self {
        let blank = string.chars().all(is_ruby_space);
        let digits = string.chars().take_while(char::is_ascii_digit).count();
        let comparable = !blank && digits > 0 && (digits == string.len() || string[digits..].starts_with('.'));
        Version { string: string.to_string(), blank, comparable }
    }

    /// `Version#nil?`: the string is empty or whitespace.
    pub fn is_nil(&self) -> bool {
        self.blank
    }

    /// `version.to_s.present?` (ActiveSupport's Unicode-whitespace `blank?`).
    pub fn is_present(&self) -> bool {
        is_present(&self.string)
    }

    pub fn as_str(&self) -> &str {
        &self.string
    }

    /// `Version#to_a`, split out when asked for rather than in `new`: a parse makes a version for
    /// every product, and only comparisons read the segments.
    pub fn to_a(&self) -> Vec<Segment> {
        if self.blank {
            Vec::new()
        } else if self.comparable {
            scan_sequences(&self.string)
        } else {
            vec![Segment::Str(self.string.clone())]
        }
    }

    /// `Version#<=>` against another version: only the first six segments count.
    pub fn ruby_cmp(&self, other: &Version) -> Ordering {
        if self.comparable {
            let (ours, theirs) = (self.to_a(), other.to_a());
            let zero = Segment::Int("0".into());
            for i in 0..6 {
                let a = ours.get(i).unwrap_or(&zero);
                let b = theirs.get(i).unwrap_or(&zero);
                match (a, b) {
                    (Segment::Str(_), Segment::Int(_)) => return Ordering::Less,
                    (Segment::Int(_), Segment::Str(_)) => return Ordering::Greater,
                    _ if a == b => continue,
                    (Segment::Int(x), Segment::Int(y)) => {
                        return x.len().cmp(&y.len()).then_with(|| x.cmp(y));
                    }
                    (Segment::Str(x), Segment::Str(y)) => return x.as_bytes().cmp(y.as_bytes()),
                }
            }
            Ordering::Equal
        } else if self.string == other.string {
            Ordering::Equal
        } else {
            Ordering::Less
        }
    }
}

/// `str.scan(/\d+|[A-Za-z][0-9A-Za-z-]*$/)`.
fn scan_sequences(string: &str) -> Vec<Segment> {
    let bytes = string.as_bytes();
    let mut sequences = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            sequences.push(Segment::int(&string[start..i]));
        } else if bytes[i].is_ascii_alphabetic() {
            let mut end = i + 1;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-') {
                end += 1;
            }
            if end == bytes.len() {
                sequences.push(Segment::Str(string[i..].to_string()));
                i = end;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    sequences
}

impl Default for Version {
    /// `Version.new(nil)`.
    fn default() -> Self {
        Version::new("")
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.string == other.string
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.ruby_cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.string)
    }
}

// ---------------------------------------------------------------------------------------------
// UserAgent (one product) and parsing

#[derive(Debug, Clone)]
struct Product {
    product: String,
    version: Version,
    comment: Option<Vec<String>>,
}

impl Product {
    fn comment_at(&self, index: usize) -> Option<&str> {
        self.comment.as_ref().and_then(|comment| comment.get(index)).map(String::as_str)
    }

    fn joined_comment(&self) -> Option<String> {
        self.comment.as_ref().map(|comment| comment.join("; "))
    }
}

/// `\s` in a Ruby regexp.
fn is_ruby_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

/// `a.downcase == b.downcase`. Two ASCII strings can compare byte by byte, ignoring ASCII case:
/// lowercasing them changes only A-Z. Anything else is lowercased first, because a non-ASCII
/// character can lowercase to an ASCII one (U+212A KELVIN SIGN to "k").
fn same_ignoring_case(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() { a.eq_ignore_ascii_case(b) } else { a.to_lowercase() == b.to_lowercase() }
}

/// `haystack.downcase.include?(needle)` for a non-empty, lowercase ASCII `needle`, with the same
/// ASCII fast path as [`same_ignoring_case`]. For short haystacks: product names and comments.
/// A whole header is quicker to lowercase once and search (`String::to_lowercase` has its own
/// ASCII path).
fn contains_ignoring_case(haystack: &str, needle: &str) -> bool {
    debug_assert!(!needle.is_empty() && needle.bytes().all(|b| b.is_ascii() && !b.is_ascii_uppercase()), "{needle:?}");
    if haystack.is_ascii() {
        haystack.as_bytes().windows(needle.len()).any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
    } else {
        haystack.to_lowercase().contains(needle)
    }
}

/// ActiveSupport's `present?` for strings.
pub(crate) fn is_present(string: &str) -> bool {
    !string.chars().all(char::is_whitespace)
}

/// `UserAgent::MATCHER` applied at the start of `s`:
/// `^['"]*([^/\s]+)/?([^\s,]*)(\s\(([^\)]*)\)|,gzip\(gfe\))?`. Returns the match length in bytes.
///
/// It scans bytes. Every delimiter the pattern names is ASCII, and every byte of a multibyte
/// character is 0x80 or above, which the product, version and comment classes all accept, so each
/// scan stops on a character boundary (even when `start + 1` lands inside the first character).
fn match_product(s: &str) -> Option<(usize, Product)> {
    let bytes = s.as_bytes();
    let is_space = |b: u8| is_ruby_space(b as char);
    let is_product_byte = |b: u8| b != b'/' && !is_space(b);
    let quotes = run(bytes, |b| b == b'\'' || b == b'"');

    let start = if bytes.get(quotes).is_some_and(|&b| is_product_byte(b)) {
        quotes
    } else if quotes > 0 {
        quotes - 1 // backtrack: the last quote is the product
    } else {
        return None;
    };

    let mut i = start + 1;
    i += run(&bytes[i..], is_product_byte);
    let product = &s[start..i];

    if bytes.get(i) == Some(&b'/') {
        i += 1;
    }
    let version_start = i;
    i += run(&bytes[i..], |b| !is_space(b) && b != b',');
    let version = &s[version_start..i];

    let mut comment = None;
    if bytes.get(i).is_some_and(|&b| is_space(b)) && bytes.get(i + 1) == Some(&b'(') {
        if let Some(close) = bytes[i + 2..].iter().position(|&b| b == b')') {
            comment = Some(&s[i + 2..i + 2 + close]);
            i += 2 + close + 1;
        }
    } else if bytes[i..].starts_with(b",gzip(gfe)") {
        i += ",gzip(gfe)".len();
    }

    let product =
        Product { product: product.to_string(), version: Version::new(version), comment: comment.map(|comment| ruby_split(comment, "; ")) };
    Some((i, product))
}

/// `String#split(separator)`: trailing empty fields are dropped.
fn ruby_split(string: &str, separator: &str) -> Vec<String> {
    let mut parts: Vec<String> = string.split(separator).map(String::from).collect();
    while parts.last().is_some_and(String::is_empty) {
        parts.pop();
    }
    parts
}

// ---------------------------------------------------------------------------------------------
// Browsers

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Base,
    Edge,
    InternetExplorer,
    Opera,
    WechatBrowser,
    Vivaldi,
    Chrome,
    ITunes,
    PlayStation,
    PodcastAddict,
    Webkit,
    Gecko,
    WindowsMediaPlayer,
    AppleCoreMedia,
    Libavformat,
}

impl Kind {
    /// `UserAgent::Browsers::ALL`, in detection order.
    const ALL: [Kind; 14] = [
        Kind::Edge,
        Kind::InternetExplorer,
        Kind::Opera,
        Kind::WechatBrowser,
        Kind::Vivaldi,
        Kind::Chrome,
        Kind::ITunes,
        Kind::PlayStation,
        Kind::PodcastAddict,
        Kind::Webkit,
        Kind::Gecko,
        Kind::WindowsMediaPlayer,
        Kind::AppleCoreMedia,
        Kind::Libavformat,
    ];

    /// Each browser class's `self.extend?(agent)`.
    fn extends(self, products: &[Product]) -> bool {
        let first = products.first();
        let first_version = first.map(|p| p.version.as_str());
        let any = |name: &str| products.iter().any(|p| p.product == name);

        match self {
            Kind::Base => true,
            Kind::Edge => products.last().is_some_and(|p| p.product == "Edge"),
            Kind::InternetExplorer => first.is_some_and(|p| {
                p.comment.is_some()
                    && (p.comment_at(1).is_some_and(|c| c.contains("MSIE")) || p.joined_comment().is_some_and(|c| trident_rv(&c)))
            }),
            Kind::Opera => first.is_some_and(|p| p.product == "Opera") || products.last().is_some_and(|p| p.product == "OPR"),
            Kind::WechatBrowser => products.iter().any(|p| contains_ignoring_case(&p.product, "micromessenger")),
            Kind::Vivaldi => any("Vivaldi"),
            Kind::Chrome => any("Chrome") || any("CriOS"),
            Kind::ITunes => any("iTunes"),
            Kind::PlayStation => first
                .and_then(|p| p.comment.as_ref())
                .and_then(|comment| comment.first())
                .is_some_and(|c| c.contains("PLAYSTATION 3") || c.contains("PlayStation Vita") || c.contains("PlayStation 4")),
            Kind::PodcastAddict => {
                products.len() >= 3 && products[0].product == "Podcast" && products[1].product == "Addict" && products[2].product == "-"
            }
            Kind::Webkit => products.iter().any(|p| {
                same_ignoring_case(&p.product, "applewebkit") || p.comment.iter().flatten().any(|c| webkit_comment_version(c).is_some())
            }),
            Kind::Gecko => first.is_some_and(|p| p.product == "Mozilla"),
            Kind::WindowsMediaPlayer => products.iter().any(|p| {
                ["NSPlayer", "Windows-Media-Player", "WMFSDK"].contains(&p.product.as_str())
                    && !matches!(first_version, Some("4.1.0.3856" | "7.10.0.3059" | "7.0.0.1956"))
            }),
            Kind::AppleCoreMedia => any("AppleCoreMedia"),
            Kind::Libavformat => {
                products.iter().any(|p| p.product == "Lavf" || (p.product == "NSPlayer" && first_version == Some("4.1.0.3856")))
            }
        }
    }
}

/// A parsed User-Agent: `UserAgent::Browsers::Base` extended with the detected browser class.
#[derive(Debug, Clone)]
pub struct Agent {
    kind: Kind,
    products: Vec<Product>,
}

impl Agent {
    /// `browser`; nil (possible for unparseable strings and a bare PlayStation Vita) is "".
    pub fn browser(&self) -> String {
        self.try_browser().ok().flatten().unwrap_or_default()
    }

    /// `version`; nil is an empty version.
    pub fn version(&self) -> Version {
        self.try_version().ok().flatten().unwrap_or_default()
    }

    pub fn platform(&self) -> Option<String> {
        self.try_platform().ok().flatten()
    }

    /// `bot?`.
    pub fn is_bot(&self) -> bool {
        let Some(application) = self.application() else {
            return true;
        };

        self.products.iter().flat_map(|p| p.comment.iter().flatten()).any(|c| contains_ignoring_case(c, "bot"))
            || self.detect_product("Chrome-Lighthouse").is_some()
            || application.product.contains("bot")
    }

    // --- Base helpers ---

    fn first(&self) -> Option<&Product> {
        self.products.first()
    }

    fn last(&self) -> Option<&Product> {
        self.products.last()
    }

    /// `detect_product`: case-insensitive product name lookup (also what `respond_to?` and
    /// `method_missing` use).
    fn detect_product(&self, name: &str) -> Option<&Product> {
        self.products.iter().find(|p| same_ignoring_case(&p.product, name))
    }

    /// `application`: most classes use the first product; the WebKit-based ones the first product
    /// with a non-empty comment.
    fn application(&self) -> Option<&Product> {
        match self.kind {
            Kind::Chrome | Kind::Vivaldi | Kind::Webkit | Kind::ITunes | Kind::AppleCoreMedia => {
                self.products.iter().find(|p| p.comment.as_ref().is_some_and(|c| !c.is_empty()))
            }
            _ => self.first(),
        }
    }

    fn application_comment(&self) -> Option<&Vec<String>> {
        self.application().and_then(|a| a.comment.as_ref())
    }

    fn base_version(&self) -> Option<Version> {
        self.application().map(|a| a.version.clone())
    }

    // --- browser ---

    pub(crate) fn try_browser(&self) -> Rb<Option<String>> {
        let name = match self.kind {
            Kind::Base => return Ok(self.application().map(|a| a.product.clone())),
            Kind::Edge => "Edge",
            Kind::InternetExplorer => "Internet Explorer",
            Kind::Opera => "Opera",
            Kind::WechatBrowser => "Wechat Browser",
            Kind::Vivaldi => "Vivaldi",
            Kind::Chrome => {
                if self.detect_product("Iron").is_some() {
                    "Iron"
                } else {
                    "Chrome"
                }
            }
            Kind::ITunes => "iTunes",
            Kind::PlayStation => return Ok(self.playstation_browser().map(String::from)),
            Kind::PodcastAddict => "Podcast Addict",
            Kind::Webkit => return Ok(Some(self.webkit_browser())),
            Kind::Gecko => return Ok(Some(self.gecko_browser())),
            Kind::WindowsMediaPlayer => "Windows Media Player",
            Kind::AppleCoreMedia => "AppleCoreMedia",
            Kind::Libavformat => "libavformat",
        };
        Ok(Some(name.to_string()))
    }

    fn playstation_browser(&self) -> Option<&'static str> {
        let first_comment = self.application_comment()?.first()?;
        if first_comment.contains("PLAYSTATION 3") {
            Some("PS3 Internet Browser")
        } else if self.last().is_some_and(|p| p.product == "Silk") {
            Some("Silk")
        } else if first_comment.contains("PlayStation 4") {
            Some("PS4 Internet Browser")
        } else {
            None
        }
    }

    fn webkit_browser(&self) -> String {
        if self.webkit_os().is_some_and(|os| os.contains("Android")) {
            "Android".into()
        } else if self.webkit_platform().as_deref() == Some("BlackBerry") {
            "BlackBerry".into()
        } else {
            "Safari".into()
        }
    }

    fn gecko_browser(&self) -> String {
        ["PaleMoon", "Firefox", "Camino", "Iceweasel", "Seamonkey"]
            .into_iter()
            .find(|name| self.detect_product(name).is_some())
            .map(String::from)
            .unwrap_or_else(|| self.first().map(|p| p.product.clone()).unwrap_or_default())
    }

    // --- version ---

    pub(crate) fn try_version(&self) -> Rb<Option<Version>> {
        Ok(match self.kind {
            Kind::Base | Kind::WindowsMediaPlayer | Kind::AppleCoreMedia => self.base_version(),
            Kind::Edge | Kind::Vivaldi => self.last().map(|p| p.version.clone()),
            Kind::InternetExplorer => {
                let joined = self.application().and_then(Product::joined_comment).unwrap_or_default();
                Some(Version::new(ie_version(&joined).unwrap_or("")))
            }
            Kind::Opera => self.opera_version(),
            Kind::WechatBrowser => Some(self.detect_product("MicroMessenger").ok_or(Raised)?.version.clone()),
            Kind::Chrome => {
                let product = self.detect_product("CriOs").or_else(|| self.detect_product("chrome"));
                Some(product.ok_or(Raised)?.version.clone())
            }
            Kind::ITunes => Some(self.detect_product("iTunes").ok_or(Raised)?.version.clone()),
            Kind::PlayStation => self.playstation_version(),
            Kind::PodcastAddict => None,
            Kind::Webkit => Some(self.webkit_version()),
            Kind::Gecko => {
                let version = self.detect_product(&self.gecko_browser()).ok_or(Raised)?.version.clone();
                if version.is_nil() { self.base_version() } else { Some(version) }
            }
            Kind::Libavformat => {
                if self.detect_product("NSPlayer").is_some() {
                    None
                } else {
                    self.base_version()
                }
            }
        })
    }

    fn opera_mini(&self) -> bool {
        // `/Opera Mini/ === application` matches against UserAgent#to_str; only the comment
        // can hold a space.
        self.first().and_then(Product::joined_comment).is_some_and(|c| c.contains("Opera Mini"))
    }

    fn opera_version(&self) -> Option<Version> {
        if self.opera_mini() {
            // `rescue Version.new` covers a comment without an "Opera Mini/<version>".
            let comment = self.application_comment().into_iter().flatten().find(|c| c.contains("Opera Mini"));
            let version = comment.and_then(|c| capture_after(c, "Opera Mini/", |c| c.is_ascii_digit() || c == '.'));
            Some(Version::new(version.unwrap_or("")))
        } else if let Some(product) = self.detect_product("Version") {
            Some(product.version.clone())
        } else if let Some(product) = self.detect_product("OPR") {
            Some(product.version.clone())
        } else {
            self.base_version()
        }
    }

    fn playstation_version(&self) -> Option<Version> {
        let os = self.playstation_os()?;
        let after = |marker: &str| Version::new(ruby_split(&os, marker).last().map(String::as_str).unwrap_or(""));

        if self.playstation_browser() == Some("Silk") {
            self.last().map(|p| p.version.clone())
        } else {
            match self.playstation_platform().as_deref() {
                Some("PlayStation 3") => Some(after("PLAYSTATION 3 ")),
                Some("PlayStation 4") => Some(after("PlayStation 4 ")),
                Some("PlayStation Vita") => Some(after("PlayStation Vita ")),
                _ => None,
            }
        }
    }

    fn webkit_version(&self) -> Version {
        if let Some(product) = self.detect_product("Version") {
            return product.version.clone();
        }
        if let Some(ios) = self.webkit_os().as_deref().and_then(|os| capture_after(os, "iOS ", |c| c.is_ascii_digit() || c == '.'))
            && self.webkit_browser() == "Safari"
        {
            return Version::new(&ios.replace('_', "."));
        }
        let build = self.webkit().map(|webkit| webkit.as_str().to_string()).unwrap_or_default();
        Version::new(webkit_build_version(&build).unwrap_or(""))
    }

    /// `Webkit#webkit.version`: the AppleWebKit product's version, or one from a comment.
    fn webkit(&self) -> Option<Version> {
        if let Some(product) = self.products.iter().find(|p| same_ignoring_case(&p.product, "applewebkit")) {
            return Some(product.version.clone());
        }
        self.products.iter().flat_map(|p| p.comment.iter().flatten()).find_map(|c| webkit_comment_version(c)).map(Version::new)
    }

    // --- platform ---

    pub(crate) fn try_platform(&self) -> Rb<Option<String>> {
        let comment = self.application_comment();
        let first = comment.and_then(|c| c.first()).map(String::as_str);
        let any = |needle: &str| comment.is_some_and(|c| c.iter().any(|c| c.contains(needle)));

        Ok(match self.kind {
            Kind::Base | Kind::Libavformat => None,
            Kind::Edge | Kind::InternetExplorer | Kind::WindowsMediaPlayer => Some("Windows".into()),
            Kind::Opera | Kind::AppleCoreMedia => {
                if comment.is_none() {
                    return Ok(None);
                }
                if first.is_some_and(|c| c.contains("Windows")) { Some("Windows".into()) } else { first.map(String::from) }
            }
            Kind::WechatBrowser => {
                if comment.is_none() {
                    return Ok(None);
                }
                if first.is_some_and(|c| c.contains("iPhone")) {
                    Some("iPhone".into())
                } else if any("Android") {
                    Some("Android".into())
                } else {
                    first.map(String::from)
                }
            }
            Kind::Chrome | Kind::Vivaldi => {
                if comment.is_none() {
                    return Ok(None);
                }
                if first.is_some_and(|c| c.contains("Windows")) {
                    Some("Windows".into())
                } else if any("CrOS") {
                    Some("ChromeOS".into())
                } else if any("Android") {
                    Some("Android".into())
                } else {
                    first.map(String::from)
                }
            }
            Kind::Webkit | Kind::ITunes => self.webkit_platform(),
            Kind::PlayStation => self.playstation_platform(),
            Kind::PodcastAddict => {
                let os = self.podcast_addict_os()?.ok_or(Raised)?;
                os.contains("Android").then(|| "Android".into())
            }
            Kind::Gecko => {
                if comment.is_none() {
                    return Ok(None);
                }
                match first {
                    Some("compatible" | "Mobile") => None,
                    Some(c) if c.starts_with("Windows ") => Some("Windows".into()),
                    other => other.map(String::from),
                }
            }
        })
    }

    fn webkit_platform(&self) -> Option<String> {
        let comment = self.application_comment()?;
        let first = comment.first().map(String::as_str);
        if first.is_some_and(|c| c.contains("Windows")) {
            Some("Windows".into())
        } else if first == Some("BB10") {
            Some("BlackBerry".into())
        } else if comment.iter().any(|c| c.contains("Android")) {
            Some("Android".into())
        } else {
            first.map(String::from)
        }
    }

    fn playstation_platform(&self) -> Option<String> {
        let os = self.playstation_os()?;
        if os.contains("PLAYSTATION 3") {
            Some("PlayStation 3".into())
        } else if os.contains("PlayStation 4") {
            Some("PlayStation 4".into())
        } else if os.contains("PlayStation Vita") {
            Some("PlayStation Vita".into())
        } else {
            None
        }
    }

    // --- os ---

    pub(crate) fn try_os(&self) -> Rb<Option<String>> {
        Ok(match self.kind {
            Kind::Base | Kind::Libavformat => None,
            Kind::Edge => {
                let matched = self.products.iter().flat_map(|p| p.comment.iter().flatten()).find_map(|c| windows_os(c));
                Some(normalize_os(matched.unwrap_or("")))
            }
            Kind::InternetExplorer => {
                let joined = self.application().and_then(Product::joined_comment).unwrap_or_default();
                Some(normalize_os(windows_os(&joined).unwrap_or("")))
            }
            Kind::Opera => {
                let Some(comment) = self.application_comment() else {
                    return Ok(None);
                };
                match comment.first() {
                    Some(first) if first.contains("Windows") => Some(normalize_os(first)),
                    _ => comment.get(1).cloned(),
                }
            }
            Kind::WechatBrowser | Kind::Chrome | Kind::Vivaldi | Kind::AppleCoreMedia => {
                self.application_comment().and_then(|comment| chrome_os(comment))
            }
            Kind::Webkit => self.webkit_os(),
            Kind::ITunes => self.itunes_os(),
            Kind::PlayStation => self.playstation_os(),
            Kind::PodcastAddict => self.podcast_addict_os()?,
            Kind::Gecko => self.gecko_os(),
            Kind::WindowsMediaPlayer => Some(self.windows_media_player_os()?.to_string()),
        })
    }

    fn webkit_os(&self) -> Option<String> {
        let comment = self.application_comment()?;
        let at = |i: usize| comment.get(i).map(String::as_str);

        if at(0).is_some_and(|c| c.contains("Windows NT")) {
            at(0).map(normalize_os)
        } else if at(2).is_none() || at(1).is_some_and(|c| c.contains("Android")) {
            at(1).map(normalize_os)
        } else if let Some(ios) = comment.iter().find(|c| ios_version(c).is_some()) {
            Some(normalize_os(ios))
        } else {
            at(2).map(normalize_os)
        }
    }

    fn itunes_os(&self) -> Option<String> {
        let windows = self.application_comment().and_then(|c| c.first()).is_some_and(|c| c.contains("Windows"));
        if !windows {
            return self.webkit_os();
        }

        let full_os = self.itunes_full_os().unwrap_or_default();
        let name = if full_os.contains("Windows 8.1") {
            "Windows 8.1"
        } else if full_os.contains("Windows 8") {
            "Windows 8"
        } else if full_os.contains("Windows 7") {
            "Windows 7"
        } else if full_os.contains("Windows Vista") {
            "Windows Vista"
        } else if full_os.contains("Windows XP") {
            "Windows XP"
        } else {
            "Windows"
        };
        Some(name.into())
    }

    /// `ITunes#full_os`: the comment was cut at the first ")", so "(Build 7601" gets it back.
    fn itunes_full_os(&self) -> Option<String> {
        let full_os = self.application_comment().filter(|c| c.len() > 1)?[1].clone();
        let chars: Vec<char> = full_os.chars().collect();
        let n = chars.len();
        let reopened =
            n >= 11 && chars[n - 11..n - 4].iter().copied().eq("(Build ".chars()) && chars[n - 4..].iter().all(char::is_ascii_digit);
        Some(if reopened { format!("{full_os})") } else { full_os })
    }

    fn playstation_os(&self) -> Option<String> {
        self.application_comment().map(|c| c.join(" "))
    }

    /// `PodcastAddict#os`; the outer `Err` is the gem raising on a comment-less Dalvik/Mozilla.
    fn podcast_addict_os(&self) -> Rb<Option<String>> {
        let Some(device) = self.products.get(3) else {
            return Ok(None);
        };
        if device.product != "Dalvik" && device.product != "Mozilla" {
            return Ok(None);
        }
        let comment = device.comment.as_ref().ok_or(Raised)?;
        Ok(match comment.len() {
            n if n > 3 => Some(comment[2].clone()),
            3 => Some("Android".into()),
            _ => None,
        })
    }

    fn gecko_os(&self) -> Option<String> {
        let comment = self.application_comment()?;
        let first = comment.first().map(String::as_str);
        let index = if comment.get(1).map(String::as_str) == Some("U") {
            2
        } else if first.is_some_and(|c| c.starts_with("Windows ") || c.starts_with("Android")) {
            0
        } else if first == Some("Mobile") {
            return None;
        } else {
            1
        };
        comment.get(index).map(|c| normalize_os(c))
    }

    fn windows_media_player_major(&self) -> Rb<u64> {
        // `version.to_a[0]` compared with an Integer: nil raises NoMethodError, a String
        // ArgumentError.
        let version = self.base_version().ok_or(Raised)?;
        match version.to_a().first() {
            Some(Segment::Int(digits)) => Ok(digits.parse().unwrap_or(u64::MAX)),
            _ => Err(Raised),
        }
    }

    fn windows_media_player_os(&self) -> Rb<&'static str> {
        let major = self.windows_media_player_major()?;
        let segments = self.base_version().unwrap_or_default().to_a();
        let part = |i: usize| segments.get(i).and_then(Segment::as_u64);

        Ok(if major <= 4 {
            match part(3) {
                Some(3564 | 3925) => "Windows 98",
                Some(3857) => "Windows 9x",
                Some(3936) => "Windows XP",
                Some(3938) => "Windows 2000",
                _ => "Windows",
            }
        } else if major == 7 {
            match part(3) {
                Some(3055) => "Windows 98",
                _ => "Windows",
            }
        } else if major == 8 {
            "Windows XP"
        } else if major == 9 || major == 10 {
            match part(3) {
                Some(2980) => "Windows 98/2000",
                Some(3268 | 3367 | 3270) => "Windows 2000",
                Some(3802 | 4503) => "Windows XP",
                _ => "Windows",
            }
        } else if major == 11 || major == 12 {
            match part(2) {
                Some(9841 | 9858 | 9860 | 9879) => "Windows 10",
                Some(9651) => "Windows Phone 8.1",
                Some(9600) => "Windows 8.1",
                Some(9200) => "Windows 8",
                Some(7600 | 7601) => "Windows 7",
                Some(6000..=6002) => "Windows Vista",
                Some(5721) => "Windows XP",
                _ => "Windows",
            }
        } else {
            "Windows"
        })
    }

    // --- mobile? (unused by the app; kept for the gem's vectors) ---

    #[cfg(test)]
    pub(crate) fn try_mobile(&self) -> Rb<bool> {
        Ok(match self.kind {
            Kind::Opera => self.opera_mini(),
            Kind::PlayStation => self.playstation_platform().as_deref() == Some("PlayStation Vita"),
            Kind::PodcastAddict => true,
            Kind::WindowsMediaPlayer => matches!(self.windows_media_player_os()?, "Windows Phone 8" | "Windows Phone 8.1"),
            _ => {
                self.detect_product("Mobile").is_some()
                    || self.products.iter().any(|p| p.comment.iter().flatten().any(|c| c == "Mobile"))
                    || self.try_os()?.is_some_and(|os| os.contains("Android"))
                    || self.application_comment().is_some_and(|c| c.iter().any(|c| c.starts_with("IEMobile")))
            }
        })
    }
}

/// `os` shared by Chrome, Vivaldi, WechatBrowser and AppleCoreMedia.
fn chrome_os(comment: &[String]) -> Option<String> {
    let at = |i: usize| comment.get(i).map(String::as_str);
    let pick = if at(0).is_some_and(|c| c.contains("Windows NT")) {
        at(0)
    } else if at(2).is_none() || at(1).is_some_and(|c| c.contains("Android")) {
        at(1)
    } else {
        at(2)
    };
    pick.map(normalize_os)
}

// ---------------------------------------------------------------------------------------------
// OperatingSystems and hand-written regexps

/// `UserAgent::OperatingSystems.normalize_os`.
fn normalize_os(os: &str) -> String {
    let windows = match os {
        "Windows NT 10.0" => Some("Windows 10"),
        "Windows NT 6.3" => Some("Windows 8.1"),
        "Windows NT 6.2" => Some("Windows 8"),
        "Windows NT 6.1" => Some("Windows 7"),
        "Windows NT 6.0" => Some("Windows Vista"),
        "Windows NT 5.2" => Some("Windows XP x64 Edition"),
        "Windows NT 5.1" => Some("Windows XP"),
        "Windows NT 5.01" => Some("Windows 2000, Service Pack 1 (SP1)"),
        "Windows NT 5.0" => Some("Windows 2000"),
        "Windows NT 4.0" => Some("Windows NT 4.0"),
        "Windows 98" => Some("Windows 98"),
        "Windows 95" => Some("Windows 95"),
        "Windows CE" => Some("Windows CE"),
        _ => None,
    };
    if let Some(windows) = windows {
        return windows.into();
    }
    if let Some(version) = mac_os_x_version(os) {
        return match version {
            Some(version) => format!("OS X {}", version.replace('_', ".")),
            None => "OS X".into(),
        };
    }
    if let Some(version) = ios_version(os) {
        return format!("iOS {}", version.replace('_', "."));
    }
    if let Some(version) = chrome_os_version(os) {
        return format!("ChromeOS {version}");
    }
    os.to_string()
}

fn is_version_char(b: u8) -> bool {
    b.is_ascii_digit() || b == b'.'
}

/// Length of the run of bytes at the start of `s` satisfying `f`.
fn run(s: &[u8], f: impl Fn(u8) -> bool) -> usize {
    s.iter().take_while(|&&b| f(b)).count()
}

/// `/(?:Intel|PPC) Mac OS X\s*([0-9_\.]+)?/`: `Some(capture)` when it matches.
fn mac_os_x_version(os: &str) -> Option<Option<&str>> {
    os.char_indices().map(|(i, _)| i).find_map(|i| {
        let rest = &os[i..];
        let after = rest.strip_prefix("Intel Mac OS X").or_else(|| rest.strip_prefix("PPC Mac OS X"))?;
        let after = &after[run(after.as_bytes(), |b| is_ruby_space(b as char))..];
        let digits = run(after.as_bytes(), |b| b.is_ascii_digit() || b == b'_' || b == b'.');
        Some((digits > 0).then(|| &after[..digits]))
    })
}

/// `IOS_VERSION_REGEX = /CPU (?:iPhone |iPod )?OS ([\d_]+) like Mac OS X/`.
fn ios_version(os: &str) -> Option<&str> {
    os.char_indices().map(|(i, _)| i).find_map(|i| {
        let rest = os[i..].strip_prefix("CPU ")?;
        [rest.strip_prefix("iPhone "), rest.strip_prefix("iPod "), Some(rest)].into_iter().flatten().find_map(|rest| {
            let rest = rest.strip_prefix("OS ")?;
            let digits = run(rest.as_bytes(), |b| b.is_ascii_digit() || b == b'_');
            (digits > 0 && rest[digits..].starts_with(" like Mac OS X")).then(|| &rest[..digits])
        })
    })
}

/// `/CrOS\s([^\s]+)\s(\d+(\.\d+)*)/`: the second capture.
fn chrome_os_version(os: &str) -> Option<&str> {
    os.char_indices().map(|(i, _)| i).find_map(|i| {
        let rest = os[i..].strip_prefix("CrOS")?.as_bytes();
        let is_space = |b: u8| is_ruby_space(b as char);
        if rest.first().is_none_or(|&b| !is_space(b)) {
            return None;
        }
        let word = run(&rest[1..], |b| !is_space(b));
        if word == 0 {
            return None;
        }
        let at = 1 + word;
        if rest.get(at).is_none_or(|&b| !is_space(b)) {
            return None;
        }
        let start = at + 1;
        let mut end = start + run(&rest[start..], |b| b.is_ascii_digit());
        if end == start {
            return None;
        }
        while rest.get(end) == Some(&b'.') && rest.get(end + 1).is_some_and(u8::is_ascii_digit) {
            end += 1 + run(&rest[end + 1..], |b| b.is_ascii_digit());
        }
        let offset = os.len() - rest.len();
        Some(&os[offset + start..offset + end])
    })
}

/// `/Windows NT [\d\.]+|Windows Phone (OS )?[\d\.]+/`: the matched text.
fn windows_os(s: &str) -> Option<&str> {
    s.char_indices().map(|(i, _)| i).find_map(|i| {
        let rest = &s[i..];
        let tail = if let Some(after) = rest.strip_prefix("Windows NT ") {
            Some(after)
        } else if let Some(after) = rest.strip_prefix("Windows Phone ") {
            after.strip_prefix("OS ").filter(|a| run(a.as_bytes(), is_version_char) > 0).or(Some(after))
        } else {
            None
        }?;
        let digits = run(tail.as_bytes(), is_version_char);
        (digits > 0).then(|| &rest[..rest.len() - tail.len() + digits])
    })
}

/// `joined_comment =~ /Trident.+rv:/`.
fn trident_rv(s: &str) -> bool {
    s.match_indices("Trident").any(|(i, _)| {
        let rest = &s[i + "Trident".len()..];
        let line = rest.split('\n').next().unwrap_or("");
        line.match_indices("rv:").any(|(j, _)| j >= 1)
    })
}

/// `joined_comment[/(MSIE\s|rv:)([\d\.]+)/, 2]`.
fn ie_version(s: &str) -> Option<&str> {
    s.char_indices().map(|(i, _)| i).find_map(|i| {
        let rest = &s[i..];
        let tail = rest
            .strip_prefix("MSIE")
            .filter(|after| after.chars().next().is_some_and(is_ruby_space))
            .map(|after| &after[1..])
            .or_else(|| rest.strip_prefix("rv:"))?;
        let digits = run(tail.as_bytes(), is_version_char);
        (digits > 0).then(|| &tail[..digits])
    })
}

/// The capture of `/<prefix>([class]+)/` at its leftmost match.
fn capture_after<'a>(s: &'a str, prefix: &str, class: impl Fn(char) -> bool) -> Option<&'a str> {
    s.match_indices(prefix).find_map(|(i, _)| {
        let tail = &s[i + prefix.len()..];
        let len: usize = tail.chars().take_while(|&c| class(c)).map(char::len_utf8).sum();
        (len > 0).then(|| &tail[..len])
    })
}

/// `WEBKIT_VERSION_REGEXP = /\A(?<webkit>AppleWebKit)\/(?<version>[\d\.]+)/i`: the version.
fn webkit_comment_version(comment: &str) -> Option<&str> {
    let name = &comment[..comment.char_indices().nth(11).map_or(comment.len(), |(i, _)| i)];
    if name.chars().count() != 11 || !same_ignoring_case(name, "applewebkit") {
        return None;
    }
    let tail = comment[name.len()..].strip_prefix('/')?;
    let digits = run(tail.as_bytes(), is_version_char);
    (digits > 0).then(|| &tail[..digits])
}

/// `Webkit::BuildVersions`: Safari versions before Safari 3 reported only the WebKit build.
fn webkit_build_version(build: &str) -> Option<&'static str> {
    Some(match build {
        "85.7" => "1.0",
        "85.8.5" | "85.8.2" => "1.0.3",
        "124" => "1.2",
        "125.2" => "1.2.2",
        "125.4" => "1.2.3",
        "125.5.5" | "125.5.6" | "125.5.7" => "1.2.4",
        "312.1.1" | "312.1" => "1.3",
        "312.5" | "312.5.1" | "312.5.2" => "1.3.1",
        "312.8" | "312.8.1" => "1.3.2",
        "412" | "412.6" | "412.6.2" => "2.0",
        "412.7" => "2.0.1",
        "416.11" | "416.12" => "2.0.2",
        "417.9" | "418" => "2.0.3",
        "418.8" | "418.9" | "418.9.1" | "419" => "2.0.4",
        "425.13" => "2.2",
        "534.52.7" => "5.1.2",
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::collections::BTreeSet;

    pub(crate) fn vectors() -> Value {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../vectors/campfire_user_agents.json");
        serde_json::from_str(&std::fs::read_to_string(path).expect("read vectors")).expect("parse vectors")
    }

    /// Compares a Ruby value (`{"error": ...}` when it raised) with ours.
    pub(crate) fn check(failures: &mut Vec<String>, context: &str, expected: &Value, actual: Rb<Value>) {
        let ok = match (expected.get("error"), &actual) {
            (Some(_), Err(Raised)) => true,
            (None, Ok(value)) => value == expected,
            _ => false,
        };
        if !ok {
            failures.push(format!("{context}: expected {expected}, got {actual:?}"));
        }
    }

    #[test]
    fn matches_the_gem() {
        let vectors = vectors();
        let mut failures = Vec::new();

        for case in vectors["user_agents"].as_array().unwrap() {
            let agent = parse(case["ua"].as_str().unwrap_or(""));
            let ua = &case["ua"];
            check(&mut failures, &format!("{ua} browser"), &case["browser"], agent.try_browser().map(|b| json!(b)));
            check(&mut failures, &format!("{ua} version"), &case["version"], agent.try_version().map(|v| json!(v.map(|v| v.to_string()))));
            check(&mut failures, &format!("{ua} platform"), &case["platform"], agent.try_platform().map(|p| json!(p)));
            check(&mut failures, &format!("{ua} os"), &case["os"], agent.try_os().map(|o| json!(o)));
            check(&mut failures, &format!("{ua} bot"), &case["bot"], Ok(json!(agent.is_bot())));
            check(&mut failures, &format!("{ua} mobile"), &case["mobile"], agent.try_mobile().map(|m| json!(m)));
        }

        assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
    }

    #[test]
    fn versions_match_the_gem() {
        let vectors = vectors();
        let mut failures = Vec::new();

        for case in vectors["versions"].as_array().unwrap() {
            let version = Version::new(case["string"].as_str().unwrap());
            let to_a: Vec<String> = version
                .to_a()
                .iter()
                .map(|s| match s {
                    Segment::Int(i) => format!("i:{i}"),
                    Segment::Str(s) => format!("s:{s}"),
                })
                .collect();
            if json!(version.is_nil()) != case["nil"] || json!(to_a) != case["to_a"] {
                failures.push(format!("{case}: nil={} to_a={to_a:?}", version.is_nil()));
            }
        }

        for case in vectors["comparisons"].as_array().unwrap() {
            let a = Version::new(case["a"].as_str().unwrap());
            let b = Version::new(case["b"].as_str().unwrap());
            let cmp = match a.ruby_cmp(&b) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            };
            if json!(cmp) != case["cmp"] || json!(a < b) != case["lt"] || json!(a == b) != case["eq"] {
                failures.push(format!("{case}: cmp={cmp} lt={} eq={}", a < b, a == b));
            }
        }

        assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
    }

    #[test]
    fn blank_user_agents_parse_as_the_default() {
        let agent = parse("  ");
        assert_eq!(agent.browser(), "Mozilla");
        assert_eq!(agent.version().to_string(), "4.0");
    }

    /// Every User-Agent in the vectors: the corpus in reference-tools/campfire/user_agents.rb.
    fn user_agents() -> Vec<Option<String>> {
        vectors()["user_agents"].as_array().unwrap().iter().map(|case| case["ua"].as_str().map(String::from)).collect()
    }

    /// The straightforward code the fast paths replaced, kept to check them against.
    mod straightforward {
        use super::super::{Product, Version, is_ruby_space, ruby_split};

        /// `parse`'s product loop over chars, collecting the stripped rest after each product.
        pub fn products(user_agent: &str) -> Vec<Product> {
            let default = "Mozilla/4.0 (compatible)";
            let mut rest: Vec<char> =
                if ruby_compat::strip(user_agent).is_empty() { default.chars().collect() } else { user_agent.chars().collect() };
            let mut products = Vec::new();
            while let Some((length, product)) = match_product(&rest) {
                products.push(product);
                let tail: String = rest[length..].iter().collect();
                rest = ruby_compat::strip(&tail).chars().collect();
            }
            products
        }

        fn match_product(s: &[char]) -> Option<(usize, Product)> {
            let is_product_char = |c: char| c != '/' && !is_ruby_space(c);
            let quotes = s.iter().take_while(|&&c| c == '\'' || c == '"').count();

            let start = if quotes < s.len() && is_product_char(s[quotes]) {
                quotes
            } else if quotes > 0 {
                quotes - 1
            } else {
                return None;
            };

            let mut i = start + 1;
            while i < s.len() && is_product_char(s[i]) {
                i += 1;
            }
            let product: String = s[start..i].iter().collect();

            if i < s.len() && s[i] == '/' {
                i += 1;
            }
            let version_start = i;
            while i < s.len() && !is_ruby_space(s[i]) && s[i] != ',' {
                i += 1;
            }
            let version: String = s[version_start..i].iter().collect();

            let mut comment = None;
            if i + 1 < s.len() && is_ruby_space(s[i]) && s[i + 1] == '(' {
                if let Some(close) = s[i + 2..].iter().position(|&c| c == ')') {
                    comment = Some(s[i + 2..i + 2 + close].iter().collect::<String>());
                    i += 2 + close + 1;
                }
            } else if s[i..].iter().copied().take(10).eq(",gzip(gfe)".chars()) {
                i += 10;
            }

            let product = Product { product, version: Version::new(&version), comment: comment.map(|comment| ruby_split(&comment, "; ")) };
            Some((i, product))
        }

        pub fn same_ignoring_case(a: &str, b: &str) -> bool {
            a.to_lowercase() == b.to_lowercase()
        }

        pub fn contains_ignoring_case(haystack: &str, needle: &str) -> bool {
            haystack.to_lowercase().contains(needle)
        }

        pub fn webkit_comment_version(comment: &str) -> Option<&str> {
            let name: String = comment.chars().take(11).collect();
            if name.chars().count() != 11 || name.to_lowercase() != "applewebkit" {
                return None;
            }
            let tail = comment[name.len()..].strip_prefix('/')?;
            let digits = tail.bytes().take_while(|&b| b.is_ascii_digit() || b == b'.').count();
            (digits > 0).then(|| &tail[..digits])
        }
    }

    fn fields(product: &Product) -> (&str, &str, Option<&[String]>) {
        (&product.product, product.version.as_str(), product.comment.as_deref())
    }

    /// Each User-Agent, and each of its prefixes (the truncated and unterminated shapes).
    #[test]
    fn scans_products_like_the_char_by_char_matcher() {
        for user_agent in user_agents() {
            let user_agent = user_agent.unwrap_or_default();
            let prefixes = user_agent.char_indices().map(|(i, _)| &user_agent[..i]).chain([user_agent.as_str()]);
            for prefix in prefixes {
                let (fast, slow) = (parse(prefix).products, straightforward::products(prefix));
                assert_eq!(fast.iter().map(fields).collect::<Vec<_>>(), slow.iter().map(fields).collect::<Vec<_>>(), "{prefix:?}");
            }
        }
    }

    /// Characters whose lowercase is ASCII though they aren't, or is longer than they are.
    const SPECIAL_CASES: [&str; 12] = ["\u{212A}", "k", "K", "\u{130}", "i\u{307}", "I", "ß", "SS", "Σ", "σ", "ς", "ÀPPLEWEBKIT"];

    /// Every product name the test agents parse to, and the special cases.
    fn product_names() -> BTreeSet<String> {
        let products = user_agents().into_iter().flat_map(|user_agent| parse(&user_agent.unwrap_or_default()).products);
        products.map(|product| product.product).chain(SPECIAL_CASES.map(String::from)).collect()
    }

    /// Every comment and whole test agent, and the product names.
    fn texts() -> BTreeSet<String> {
        let mut texts = product_names();
        for user_agent in user_agents().into_iter().flatten() {
            texts.extend(parse(&user_agent).products.into_iter().flat_map(|product| product.comment.into_iter().flatten()));
            texts.insert(user_agent);
        }
        texts
    }

    #[test]
    fn compares_ignoring_case_like_lowercasing_both() {
        // What `detect_product` looks for; Gecko's `version` also looks for the first product.
        let looked_for = ["Chrome-Lighthouse", "Iron", "PaleMoon", "Firefox", "Camino", "Iceweasel", "Seamonkey", "MicroMessenger"];
        let looked_for = looked_for.into_iter().chain(["CriOs", "chrome", "iTunes", "Version", "OPR", "NSPlayer", "Mobile", "applewebkit"]);
        let names = product_names();
        let looked_for: Vec<&str> = looked_for.chain(names.iter().map(String::as_str)).collect();

        for name in &names {
            for other in &looked_for {
                assert_eq!(same_ignoring_case(name, other), straightforward::same_ignoring_case(name, other), "{name:?} {other:?}");
            }
        }
    }

    #[test]
    fn searches_ignoring_case_like_lowercasing_the_haystack() {
        for haystack in texts() {
            for needle in ["bot", "micromessenger", "facebookexternalhit", "twitterbot", "k"] {
                let expected = straightforward::contains_ignoring_case(&haystack, needle);
                assert_eq!(contains_ignoring_case(&haystack, needle), expected, "{haystack:?} {needle:?}");
            }
        }
    }

    /// Each comment, and each of its suffixes.
    #[test]
    fn reads_webkit_comment_versions_like_lowercasing_the_name() {
        for text in texts() {
            for comment in text.char_indices().map(|(i, _)| &text[i..]) {
                assert_eq!(webkit_comment_version(comment), straightforward::webkit_comment_version(comment), "{comment:?}");
            }
        }
    }
}
