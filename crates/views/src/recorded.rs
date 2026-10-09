//! Pages rendered with their cached fragments left out of the page's text, and recorded where
//! they go instead. A room page is mostly its ~40 message fragments (~400 KB); copying them into
//! one buffer only for the kit to find them in it again (for its gzip and ETag, which reuse what
//! it knows about each fragment) costs more than the rest of assembling the page. So a template
//! [`hand`]s a fragment over just before writing it, and the page's writer, seeing those very
//! bytes arrive, notes the fragment at its offset in the text instead of copying it. Anything
//! else, including a fragment that wasn't handed over, is copied in as text, so the page is
//! always the same bytes a plain render gives.

use std::cell::{Cell, RefCell};
use std::fmt;

use askama::Template;

use crate::fragment_cache::Fragment;

/// Shorter writes are copied without looking for a fragment: the kit keeps fragments under 1 KB
/// in the text around them anyway (`ember_kit::deflater::splice`), and checking every short
/// write would cost more than copying it.
const MIN_RECORDED: usize = 1024;

/// A rendered page: its text, and each cached fragment with its byte offset in the text, in order.
#[derive(Debug)]
pub struct RecordedPage {
    text: String,
    fragments: Vec<(usize, Fragment)>,
}

impl RecordedPage {
    pub fn into_parts(self) -> (String, Vec<(usize, Fragment)>) {
        (self.text, self.fragments)
    }

    /// The text, without its fragments.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn fragments(&self) -> &[(usize, Fragment)] {
        &self.fragments
    }
}

/// A page rendered without recording.
impl From<String> for RecordedPage {
    fn from(text: String) -> Self {
        Self { text, fragments: Vec::new() }
    }
}

/// The whole page, handing its fragments over again, so that a page written into another one
/// (turbo-rails' frame layout around a page's content) keeps them.
impl fmt::Display for RecordedPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut position = 0;
        for (offset, fragment) in &self.fragments {
            f.write_str(&self.text[position..*offset])?;
            hand(fragment);
            f.write_str(fragment)?;
            position = *offset;
        }
        f.write_str(&self.text[position..])
    }
}

thread_local! {
    /// Whether a page is rendering into a [`Recorder`] on this thread.
    static RECORDING: Cell<bool> = const { Cell::new(false) };
    /// The fragment a template is about to write, while a page is recording.
    static HANDED: RefCell<Option<Fragment>> = const { RefCell::new(None) };
}

/// Tells the page being recorded, if any, that `fragment` is what the template writes next.
pub fn hand(fragment: &Fragment) {
    if RECORDING.get() {
        HANDED.set(Some(fragment.clone()));
    }
}

/// The handed fragment, if `written` is its very bytes (not merely equal ones).
fn take_handed(written: &str) -> Option<Fragment> {
    HANDED.with_borrow_mut(|handed| handed.take_if(|fragment| std::ptr::eq(fragment.as_str(), written)))
}

/// `template` rendered and recorded, into a text buffer of `capacity` bytes.
pub fn render<T: Template + ?Sized>(template: &T, capacity: usize) -> askama::Result<RecordedPage> {
    let _recording = Recording::start();
    let mut recorder = Recorder(RecordedPage { text: String::with_capacity(capacity), fragments: Vec::new() });
    template.render_into(&mut recorder)?;
    Ok(recorder.0)
}

/// Marks this thread as recording until dropped (restoring what it was before).
struct Recording {
    was: bool,
}

impl Recording {
    fn start() -> Self {
        Self { was: RECORDING.replace(true) }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        RECORDING.set(self.was);
        HANDED.take();
    }
}

/// The writer a recorded page renders into.
struct Recorder(RecordedPage);

impl fmt::Write for Recorder {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let page = &mut self.0;
        match (s.len() >= MIN_RECORDED).then(|| take_handed(s)).flatten() {
            Some(fragment) => page.fragments.push((page.text.len(), fragment)),
            None => page.text.push_str(s),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A fragment written the way `messages::cached_message_item` writes one.
    struct Handed(Fragment);

    impl fmt::Display for Handed {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            hand(&self.0);
            f.write_str(&self.0)
        }
    }

    #[derive(Template)]
    #[template(source = "<ul>{% for item in items %}\n  {{ item|safe }}{% endfor %}\n</ul>{{ unhanded|safe }}", ext = "html")]
    struct List<'a> {
        items: &'a [Handed],
        unhanded: &'a str,
    }

    #[derive(Template)]
    #[template(source = "<frame>{{ content|safe }}</frame>", ext = "html")]
    struct Frame<'a> {
        content: &'a RecordedPage,
    }

    fn fragment(n: usize) -> Fragment {
        Arc::new(format!("<li id=\"{n}\">{}</li>", "x".repeat(MIN_RECORDED + n)))
    }

    fn list() -> Vec<Handed> {
        vec![Handed(fragment(1)), Handed(Arc::new("<li>small</li>".into())), Handed(fragment(2))]
    }

    #[test]
    fn records_handed_fragments_and_renders_the_same_bytes() {
        let items = list();
        let unhanded = fragment(3);
        let template = List { items: &items, unhanded: &unhanded };
        let page = render(&template, 0).unwrap();
        assert_eq!(page.to_string(), template.render().unwrap());
        let recorded: Vec<_> = page.fragments().iter().map(|(offset, fragment)| (*offset, fragment.clone())).collect();
        assert_eq!(recorded.len(), 2, "the small one and the unhanded one are text");
        assert!(Arc::ptr_eq(&recorded[0].1, &items[0].0) && Arc::ptr_eq(&recorded[1].1, &items[2].0));
        assert_eq!(&page.text()[..recorded[0].0], "<ul>\n  ");
        assert_eq!(&page.text()[recorded[0].0..recorded[1].0], "\n  <li>small</li>\n  ");
    }

    #[test]
    fn a_page_written_into_another_keeps_its_fragments() {
        let items = list();
        let content = render(&List { items: &items, unhanded: "" }, 0).unwrap();
        let framed = render(&Frame { content: &content }, 0).unwrap();
        assert_eq!(framed.to_string(), format!("<frame>{content}</frame>"));
        assert_eq!(framed.fragments().len(), 2);
    }

    #[test]
    fn equal_bytes_elsewhere_are_not_the_fragment() {
        let items = [Handed(fragment(1))];
        let copy = String::clone(&items[0].0);
        hand(&items[0].0);
        let page = render(&List { items: &[], unhanded: &copy }, 0).unwrap();
        assert!(page.fragments().is_empty());
        let page = render(&List { items: &items, unhanded: &copy }, 0).unwrap();
        assert_eq!(page.fragments().len(), 1);
        assert_eq!(page.to_string(), format!("<ul>\n  {}\n</ul>{copy}", items[0].0));
    }

    #[test]
    fn outside_a_recording_nothing_is_held() {
        hand(&fragment(1));
        assert!(HANDED.with_borrow(Option::is_none));
    }
}
