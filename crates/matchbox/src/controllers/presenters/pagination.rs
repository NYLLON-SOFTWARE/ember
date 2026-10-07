//! geared_pagination 1.2.0's `set_page_and_extract_portion_from records, per_page:` for an
//! unordered-by-cursor relation (`PortionAtOffset`), and its JSON response headers.

use matchbox_kit::Ctx;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// `page.number`
    pub number: i64,
    /// `recordset.records_count`
    pub records_count: i64,
    ratios: Vec<i64>,
}

impl Page {
    /// `Recordset.new(records, per_page:).page(params[:page])`.
    pub fn new(page_param: Option<&str>, records_count: i64, per_page: &[i64]) -> Self {
        Self { number: page_number_from(page_param), records_count, ratios: per_page.to_vec() }
    }

    fn ratio(&self, page_number: i64) -> i64 {
        let fixed = *self.ratios.last().expect("ratios");
        usize::try_from(page_number - 1).ok().and_then(|i| self.ratios.get(i).copied()).unwrap_or(fixed)
    }

    /// `PortionAtOffset#limit`
    pub fn limit(&self) -> i64 {
        self.ratio(self.number)
    }

    /// `PortionAtOffset#offset`
    pub fn offset(&self) -> i64 {
        let size = self.ratios.len() as i64;
        let variable: i64 = (0..(self.number - 1).min(size - 1)).map(|index| self.ratio(index + 1)).sum();
        let fixed = (self.number - size).max(0) * self.ratio(size);
        variable + fixed
    }

    /// The page's slice of the (already ordered) records.
    pub fn records<T: Clone>(&self, all: &[T]) -> Vec<T> {
        all.iter().skip(self.offset() as usize).take(self.limit() as usize).cloned().collect()
    }

    /// `recordset.page_count`
    pub fn page_count(&self) -> i64 {
        let mut count = 0;
        let mut residual = self.records_count;
        while residual > 0 {
            count += 1;
            residual -= self.ratio(count);
        }
        count.max(1)
    }

    /// `page.last?`
    pub fn is_last(&self) -> bool {
        self.number == self.page_count()
    }

    /// `page.next_param`
    pub fn next_param(&self) -> i64 {
        self.number + 1
    }

    /// `set_paginated_headers` (after_action), for JSON requests: `X-Total-Count`, and a `Link`
    /// to the next page unless this is the last one.
    pub fn apply_headers(&self, c: &mut Ctx) {
        let json = c.formats().ok().and_then(|formats| formats.first().copied()).is_some_and(|format| format.is("json"));
        if !json {
            return;
        }
        c.set_header("x-total-count", &self.records_count.to_string());
        if !self.is_last() {
            let url = with_page(&c.request.url(), &self.next_param().to_string());
            c.set_header("link", &format!("<{url}>; rel=\"next\""));
        }
    }
}

/// `param.to_i > 0 ? param.to_i : 1`
fn page_number_from(param: Option<&str>) -> i64 {
    // Capped, so the page arithmetic can't overflow on a huge `?page=`.
    param.map(ruby_compat::to_i).unwrap_or(0).clamp(1, 1_000_000_000)
}

/// Addressable's `uri.query_values = (uri.query_values || {}).merge("page" => page)`: the query
/// is re-encoded from a hash, so keys come out sorted, duplicates collapse to the last value, and
/// components are percent-encoded outside Addressable's unreserved set.
fn with_page(url: &str, page: &str) -> String {
    let (base, fragment) = match url.split_once('#') {
        Some((base, fragment)) => (base, Some(fragment)),
        None => (url, None),
    };
    let (path, query) = match base.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (base, None),
    };
    let mut values: Vec<(String, Option<String>)> = Vec::new();
    for pair in query.unwrap_or("").split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (unencode(key), Some(unencode(&value.replace('+', " ")))),
            None => (unencode(pair), None),
        };
        match values.iter_mut().find(|(existing, _)| *existing == key) {
            Some(entry) => entry.1 = value,
            None => values.push((key, value)),
        }
    }
    match values.iter_mut().find(|(key, _)| key == "page") {
        Some(entry) => entry.1 = Some(page.to_string()),
        None => values.push(("page".into(), Some(page.to_string()))),
    }
    values.sort_by(|a, b| a.0.cmp(&b.0));
    let query: Vec<String> = values
        .iter()
        .map(|(key, value)| match value {
            Some(value) => format!("{}={}", encode_component(key), encode_component(value)),
            None => encode_component(key),
        })
        .collect();
    let mut result = format!("{path}?{}", query.join("&"));
    if let Some(fragment) = fragment {
        result.push('#');
        result.push_str(fragment);
    }
    result
}

/// `Addressable::URI.unencode_component`. `query_values` turns "+" in values (not keys) into
/// spaces first, so an encoded `%2B` stays a "+".
fn unencode(value: &str) -> String {
    percent_encoding::percent_decode_str(value).decode_utf8_lossy().into_owned()
}

/// `Addressable::URI.encode_component(value, CharacterClassesRegexps::UNRESERVED)`, which keeps
/// the same characters as `ERB::Util.url_encode` (ruby_compat checks both against the reference).
fn encode_component(value: &str) -> String {
    ruby_compat::url_encode(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_with_one_ratio() {
        let page = Page::new(Some("2"), 1005, &[500]);
        assert_eq!((page.offset(), page.limit(), page.page_count()), (500, 500, 3));
        assert!(!page.is_last());
        assert!(Page::new(Some("3"), 1005, &[500]).is_last());
        assert!(Page::new(None, 0, &[20]).is_last());
    }

    #[test]
    fn offsets_with_default_ratios() {
        // `GearedPagination::Ratios::DEFAULTS`
        let page = |n: &str| Page::new(Some(n), 1000, &[15, 30, 50, 100]);
        assert_eq!((page("1").offset(), page("1").limit()), (0, 15));
        assert_eq!((page("3").offset(), page("3").limit()), (45, 50));
        assert_eq!((page("5").offset(), page("5").limit()), (195, 100));
        assert_eq!((page("6").offset(), page("6").limit()), (295, 100));
    }

    #[test]
    fn huge_page_numbers_are_capped() {
        let page = Page::new(Some("99999999999999999999"), 10, &[5]);
        assert_eq!(page.number, 1_000_000_000);
        assert_eq!(page.next_param(), 1_000_000_001);
        assert!(page.offset() > 0);
    }

    #[test]
    fn page_params_like_ruby_to_i() {
        assert_eq!(Page::new(Some("abc"), 10, &[5]).number, 1);
        assert_eq!(Page::new(Some("-2"), 10, &[5]).number, 1);
        assert_eq!(Page::new(Some(" 2x"), 10, &[5]).number, 2);
        assert_eq!(Page::new(Some("1_0"), 100, &[5]).number, 10);
    }

    #[test]
    fn next_links_merge_the_page_into_sorted_query_values() {
        assert_eq!(with_page("http://x.test/autocompletable/users.json", "2"), "http://x.test/autocompletable/users.json?page=2");
        assert_eq!(
            with_page("http://x.test/autocompletable/users.json?query=a+b&page=1&room_id=3", "2"),
            "http://x.test/autocompletable/users.json?page=2&query=a%20b&room_id=3"
        );
    }

    #[test]
    fn plus_is_a_space_only_before_unencoding() {
        // Addressable's `query_values=` after `query_values` in the reference.
        for (query, next) in [
            ("query=a%2Bb", "page=2&query=a%2Bb"),
            ("q=a%2B+b&page=1", "page=2&q=a%2B%20b"),
            ("a+b=c+d", "a%2Bb=c%20d&page=2"),
            ("k=%20+", "k=%20%20&page=2"),
            ("k", "k&page=2"),
        ] {
            assert_eq!(with_page(&format!("http://x.test/p?{query}"), "2"), format!("http://x.test/p?{next}"), "{query}");
        }
    }
}
