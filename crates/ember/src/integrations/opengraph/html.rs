//! The slice of `Nokogiri::HTML(html)` (libxml2's legacy HTML parser, in recovery mode) that
//! `Opengraph::Document` reads: every `<meta>` element's attributes.
//!
//! The body string comes from `Opengraph::Fetch`, tagged UTF-8, so libxml2 decodes it as UTF-8
//! whatever the page declares, taking a byte that isn't valid UTF-8 as Latin-1. Then it tokenizes
//! the way its pre-HTML5 parser does: `<script>`/`<style>` hold raw text, comments and
//! `<!…>`/`<?…>` markup are skipped, tag and attribute names are lowercased, the first of a
//! repeated attribute wins, and attribute values decode HTML 4 entities only when terminated by
//! `;` and numeric references with or without one (an invalid one cuts the value short, as the
//! NUL it produces ends libxml2's C string).
//!
//! The scanner steps through bytes: everything it matches is ASCII, so it only ever splits the
//! text between characters.

use super::entities::ENTITIES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub attributes: Vec<(String, String)>,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    pub fn has_attr(&self, name: &str) -> bool {
        self.attributes.iter().any(|(n, _)| n == name)
    }
}

/// libxml2 reading a UTF-8 buffer: valid sequences decode, any other byte is taken as Latin-1.
pub fn decode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                out.push_str(valid);
                break;
            }
            Err(error) => {
                let (valid, after) = rest.split_at(error.valid_up_to());
                out.push_str(std::str::from_utf8(valid).expect("validated"));
                out.push(after[0] as char);
                rest = &after[1..];
            }
        }
    }
    out
}

/// How many attributes of one tag are kept; the rest are parsed and dropped. Real tags have a
/// handful, and a page with thousands in one tag gains nothing from them.
const MAX_ATTRIBUTES: usize = 256;

/// The `<meta>` elements of the document, in document order.
pub fn meta_elements(html: &str) -> Vec<Element> {
    // A NUL ends libxml2's input
    let html = &html[..html.find('\0').unwrap_or(html.len())];
    let mut scanner = Scanner { bytes: html.as_bytes(), pos: 0 };
    let mut metas = Vec::new();
    while let Some(c) = scanner.peek(0) {
        if c != b'<' {
            scanner.pos += 1;
            continue;
        }
        match scanner.peek(1) {
            Some(b'/') => scanner.end_tag(),
            Some(b'!') => scanner.markup_declaration(),
            Some(b'?') => scanner.skip_past(b'>'),
            Some(c) if c.is_ascii_alphabetic() => {
                let (name, element, self_closing) = scanner.start_tag();
                if name == "meta" {
                    metas.push(element);
                } else if (name == "script" || name == "style") && !self_closing {
                    scanner.raw_text(&name);
                }
            }
            _ => scanner.pos += 1,
        }
    }
    metas
}

struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
}

fn is_blank(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

impl Scanner<'_> {
    fn peek(&self, ahead: usize) -> Option<u8> {
        self.bytes.get(self.pos + ahead).copied()
    }

    fn starts_with_ignore_case(&self, text: &str) -> bool {
        self.bytes.get(self.pos..self.pos + text.len()).is_some_and(|b| b.eq_ignore_ascii_case(text.as_bytes()))
    }

    /// The text from `start` to here, which begins and ends at ASCII bytes.
    fn text_from(&self, start: usize) -> &str {
        std::str::from_utf8(&self.bytes[start..self.pos]).expect("split at ASCII")
    }

    fn skip_blanks(&mut self) {
        while self.peek(0).is_some_and(is_blank) {
            self.pos += 1;
        }
    }

    /// Moves past the next `c` (or to the end).
    fn skip_past(&mut self, c: u8) {
        while let Some(next) = self.peek(0) {
            self.pos += 1;
            if next == c {
                break;
            }
        }
    }

    /// `htmlParseHTMLName`: `[A-Za-z_:.][A-Za-z0-9:_.-]*`, lowercased.
    fn html_name(&mut self) -> Option<String> {
        let first = self.peek(0)?;
        if !(first.is_ascii_alphabetic() || matches!(first, b'_' | b':' | b'.')) {
            return None;
        }
        let start = self.pos;
        while self.peek(0).is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b':' | b'-' | b'_' | b'.')) {
            self.pos += 1;
        }
        Some(self.text_from(start).to_ascii_lowercase())
    }

    fn end_tag(&mut self) {
        self.pos += 2;
        if self.html_name().is_some() {
            self.skip_past(b'>');
        }
    }

    /// `<!--…-->` (with `<!-->` and `<!--->` closing at once, and `--!>` accepted), `<!DOCTYPE…>`,
    /// and any other `<!…>` skipped as a bogus comment.
    fn markup_declaration(&mut self) {
        if self.peek(2) == Some(b'-') && self.peek(3) == Some(b'-') {
            self.pos += 4;
            if self.peek(0) == Some(b'>') {
                self.pos += 1;
                return;
            }
            if self.peek(0) == Some(b'-') && self.peek(1) == Some(b'>') {
                self.pos += 2;
                return;
            }
            while self.pos < self.bytes.len() {
                if self.starts_with_ignore_case("-->") {
                    self.pos += 3;
                    return;
                }
                if self.starts_with_ignore_case("--!>") {
                    self.pos += 4;
                    return;
                }
                self.pos += 1;
            }
        } else {
            self.skip_past(b'>');
        }
    }

    /// `htmlParseStartTag`: returns the name, the element, and whether it ended with `/>`.
    fn start_tag(&mut self) -> (String, Element, bool) {
        self.pos += 1;
        let name = self.html_name().unwrap_or_default();
        let mut attributes: Vec<(String, String)> = Vec::new();
        self.skip_blanks();
        loop {
            match self.peek(0) {
                None => break,
                Some(b'>') => break,
                Some(b'/') if self.peek(1) == Some(b'>') => break,
                _ => {}
            }
            match self.html_name() {
                Some(attribute) => {
                    self.skip_blanks();
                    let value = if self.peek(0) == Some(b'=') {
                        self.pos += 1;
                        self.skip_blanks();
                        self.attribute_value()
                    } else {
                        String::new()
                    };
                    if attributes.len() < MAX_ATTRIBUTES && !attributes.iter().any(|(n, _)| *n == attribute) {
                        attributes.push((attribute, value));
                    }
                }
                None => {
                    // Dump the bogus attribute string up to the next blank or the end of the tag
                    while let Some(c) = self.peek(0) {
                        if is_blank(c) || c == b'>' || (c == b'/' && self.peek(1) == Some(b'>')) {
                            break;
                        }
                        self.pos += 1;
                    }
                }
            }
            self.skip_blanks();
        }
        let self_closing = self.peek(0) == Some(b'/');
        if self_closing {
            self.pos += 2;
        } else if self.peek(0) == Some(b'>') {
            self.pos += 1;
        }
        (name, Element { attributes }, self_closing)
    }

    /// `htmlParseAttValue`
    fn attribute_value(&mut self) -> String {
        match self.peek(0) {
            Some(quote @ (b'"' | b'\'')) => {
                self.pos += 1;
                let value = self.attribute_text(Some(quote));
                if self.peek(0) == Some(quote) {
                    self.pos += 1;
                }
                value
            }
            _ => self.attribute_text(None),
        }
    }

    /// `htmlParseHTMLAttribute`: up to the quote, or (unquoted) a blank or `>`.
    fn attribute_text(&mut self, stop: Option<u8>) -> String {
        let ends_text = |c: u8| c == b'&' || Some(c) == stop || (stop.is_none() && (c == b'>' || is_blank(c)));
        let mut out = String::new();
        let mut truncated = false;
        loop {
            let start = self.pos;
            while self.peek(0).is_some_and(|c| !ends_text(c)) {
                self.pos += 1;
            }
            if !truncated {
                out.push_str(self.text_from(start));
            }
            if self.peek(0) != Some(b'&') {
                break;
            }
            let decoded = if self.peek(1) == Some(b'#') { self.char_ref().map(|c| c.to_string()) } else { Some(self.entity_ref()) };
            match decoded {
                Some(text) if !truncated => out.push_str(&text),
                Some(_) => {}
                None => truncated = true,
            }
        }
        out
    }

    /// `htmlParseCharRef`: `None` for a value that isn't a valid XML character.
    fn char_ref(&mut self) -> Option<char> {
        let hex = matches!(self.peek(2), Some(b'x' | b'X'));
        self.pos += if hex { 3 } else { 2 };
        let radix = if hex { 16 } else { 10 };
        let mut value: u32 = 0;
        while let Some(c) = self.peek(0) {
            if c == b';' {
                self.pos += 1;
                break;
            }
            let Some(digit) = char::from(c).to_digit(radix) else { break };
            if value < 0x110000 {
                value = value * radix + digit;
            }
            self.pos += 1;
        }
        let is_char = matches!(value, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF);
        if is_char { char::from_u32(value) } else { None }
    }

    /// `htmlParseEntityRef`: a known name followed by `;` decodes; anything else stays as written.
    fn entity_ref(&mut self) -> String {
        self.pos += 1;
        let start = self.pos;
        if self.peek(0).is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'_' | b':')) {
            while self.peek(0).is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b':' | b'.' | b'-')) {
                self.pos += 1;
            }
        }
        let name = self.text_from(start);
        if !name.is_empty()
            && self.peek(0) == Some(b';')
            && let Ok(index) = ENTITIES.binary_search_by(|(n, _)| (*n).cmp(name))
        {
            self.pos += 1;
            return char::from_u32(ENTITIES[index].1).map(String::from).unwrap_or_default();
        }
        format!("&{name}")
    }

    /// `htmlParseScript`: everything up to `</name` (any case) is text.
    fn raw_text(&mut self, name: &str) {
        let end = format!("</{name}");
        while self.pos < self.bytes.len() && !self.starts_with_ignore_case(&end) {
            self.pos += 1;
        }
    }
}

/// `Nokogiri::HTML4::Document#meta_encoding`: the first `meta[@charset]`, else the charset in
/// the first `http-equiv="Content-Type"` meta with a `content`.
pub fn meta_encoding(metas: &[Element]) -> Option<String> {
    if let Some(meta) = metas.iter().find(|m| m.has_attr("charset")) {
        return meta.attr("charset").map(str::to_string);
    }
    let meta =
        metas.iter().find(|m| m.has_attr("content") && m.attr("http-equiv").is_some_and(|v| v.eq_ignore_ascii_case("content-type")))?;
    charset_in(meta.attr("content")?)
}

/// `content[/charset\s*=\s*([\w-]+)/i, 1]`
fn charset_in(content: &str) -> Option<String> {
    static CHARSET: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i-u)charset[ \t\n\x0B\x0C\r]*=[ \t\n\x0B\x0C\r]*([A-Za-z0-9_-]+)").unwrap());
    CHARSET.captures(content).map(|c| c[1].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(html: &str) -> Option<String> {
        meta_elements(&format!("<meta charset=utf-8>{html}"))
            .into_iter()
            .rfind(|m| m.attr("property") == Some("og:title"))
            .and_then(|m| m.attr("content").filter(|c| !c.is_empty()).map(str::to_string))
    }

    fn title_of(content: &str) -> Option<String> {
        title(&format!("<meta property=\"og:title\" content=\"a{content}b\">"))
    }

    /// Probed against the reference's Nokogiri 1.19.4.
    #[test]
    fn decodes_references_like_libxml2() {
        for (reference, expected) in [
            ("&apos;", "a'b"),
            ("&eacute", "a&eacuteb"),
            ("&eacute;x", "aéxb"),
            ("&#233", "aéb"),
            ("&#233x", "aéxb"),
            ("&#xE9", "a\u{0E9B}"),
            ("&#xe9;", "aéb"),
            ("&AMP;", "a&AMP;b"),
            ("&Eacute;", "aÉb"),
            ("&unknown;", "a&unknown;b"),
            ("& x", "a& xb"),
            ("&#65;&#x41;", "aAAb"),
            ("&#128;", "a\u{80}b"),
            ("&#150;", "a\u{96}b"),
            ("&#xD800;", "a"),
            ("&#1114112;", "a"),
            ("&lt", "a&ltb"),
            ("&amp;amp;", "a&amp;b"),
            ("&hellip;", "a…b"),
            ("&nbsp", "a&nbspb"),
            ("&#;", "a"),
            ("&#x;", "a"),
        ] {
            assert_eq!(title_of(reference).as_deref(), Some(expected), "{reference}");
        }
    }

    #[test]
    fn tokenizes_like_libxml2() {
        let cases: &[(&str, Option<&str>)] = &[
            (
                "<script><meta property=\"og:title\" content=\"in script\"></script><meta property=\"og:title\" content=\"after\">",
                Some("after"),
            ),
            ("<style><meta property=\"og:title\" content=\"in style\"></style>", None),
            ("<textarea><meta property=\"og:title\" content=\"in textarea\"></textarea>", Some("in textarea")),
            ("<title><meta property=\"og:title\" content=\"in title\"></title>", Some("in title")),
            ("<noscript><meta property=\"og:title\" content=\"in noscript\"></noscript>", Some("in noscript")),
            ("<template><meta property=\"og:title\" content=\"in template\"></template>", Some("in template")),
            ("<svg><meta property=\"og:title\" content=\"in svg\"></svg>", Some("in svg")),
            ("<!-- <meta property=\"og:title\" content=\"comment\"> --><p>", None),
            ("<meta property=og:title content=unquoted>", Some("unquoted")),
            ("<meta property=\"og:title\" content=\"line1\r\nline2\">", Some("line1\r\nline2")),
            ("<meta property='og:title' content='single'>", Some("single")),
            ("<meta property = \"og:title\" content = \"spaced\">", Some("spaced")),
            ("<META PROPERTY=\"og:title\" CONTENT=\"upper\">", Some("upper")),
            ("<meta property=\"og:title\"content=\"nospace\">", Some("nospace")),
            ("<meta/property=\"og:title\"/content=\"slashes\">", None),
            ("<meta property=\"og:title\" content=\"<b>tag</b>\">", Some("<b>tag</b>")),
            ("<meta property=\"og:title\" content=\"a\">b\">", Some("a")),
            ("<meta property=\"og:title\" content=\"unterminated>", Some("unterminated>")),
            ("<meta property=\"og:title\" content=unq\"uoted>", Some("unq\"uoted")),
            ("<meta property=\"og:title\" content=a&amp;b>", Some("a&b")),
            ("<!--> <meta property=\"og:title\" content=\"after empty comment\"> -->", Some("after empty comment")),
            ("<!---> <meta property=\"og:title\" content=\"after dash comment\"> -->", Some("after dash comment")),
            ("<!DOCTYPE html><meta property=\"og:title\" content=\"doctype\">", Some("doctype")),
            ("<?xml version=\"1.0\"?><meta property=\"og:title\" content=\"pi\">", Some("pi")),
            ("<![CDATA[ <meta property=\"og:title\" content=\"cdata\"> ]]>", None),
            ("<p <meta property=\"og:title\" content=\"broken\">", None),
            ("< meta property=\"og:title\" content=\"space\">", None),
            ("<meta property=\"og:title\" content=\"tab\there\">", Some("tab\there")),
            ("<meta property=\"og:title\" content=\"\x00nul\">", None),
        ];
        for (html, expected) in cases {
            assert_eq!(title(html).as_deref(), *expected, "{html}");
        }
    }

    #[test]
    fn decodes_bytes_like_libxml2_reading_utf8() {
        assert_eq!(decode(b"caf\xc3\xa9 \xff x"), "café ÿ x");
        assert_eq!(decode(b"\xff caf\xc3\xa9 x"), "ÿ café x");
        assert_eq!(decode(b"\x93q\x94"), "\u{93}q\u{94}");
        assert_eq!(decode(b"\x82\xa0"), "\u{82}\u{a0}");
    }

    #[test]
    fn finds_the_meta_encoding_like_nokogiri() {
        let encoding = |html: &str| meta_encoding(&meta_elements(html));
        assert_eq!(encoding("<meta charset=\"iso-8859-1\">"), Some("iso-8859-1".into()));
        assert_eq!(encoding("<meta charset=\"\">"), Some("".into()));
        assert_eq!(encoding("<meta http-equiv=\"content-type\" content=\"text/html; charset=iso-8859-1\">"), Some("iso-8859-1".into()));
        assert_eq!(
            encoding(
                "<meta http-equiv=\"Content-Type\" content=\"text/html\"><meta http-equiv=\"Content-Type\" content=\"charset=utf-8\">"
            ),
            None
        );
        assert_eq!(encoding("<meta http-equiv=\"refresh\" content=\"charset=utf-8\">"), None);
        assert_eq!(encoding("<meta property=\"og:title\" content=\"x\">"), None);
    }
}
