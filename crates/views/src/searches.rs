//! Views for `reference/app/views/searches`, plus `SearchesHelper`.

use askama::Template;
use serde::Deserialize;

use crate::ViewContext;
use crate::helpers as h;
use crate::layouts::Page;
use crate::messages::MessageItem;

/// What `searches/index` shows.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct IndexView {
    /// `@query`: `params[:q]` with non-word characters turned into spaces, when present.
    pub query: Option<String>,
    /// `params[:q]` as submitted, the search field's value.
    pub q: Option<String>,
    /// `Current.user.reachable_messages.search(query).last(100)`.
    pub messages: Vec<MessageItem>,
    /// `Current.user.searches.ordered.pluck(:query)`.
    pub recent_searches: Vec<String>,
    /// `last_room_visited.id`, where the exit button goes.
    pub return_to_room_id: i64,
}

/// `searches/index`.
#[derive(Template)]
#[template(path = "searches/index.html", blocks = ["head", "content"])]
pub struct Index<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub index: &'a IndexView,
}

impl Page for Index<'_> {
    fn page_title(&self) -> Option<String> {
        Some("Search".into())
    }

    fn body_class(&self) -> Option<&str> {
        Some("sidebar searches")
    }
}

/// `searches_path(q: query)`.
pub fn search_path(query: &str) -> String {
    format!("{}?q={}", ember_routes::searches(), ruby_compat::cgi_escape(query))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_paths_escape_the_query_like_cgi_escape() {
        assert_eq!(search_path(r#"pizza & "pie" *~"#), "/searches?q=pizza+%26+%22pie%22+%2A~");
    }
}
