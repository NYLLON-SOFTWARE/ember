//! `SearchesController#query` (reference/app/controllers/searches_controller.rb):
//! `params[:q]&.gsub(/[^[:word:]]/, " ")`, which leaves only Onigmo's Unicode word characters
//! (alphabetic, marks, decimal digits, connector punctuation, join controls) for the FTS5
//! `MATCH`.

#[rustfmt::skip]
mod word_ranges;

use word_ranges::WORD_RANGES;

/// Replaces every character that isn't a `[[:word:]]` character with a space (one space per
/// character). `None` stays `None` (no `q` param).
pub fn sanitize_query(q: Option<&str>) -> Option<String> {
    q.map(|q| q.chars().map(|c| if is_word(c) { c } else { ' ' }).collect())
}

/// Onigmo's `[[:word:]]` in the reference's Ruby (Unicode 15.0), from the generated table.
pub fn is_word(c: char) -> bool {
    let c = c as u32;
    WORD_RANGES
        .binary_search_by(|&(start, end)| {
            if end < c {
                std::cmp::Ordering::Less
            } else if start > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_word_characters_like_ruby() {
        // Probed against the reference's Ruby
        assert_eq!(
            sanitize_query(Some("héllo wörld_1 ２ 日本語 ‿ a-b \u{fe0f} ❤ é")).as_deref(),
            Some("héllo wörld_1 ２ 日本語 ‿ a b \u{fe0f}   é")
        );
        assert_eq!(sanitize_query(Some("\"quoted\" OR NEAR(x*)")).as_deref(), Some(" quoted  OR NEAR x  "));
        assert_eq!(sanitize_query(Some("")).as_deref(), Some(""));
        assert_eq!(sanitize_query(None), None);
    }

    #[test]
    fn classifies_like_onigmo() {
        // Probed against the reference's Ruby: alphabetic (including letter numbers and circled
        // letters), marks, decimal digits, connector punctuation and join controls.
        for c in ['a', 'Z', '0', '_', 'é', 'ß', '日', '‿', '\u{0301}', '٣', 'ǅ', 'ʰ', 'Ⅻ', 'Ⓐ', '\u{200d}'] {
            assert!(is_word(c), "{c:?} is a word character");
        }
        for c in [' ', '-', '*', '"', '\u{a0}', '❤', '€', '½', '²'] {
            assert!(!is_word(c), "{c:?} is not a word character");
        }
    }
}
