//! `Opengraph::Document` (reference/app/models/opengraph/document.rb).

use super::html::{self, Element};

/// `Opengraph::Metadata::ATTRIBUTES`, in the order `Hash#slice` returns them.
pub const ATTRIBUTES: [&str; 4] = ["title", "url", "image", "description"];

/// `opengraph_attributes`: from each `meta` whose `property` or `name` starts with "og:", the
/// key is that attribute (`property` when present) with every "og:" removed, and the value its
/// non-blank `content`. Later tags win. Without a meta charset, non-ASCII characters are
/// dropped (`content.encode("UTF-8", "binary", invalid: :replace, undef: :replace, replace: "")`).
pub fn opengraph_attributes(body: Option<&[u8]>) -> Vec<(&'static str, String)> {
    let html = html::decode(body.unwrap_or_default());
    let metas = html::meta_elements(&html);
    let meta_encoding = html::meta_encoding(&metas);

    // Only the `ATTRIBUTES` keys are sliced out, so only they are kept.
    let mut found: [Option<String>; ATTRIBUTES.len()] = Default::default();
    for meta in metas.iter().filter(|m| is_opengraph_tag(m)) {
        let key = if meta.has_attr("property") { "property" } else { "name" };
        let name = meta.attr(key).unwrap_or("").replace("og:", "");
        let Some(index) = ATTRIBUTES.iter().position(|a| *a == name) else { continue };
        let Some(content) = meta.attr("content").filter(|c| !is_blank(c)) else { continue };
        let content = if meta_encoding.is_some() { content.to_string() } else { content.chars().filter(char::is_ascii).collect() };
        found[index] = Some(content);
    }

    ATTRIBUTES.into_iter().zip(found).filter_map(|(key, value)| Some((key, value?))).collect()
}

/// `//*/meta[starts-with(@property, "og:") or starts-with(@name, "og:")]`
fn is_opengraph_tag(meta: &Element) -> bool {
    meta.attr("property").is_some_and(|p| p.starts_with("og:")) || meta.attr("name").is_some_and(|n| n.starts_with("og:"))
}

/// `String#blank?`: empty or only (Unicode) whitespace.
pub fn is_blank(s: &str) -> bool {
    s.chars().all(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// test/models/opengraph/document_test.rb
    #[test]
    fn extracts_opengraph_tags() {
        let expected = vec![
            ("title", "Hey!".to_string()),
            ("url", "https://example.com".to_string()),
            ("image", "https://example.com/image.png".to_string()),
            ("description", "desc..".to_string()),
        ];
        let html = "<html><head><meta property=\"og:url\" content=\"https://example.com\"><meta property=\"og:title\" content=\"Hey!\"><meta property=\"og:description\" content=\"desc..\"><meta property=\"og:image\" content=\"https://example.com/image.png\"></head></html>";
        assert_eq!(opengraph_attributes(Some(html.as_bytes())), expected);
        assert_eq!(opengraph_attributes(Some(html.replace("property=", "name=").as_bytes())), expected);

        let html = "<html><head><meta name=\"og:url\" content=\"https://example.com\"><meta name=\"og:title\" content=\"Hey!\"><meta name=\"og:description\" content=\"Hello â\u{0080}\u{0099}World\"><meta name=\"og:image\" content=\"https://example.com/image.png\"></head></html>";
        let attributes = opengraph_attributes(Some(html.as_bytes()));
        assert_eq!(attributes[3], ("description", "Hello World".to_string()));
        assert!(opengraph_attributes(None).is_empty());
    }

    /// Pages as large as a fetch allows, built to make a parser do quadratic work, parse in time
    /// proportional to their size. Quadratic parsing took 14.7 s for 1.4 MB, so it would take
    /// minutes here; the bound leaves room for a debug build on a busy CI runner, which has taken
    /// just over 1 s.
    #[test]
    fn parses_pathological_pages_quickly() {
        let limit = super::super::fetch::MAX_BODY_SIZE;
        let fill = |open: &str, item: &dyn Fn(usize) -> String, close: &str| {
            let mut page = String::from(open);
            for i in 0.. {
                if page.len() > limit - close.len() - 64 {
                    break;
                }
                page.push_str(&item(i));
            }
            page + close
        };
        let pages = [
            fill("<meta property=\"og:title\" content=\"x\" ", &|i| format!("a{i:07} "), ">"),
            fill(
                "<meta charset=utf-8>",
                &|i| format!("<meta property=\"og:t{i}\" content=\"x\">"),
                "<meta property=\"og:title\" content=\"x\">",
            ),
            fill("<meta property=\"og:title\" content=\"", &|_| "&amp;é".to_string(), "\">"),
        ];
        for page in pages {
            let started = std::time::Instant::now();
            let found = opengraph_attributes(Some(page.as_bytes()));
            assert_eq!(found[0].0, "title");
            assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?} for {}…", started.elapsed(), &page[..60]);
        }
    }
}
