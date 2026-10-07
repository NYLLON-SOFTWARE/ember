//! Views for `reference/app/views/layouts`.
//!
//! A page template starts with `{% extends "layouts/application.html" %}`, fills the `content`
//! block (and `head`, `nav`, `footer`, `sidebar` for `content_for`), has a `ctx: &ViewContext`
//! field, and implements [`Page`] for what the ERB sets as `@page_title` and `@body_class`.

use askama::Template;

use crate::helpers as h;
use crate::recorded::{self, RecordedPage};

/// The instance variables a page template hands to the application layout.
pub trait Page {
    /// `@page_title`; the layout falls back to "Matchbox".
    fn page_title(&self) -> Option<String> {
        None
    }

    /// `@body_class`.
    fn body_class(&self) -> Option<&str> {
        None
    }
}

/// The application layout around page parts rendered elsewhere, for templates that don't extend
/// the layout themselves (Rails picks the layout per request). Each part is what the ERB's
/// `content_for` / `yield` would have produced.
#[derive(Template)]
#[template(path = "layouts/application_wrapper.html")]
pub struct Application<'a> {
    pub ctx: &'a crate::ViewContext<'a>,
    /// `@page_title`.
    pub page_title: Option<String>,
    /// `@body_class`.
    pub body_class: Option<String>,
    pub head: h::Html,
    pub nav: h::Html,
    pub content: h::Html,
    pub footer: h::Html,
    pub sidebar: h::Html,
}

impl<'a> Application<'a> {
    /// Just a page body, with no title, body class or `content_for` regions.
    pub fn new(ctx: &'a crate::ViewContext<'a>, content: h::Html) -> Self {
        Application {
            ctx,
            page_title: None,
            body_class: None,
            head: h::empty(),
            nav: h::empty(),
            content,
            footer: h::empty(),
            sidebar: h::empty(),
        }
    }
}

impl Page for Application<'_> {
    fn page_title(&self) -> Option<String> {
        self.page_title.clone()
    }

    fn body_class(&self) -> Option<&str> {
        self.body_class.as_deref()
    }
}

/// turbo-rails' `layouts/turbo_rails/frame.html.erb`, used instead of the application layout
/// whenever a request carries a `Turbo-Frame` header. Pages expose their blocks for it through
/// askama's `blocks = ["head", "content"]` (see [`frame`]).
#[derive(Template)]
#[template(path = "layouts/turbo_rails/frame.html")]
pub struct FrameLayout<'a> {
    pub ctx: &'a crate::ViewContext<'a>,
    /// The page's `:head` content.
    pub head: h::Html,
    /// The page itself.
    pub content: h::Safe<RecordedPage>,
}

/// Renders a page's `head` and `content` blocks in the Turbo-Frame layout, recorded (the content's
/// cached fragments too): `frame(ctx, page.as_head(), page.as_content())`.
pub fn frame<C: Template>(ctx: &crate::ViewContext, head: impl Template, content: C) -> askama::Result<RecordedPage> {
    let content = recorded::render(&content, C::SIZE_HINT)?;
    let layout = FrameLayout { ctx, head: h::raw(head.render()?), content: h::raw(content) };
    recorded::render(&layout, FrameLayout::SIZE_HINT + layout.content.0.text().len())
}
