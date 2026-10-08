//! DOM parity of the search views with the reference app (goldens in tests/golden/b).

mod messages_support;

use askama::Template;
use matchbox_views::searches::{self, IndexView};
use messages_support::golden;

#[test]
fn index_with_results() {
    let g = golden("searches_index");
    let index: IndexView = g.input();
    let html = g.render(|ctx| searches::Index { ctx, index: &index }.render().unwrap());
    assert!(!index.recent_searches.is_empty());
    let recents = html.split(r#"aria-label="Recent searches">"#).nth(1).unwrap().split("</div>").next().unwrap();
    for query in &index.recent_searches {
        let link = format!("href=\"{}\"", searches::search_path(query));
        assert_eq!(html.matches(&link).count(), 1);
        assert!(recents.contains(&link));
    }
    g.assert_dom(&html);
}

#[test]
fn index_without_query() {
    let g = golden("searches_index_empty");
    let index: IndexView = g.input();
    g.assert_dom(&g.render(|ctx| searches::Index { ctx, index: &index }.render().unwrap()));
}
