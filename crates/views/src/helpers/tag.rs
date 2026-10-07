//! `ActionView::Helpers::TagHelper`: attribute rendering (`tag_options`) and element builders.
//!
//! [`Attrs`] keeps insertion order, since Rails renders attributes in hash order. Build one with
//! [`attrs()`] and chain setters, mirroring the Ruby options hash:
//! `attrs().class("btn").data("turbo_frame", "_top")` is `class: "btn", data: { turbo_frame: "_top" }`.
//! Helpers that add to or rearrange the caller's options work on a borrowed [`AttrsView`], so a
//! block filter's options aren't cloned.

use std::borrow::Cow;
use std::fmt::{Display, Write as _};

use super::html::{Html, Safe, push_escaped};

/// `TagHelper::BOOLEAN_ATTRIBUTES`: rendered as `name="name"` when true, omitted when false.
const BOOLEAN_ATTRIBUTES: &[&str] = &[
    "allowfullscreen", "allowpaymentrequest", "async", "autofocus", "autoplay", "checked", "compact", "controls", "declare", "default",
    "defaultchecked", "defaultmuted", "defaultselected", "defer", "disabled", "enabled", "formnovalidate", "hidden", "indeterminate",
    "inert", "ismap", "itemscope", "loop", "multiple", "muted", "nohref", "nomodule", "noresize", "noshade", "novalidate", "nowrap",
    "open", "pauseonexit", "playsinline", "readonly", "required", "reversed", "scoped", "seamless", "selected", "sortable", "truespeed",
    "typemustmatch", "visible",
];

/// HTML void elements, which the `tag.*` builder renders without a closing tag.
const VOID_ELEMENTS: &[&str] =
    &["area", "base", "br", "col", "embed", "hr", "img", "input", "keygen", "link", "meta", "source", "track", "wbr"];

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A plain string, escaped on output.
    Text(String),
    /// An html_safe string: only `"` is replaced on output.
    Safe(String),
    Bool(bool),
}

/// Any borrowed displayable value (`&str`, `&String`, `&i64`, askama's `a ~ b`) is plain text.
impl<T: std::fmt::Display + ?Sized> From<&T> for Value {
    fn from(value: &T) -> Self {
        Value::Text(value.to_string())
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Text(value)
    }
}
impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}
impl From<Html> for Value {
    fn from(value: Html) -> Self {
        Value::Safe(value.0)
    }
}
macro_rules! value_from_number {
    ($($t:ty),*) => { $(
        impl From<$t> for Value {
            fn from(value: $t) -> Self { Value::Text(value.to_string()) }
        }
    )* };
}
value_from_number!(i32, i64, u32, u64, usize, f64);

impl Value {
    pub fn as_attr(&self) -> AttrValue<'_> {
        match self {
            Value::Text(text) => AttrValue::Text(text),
            Value::Safe(html) => AttrValue::Safe(html),
            Value::Bool(flag) => AttrValue::Bool(*flag),
        }
    }
}

/// A borrowed [`Value`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AttrValue<'a> {
    Text(&'a str),
    Safe(&'a str),
    Bool(bool),
}

impl<'a> AttrValue<'a> {
    /// The value as Ruby's `to_s` prints it.
    pub fn as_str(self) -> &'a str {
        match self {
            AttrValue::Text(text) | AttrValue::Safe(text) => text,
            AttrValue::Bool(true) => "true",
            AttrValue::Bool(false) => "false",
        }
    }
}

impl<'a> From<&'a str> for AttrValue<'a> {
    fn from(value: &'a str) -> Self {
        AttrValue::Text(value)
    }
}
impl From<bool> for AttrValue<'_> {
    fn from(value: bool) -> Self {
        AttrValue::Bool(value)
    }
}

/// An ordered options hash. A `None` value is kept so that later assignments stay in place
/// (Ruby's `options["value"] = nil` keeps the key's position) but is never rendered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attrs(Vec<(Cow<'static, str>, Option<Value>)>);

pub fn attrs() -> Attrs {
    Attrs::default()
}

impl Attrs {
    /// Sets `name`, replacing an earlier value in place (Ruby hash assignment semantics).
    pub fn attr(mut self, name: impl Into<Cow<'static, str>>, value: impl Into<Value>) -> Self {
        self.set(name, Some(value.into()));
        self
    }

    /// Sets `name` only when `value` is `Some`, like passing a possibly-nil option.
    pub fn attr_opt<V: Into<Value>>(mut self, name: impl Into<Cow<'static, str>>, value: Option<V>) -> Self {
        self.set(name, value.map(Into::into));
        self
    }

    pub fn data(self, key: &str, value: impl Into<Value>) -> Self {
        self.attr(prefixed("data-", key), value)
    }

    pub fn aria(self, key: &str, value: impl Into<Value>) -> Self {
        self.attr(prefixed("aria-", key), value)
    }

    /// `aria: { hidden: "true" }`, the most common option in Matchbox's views.
    pub fn aria_hidden(self) -> Self {
        self.aria("hidden", "true")
    }

    pub fn class(self, value: impl Into<Value>) -> Self {
        self.attr("class", value)
    }
    pub fn id(self, value: impl Into<Value>) -> Self {
        self.attr("id", value)
    }
    pub fn style(self, value: impl Into<Value>) -> Self {
        self.attr("style", value)
    }
    pub fn title(self, value: impl Into<Value>) -> Self {
        self.attr("title", value)
    }
    pub fn alt(self, value: impl Into<Value>) -> Self {
        self.attr("alt", value)
    }
    pub fn role(self, value: impl Into<Value>) -> Self {
        self.attr("role", value)
    }
    pub fn name(self, value: impl Into<Value>) -> Self {
        self.attr("name", value)
    }
    pub fn type_(self, value: impl Into<Value>) -> Self {
        self.attr("type", value)
    }
    pub fn value(self, value: impl Into<Value>) -> Self {
        self.attr("value", value)
    }
    pub fn target(self, value: impl Into<Value>) -> Self {
        self.attr("target", value)
    }
    pub fn placeholder(self, value: impl Into<Value>) -> Self {
        self.attr("placeholder", value)
    }
    pub fn autocomplete(self, value: impl Into<Value>) -> Self {
        self.attr("autocomplete", value)
    }
    pub fn accept(self, value: impl Into<Value>) -> Self {
        self.attr("accept", value)
    }
    pub fn loading(self, value: impl Into<Value>) -> Self {
        self.attr("loading", value)
    }
    pub fn tabindex(self, value: impl Into<Value>) -> Self {
        self.attr("tabindex", value)
    }
    pub fn maxlength(self, value: impl Into<Value>) -> Self {
        self.attr("maxlength", value)
    }
    pub fn rows(self, value: impl Into<Value>) -> Self {
        self.attr("rows", value)
    }
    /// `image_tag`'s `size:` option, expanded into width and height by [`super::image_tag`].
    pub fn size(self, value: impl Into<Value>) -> Self {
        self.attr("size", value)
    }
    pub fn method(self, value: impl Into<Value>) -> Self {
        self.attr("method", value)
    }

    pub fn hidden(self) -> Self {
        self.attr("hidden", true)
    }
    pub fn required(self, value: bool) -> Self {
        self.attr("required", value)
    }
    pub fn autofocus(self) -> Self {
        self.attr("autofocus", true)
    }
    pub fn readonly(self) -> Self {
        self.attr("readonly", true)
    }
    pub fn disabled(self, value: bool) -> Self {
        self.attr("disabled", value)
    }
    pub fn checked(self, value: bool) -> Self {
        self.attr("checked", value)
    }

    pub fn set(&mut self, name: impl Into<Cow<'static, str>>, value: Option<Value>) {
        let name = name.into();
        match self.0.iter_mut().find(|(key, _)| *key == name) {
            Some((_, slot)) => *slot = value,
            None => self.0.push((name, value)),
        }
    }

    /// `options[name] ||= value`: sets only when absent or nil.
    pub fn set_default(&mut self, name: &'static str, value: Option<Value>) {
        if self.get(name).is_none() {
            self.set(name, value);
        }
    }

    /// `options.fetch(name) { value }`: sets only when the key is absent (a nil stays nil).
    pub fn fetch_or_set(&mut self, name: &'static str, value: Option<Value>) {
        if !self.has(name) {
            self.set(name, value);
        }
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.iter().find(|(key, _)| key == name).and_then(|(_, value)| value.as_ref())
    }

    pub fn get_str(&self, name: &str) -> Option<String> {
        self.get(name).map(value_to_string)
    }

    pub fn has(&self, name: &str) -> bool {
        self.0.iter().any(|(key, _)| key == name)
    }

    pub fn remove(&mut self, name: &str) -> Option<Value> {
        let index = self.0.iter().position(|(key, _)| key == name)?;
        self.0.remove(index).1
    }

    /// Appends `other`'s entries, overriding in place like `Hash#merge!`.
    pub fn merge(mut self, other: Attrs) -> Self {
        for (name, value) in other.0 {
            self.set(name, value);
        }
        self
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// A borrowed copy, with room for a few more entries.
    pub fn view(&self) -> AttrsView<'_> {
        let mut view = AttrsView::with_capacity(self.0.len() + 4);
        view.0.extend(self.0.iter().map(|(name, value)| (&**name, value.as_ref().map(Value::as_attr))));
        view
    }
}

/// An [`Attrs`] with borrowed names and values, for a helper's own additions to the options it
/// was given. It has the same hash semantics: setting a key replaces its value in place.
#[derive(Debug, Default)]
pub struct AttrsView<'a>(Vec<(&'a str, Option<AttrValue<'a>>)>);

impl<'a> AttrsView<'a> {
    pub fn with_capacity(capacity: usize) -> Self {
        AttrsView(Vec::with_capacity(capacity))
    }

    pub fn attr(mut self, name: &'a str, value: impl Into<AttrValue<'a>>) -> Self {
        self.set(name, Some(value.into()));
        self
    }

    pub fn attr_opt(mut self, name: &'a str, value: Option<&'a str>) -> Self {
        self.set(name, value.map(AttrValue::Text));
        self
    }

    pub fn set(&mut self, name: &'a str, value: Option<AttrValue<'a>>) {
        match self.0.iter_mut().find(|(key, _)| *key == name) {
            Some((_, slot)) => *slot = value,
            None => self.0.push((name, value)),
        }
    }

    pub fn remove(&mut self, name: &str) -> Option<AttrValue<'a>> {
        let index = self.0.iter().position(|(key, _)| *key == name)?;
        self.0.remove(index).1
    }

    /// Appends `other`'s entries, overriding in place like `Hash#merge!`.
    pub fn merge(mut self, other: &'a Attrs) -> Self {
        for (name, value) in &other.0 {
            self.set(name, value.as_ref().map(Value::as_attr));
        }
        self
    }

    /// `link_to(url, **attributes, data: defaults.merge(attributes.delete(:data)))`: the default
    /// data attributes go where the caller's first `data-*` attribute is (or at the end), ahead of
    /// the caller's own data attributes, which override them.
    pub fn with_default_data(self, defaults: &[(&'a str, AttrValue<'a>)]) -> Self {
        let position = self.0.iter().position(|(key, _)| key.starts_with("data-")).unwrap_or(self.0.len());
        let mut entries = self.0;
        let tail = entries.split_off(position);
        let mut before = AttrsView(entries);
        let mut data = AttrsView(defaults.iter().map(|&(key, value)| (key, Some(value))).collect());
        let mut after = AttrsView::default();
        for (key, value) in tail {
            if key.starts_with("data-") { data.set(key, value) } else { after.set(key, value) }
        }
        for (key, value) in data.0.into_iter().chain(after.0) {
            before.set(key, value);
        }
        before
    }
}

/// Attributes a tag can render: an [`Attrs`] or an [`AttrsView`].
pub trait RenderAttrs {
    /// `tag_options`: the attributes, each with a leading space, appended to `out`.
    fn render_into(&self, out: &mut String);

    /// About how many bytes [`Self::render_into`] appends, to size the tag's buffer.
    fn len_hint(&self) -> usize;
}

impl RenderAttrs for Attrs {
    fn render_into(&self, out: &mut String) {
        for (name, value) in &self.0 {
            if let Some(value) = value {
                render_attr(out, name, value.as_attr());
            }
        }
    }

    fn len_hint(&self) -> usize {
        let value_len = |value: &Option<Value>| value.as_ref().map_or(0, |value| value.as_attr().as_str().len());
        self.0.iter().map(|(name, value)| name.len() + value_len(value) + 4).sum()
    }
}

impl RenderAttrs for AttrsView<'_> {
    fn render_into(&self, out: &mut String) {
        for &(name, value) in &self.0 {
            if let Some(value) = value {
                render_attr(out, name, value);
            }
        }
    }

    fn len_hint(&self) -> usize {
        self.0.iter().map(|(name, value)| name.len() + value.map_or(0, |value| value.as_str().len()) + 4).sum()
    }
}

impl<T: RenderAttrs + ?Sized> RenderAttrs for &T {
    fn render_into(&self, out: &mut String) {
        (**self).render_into(out);
    }

    fn len_hint(&self) -> usize {
        (**self).len_hint()
    }
}

/// One attribute of `tag_options`, with Rails' value rules.
fn render_attr(out: &mut String, name: &str, value: AttrValue<'_>) {
    match value {
        AttrValue::Bool(flag) if BOOLEAN_ATTRIBUTES.contains(&name) => {
            if flag {
                push_attr(out, name, name);
            }
        }
        AttrValue::Bool(_) => push_attr(out, name, value.as_str()),
        AttrValue::Text(text) => {
            push_attr_start(out, name);
            push_escaped(out, text);
            out.push('"');
        }
        AttrValue::Safe(html) => {
            push_attr_start(out, name);
            for (index, part) in html.split('"').enumerate() {
                if index > 0 {
                    out.push_str("&quot;");
                }
                out.push_str(part);
            }
            out.push('"');
        }
    }
}

/// ` name="`.
fn push_attr_start(out: &mut String, name: &str) {
    out.push(' ');
    out.push_str(name);
    out.push_str("=\"");
}

/// ` name="value"`, with `value` as is.
fn push_attr(out: &mut String, name: &str, value: &str) {
    push_attr_start(out, name);
    out.push_str(value);
    out.push('"');
}

pub(super) fn value_to_string(value: &Value) -> String {
    value.as_attr().as_str().to_string()
}

/// `String#dasherize`.
pub fn dasherize(key: &str) -> String {
    key.replace('_', "-")
}

/// `prefix` and the dasherized key: `data: { turbo_frame: ... }` is `data-turbo-frame`.
fn prefixed(prefix: &str, key: &str) -> String {
    let mut name = String::with_capacity(prefix.len() + key.len());
    name.push_str(prefix);
    name.extend(key.chars().map(|c| if c == '_' { '-' } else { c }));
    name
}

/// A guess at a filter block's length, which is only known once it has rendered.
pub(super) const BLOCK_LEN_GUESS: usize = 512;

/// A buffer for a `name` tag with `attrs` and `content_len` bytes of content.
pub(super) fn tag_buffer(name: &str, attrs: &impl RenderAttrs, content_len: usize) -> String {
    String::with_capacity(2 * name.len() + attrs.len_hint() + content_len + 8)
}

/// `<name attributes` (the tag left open).
pub(super) fn open_tag(out: &mut String, name: &str, attrs: &impl RenderAttrs) {
    out.push('<');
    out.push_str(name);
    attrs.render_into(out);
}

/// `content_tag`'s opening tag. A textarea's content starts on a new line.
pub(super) fn open_content_tag(out: &mut String, name: &str, attrs: &impl RenderAttrs) {
    open_tag(out, name, attrs);
    out.push('>');
    if name == "textarea" {
        out.push('\n');
    }
}

pub(super) fn close_tag(out: &mut String, name: &str) {
    out.push_str("</");
    out.push_str(name);
    out.push('>');
}

/// `content_tag(name, content, options)` with already-safe content.
pub fn content_tag(name: &str, attrs: impl RenderAttrs, content: &str) -> Html {
    let mut out = tag_buffer(name, &attrs, content.len());
    open_content_tag(&mut out, name, &attrs);
    out.push_str(content);
    close_tag(&mut out, name);
    Safe(out)
}

/// `content_tag(name, options) do ... end`: the filter block renders straight into the tag.
pub fn content_tag_block(name: &str, attrs: impl RenderAttrs, content: impl Display) -> askama::Result<Html> {
    let mut out = tag_buffer(name, &attrs, BLOCK_LEN_GUESS);
    open_content_tag(&mut out, name, &attrs);
    write!(out, "{content}")?;
    close_tag(&mut out, name);
    Ok(Safe(out))
}

/// `content_tag` with plain-text content, escaped.
pub fn content_tag_text(name: &str, attrs: impl RenderAttrs, content: &str) -> Html {
    let mut out = tag_buffer(name, &attrs, content.len());
    open_content_tag(&mut out, name, &attrs);
    push_escaped(&mut out, content);
    close_tag(&mut out, name);
    Safe(out)
}

/// `tag.name(**options)` from the tag builder: void elements have no closing tag and no slash,
/// others render empty. Underscores in the name become dashes (`tag.turbo_frame`).
pub fn builder_tag(name: &str, attrs: impl RenderAttrs) -> Html {
    let name = if name.contains('_') { Cow::Owned(dasherize(name)) } else { Cow::Borrowed(name) };
    let mut out = tag_buffer(&name, &attrs, 0);
    open_tag(&mut out, &name, &attrs);
    out.push('>');
    if !VOID_ELEMENTS.contains(&&*name) {
        close_tag(&mut out, &name);
    }
    Safe(out)
}

/// Legacy `tag(:name, options)`, used by `image_tag` and form fields:
/// always self-closing with `" />"`.
pub fn legacy_tag(name: &str, attrs: impl RenderAttrs) -> Html {
    let mut out = tag_buffer(name, &attrs, 0);
    open_tag(&mut out, name, &attrs);
    out.push_str(" />");
    Safe(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(attrs: &impl RenderAttrs) -> String {
        let mut out = String::new();
        attrs.render_into(&mut out);
        out
    }

    #[test]
    fn renders_attributes_in_order_with_rails_value_rules() {
        let attrs = attrs()
            .class("a&b")
            .hidden()
            .required(false)
            .data("turbo_permanent", true)
            .aria("hidden", "true")
            .tabindex(-1)
            .attr("data-action", Html::from(Safe("a->\"b\"".to_string())));
        let expected = r#" class="a&amp;b" hidden="hidden" data-turbo-permanent="true" aria-hidden="true" tabindex="-1" data-action="a->&quot;b&quot;""#;
        assert_eq!(render(&attrs), expected);
        assert_eq!(render(&attrs.view()), expected);
    }

    #[test]
    fn assignment_keeps_position() {
        let mut attrs = attrs().value("x").class("c");
        attrs.set("value", Some("y".into()));
        assert_eq!(render(&attrs), r#" value="y" class="c""#);
        let view = attrs.view().attr("value", "z").attr("id", "i");
        assert_eq!(render(&view), r#" value="z" class="c" id="i""#);
    }

    #[test]
    fn default_data_goes_where_the_callers_data_starts() {
        let attrs = attrs().id("x").data("b", "caller").class("c").data("a", "override");
        let defaults = [("data-a", AttrValue::Text("default")), ("data-z", AttrValue::Text("z"))];
        assert_eq!(render(&attrs.view().with_default_data(&defaults)), r#" id="x" data-a="override" data-z="z" data-b="caller" class="c""#);
    }

    #[test]
    fn tags() {
        assert_eq!(content_tag("textarea", attrs().id("t"), "x").0, "<textarea id=\"t\">\nx</textarea>");
        assert_eq!(content_tag_text("span", attrs(), "<b>").0, "<span>&lt;b&gt;</span>");
        assert_eq!(content_tag_block("p", attrs(), "x").unwrap().0, "<p>x</p>");
        assert_eq!(builder_tag("turbo_frame", attrs().id("f")).0, "<turbo-frame id=\"f\"></turbo-frame>");
        assert_eq!(builder_tag("meta", attrs().name("n")).0, "<meta name=\"n\">");
        assert_eq!(legacy_tag("img", attrs().alt("a")).0, "<img alt=\"a\" />");
    }
}
