//! KRO account setup, keeping `reference/app/views/first_runs`' form contract.

use askama::Template;

use crate::ViewContext;
use crate::helpers::{self as h, filters};
use crate::layouts::Page;

#[derive(Clone, Copy, Default)]
pub struct FormValues<'a> {
    pub name: Option<&'a str>,
    pub email_address: Option<&'a str>,
    pub password_error: bool,
}

/// `first_runs/show.html.erb`: account setup, shown until the first user exists.
#[derive(Template)]
#[template(path = "first_runs/show.html", blocks = ["head", "content"])]
pub struct Show<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub values: FormValues<'a>,
    /// A frame from the legacy stylesheet profile must promote to a full document visit.
    pub reload_frame: bool,
}

impl Page for Show<'_> {
    fn page_title(&self) -> Option<String> {
        Some("Set up Ember".into())
    }
    fn body_class(&self) -> Option<&str> {
        Some("signup")
    }
}
