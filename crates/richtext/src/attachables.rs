//! Resolving `<action-text-attachment>` nodes to what they attach, and rendering each attachable's
//! partial, as Action Text, Lexxy and Ember's extensions do.

use rails_compat::encoding;
use regex::Regex;
use ruby_compat::erb::html_escape;
use ruby_compat::strip;
use std::sync::LazyLock;

use crate::Error;
use crate::dom::{Dom, NodeId};
use crate::ruby::{is_blank, presence, truncate};
use crate::uri::{self, UriError};

pub const MENTION_CONTENT_TYPE: &str = "application/vnd.campfire.mention";
pub const OPENGRAPH_EMBED_CONTENT_TYPE: &str = "application/vnd.actiontext.opengraph-embed";
const TWITTER_AVATAR_URL_PREFIX: &str = "https://pbs.twimg.com/profile_images";

/// What the app knows about a user, for rendering `users/_mention.html.erb`. Plain text and
/// mentions only read `id` and `name`; the other fields are only rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionUser {
    pub id: i64,
    pub name: String,
    /// `User#title`: name and bio joined with " – ".
    pub title: String,
    /// `user.attachable_sgid`: a freshly minted SGID for the "attachable" purpose.
    pub attachable_sgid: String,
    /// `user_path(user)`
    pub user_path: String,
    /// `fresh_user_avatar_path(user)`
    pub avatar_path: String,
}

/// A record `GlobalID.find` located.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GidLookup {
    User(MentionUser),
    /// Found, but not a `User`: the invalid-signature fallback ignores it.
    OtherModel,
    /// `GlobalID.find` returned nil or raised `ActiveRecord::RecordNotFound`.
    NotFound,
    /// `GlobalID.find` raised anything else (an unknown model constant, say).
    Raises,
}

/// What a signed GlobalID verified for the "attachable" purpose points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignedLookup {
    /// The signature verified and the user exists.
    User(MentionUser),
    /// The signature verified (`SignedGlobalID.parse` succeeds) but the record is gone.
    MissingRecord { model_name: String },
    /// Bad signature, wrong purpose, expired, or not an SGID at all.
    Invalid,
}

/// The app's access to its records. rails_compat verifies signatures; this crate only needs the
/// two lookups Action Text performs.
pub trait AttachableResolver {
    /// `GlobalID::Locator.locate_signed(sgid, for: "attachable")` (and, when that finds nothing,
    /// whether `SignedGlobalID.parse(sgid, for: "attachable")` still verifies). Ember's only
    /// attachables are users (mentions).
    fn locate_signed(&self, sgid: &str) -> SignedLookup;

    /// `GlobalID.find(gid)` for a `gid://` URI string, with no signature involved.
    fn find_gid(&self, gid: &str) -> GidLookup;
}

/// Everything rendering needs from the request and the app.
pub struct RenderContext<'a> {
    pub resolver: &'a dyn AttachableResolver,
    /// `Current.request_host`
    pub request_host: Option<String>,
}

/// `ActionText::Attachment::OpengraphEmbed` (reference/lib/rails_ext/actiontext_opengraph_embeds.rb)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpengraphEmbed {
    pub href: Option<String>,
    pub url: Option<String>,
    pub filename: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attachable {
    User(MentionUser),
    OpengraphEmbed(OpengraphEmbed),
    /// `ActionText::Attachables::ContentAttachment`
    Content {
        content: String,
    },
    /// `ActionText::Attachables::RemoteImage`
    RemoteImage {
        url: String,
        width: Option<String>,
        height: Option<String>,
    },
    /// Lexxy's `ActionText::Attachables::RemoteVideo`
    RemoteVideo {
        url: String,
        content_type: String,
        width: Option<String>,
        height: Option<String>,
        filename: Option<String>,
    },
    /// `ActionText::Attachables::MissingAttachable`, remembering the model a still-valid SGID named
    Missing {
        signed_model: Option<String>,
    },
}

impl Attachable {
    /// `attachable_content_type`, which only some attachables define.
    pub fn attachable_content_type(&self) -> Result<&str, Error> {
        match self {
            Attachable::User(_) => Ok(MENTION_CONTENT_TYPE),
            Attachable::OpengraphEmbed(_) => Ok(OPENGRAPH_EMBED_CONTENT_TYPE),
            _ => Err(Error::Raised("NoMethodError: attachable_content_type")),
        }
    }
}

/// The attachment node's attributes an attachment reads, plus its resolved attachable.
pub struct Attachment {
    pub attachable: Attachable,
    pub caption: Option<String>,
}

// --- Resolution --------------------------------------------------------------------------------

static OPENGRAPH_CONTENT_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"application/vnd.actiontext.opengraph-embed").unwrap());
static IMAGE_CONTENT_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^image(/.+|$)").unwrap());
static VIDEO_CONTENT_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^video(/.+|$)").unwrap());
static MARSHALED_GID_RE: LazyLock<regex::bytes::Regex> =
    LazyLock::new(|| regex::bytes::Regex::new(r"(?-u)(gid://campfire/[^/]+/[0-9]+)").unwrap());

/// Ember's `ActionText::Attachment.from_node` (reference/lib/rails_ext/action_text_attachables.rb):
/// an opengraph embed, else a User found through a possibly invalid SGID, else Action Text's own
/// lookup (as extended by Lexxy).
pub fn attachment_from_node(dom: &Dom, node: NodeId, ctx: &RenderContext) -> Result<Attachment, Error> {
    let attachable = attachable_from_node(dom, node, ctx)?;
    Ok(Attachment { attachable, caption: presence(dom.attr(node, "caption")).map(str::to_string) })
}

fn attachable_from_node(dom: &Dom, node: NodeId, ctx: &RenderContext) -> Result<Attachable, Error> {
    if let Some(embed) = opengraph_embed_from_node(dom, node, ctx)? {
        return Ok(Attachable::OpengraphEmbed(embed));
    }
    if let Some(user) = attachable_from_possibly_expired_sgid(dom.attr(node, "sgid"), ctx)? {
        return Ok(Attachable::User(user));
    }
    Ok(action_text_attachable_from_node(dom, node, ctx))
}

/// `ActionText::Attachable.from_node`, with Lexxy's RemoteVideo fallback for missing attachables.
/// `Content#attachables` (and so `Message#mentionees`) uses this directly, without Ember's
/// invalid-signature fallback.
pub fn action_text_attachable_from_node(dom: &Dom, node: NodeId, ctx: &RenderContext) -> Attachable {
    let signed = dom.attr(node, "sgid").map(|sgid| ctx.resolver.locate_signed(sgid)).unwrap_or(SignedLookup::Invalid);
    if let SignedLookup::User(user) = signed {
        return Attachable::User(user);
    }
    let content_type = dom.attr(node, "content-type");
    if let Some(content) = dom.attr(node, "content")
        && content_type.is_some_and(|t| t.contains("html"))
        && !is_blank(content)
    {
        return Attachable::Content { content: content.to_string() };
    }
    if let Some(url) = dom.attr(node, "url") {
        if IMAGE_CONTENT_TYPE_RE.is_match(content_type.unwrap_or("")) {
            return Attachable::RemoteImage {
                url: url.to_string(),
                width: dom.attr(node, "width").map(str::to_string),
                height: dom.attr(node, "height").map(str::to_string),
            };
        }
        if VIDEO_CONTENT_TYPE_RE.is_match(content_type.unwrap_or("")) {
            return Attachable::RemoteVideo {
                url: url.to_string(),
                content_type: content_type.unwrap_or("").to_string(),
                width: dom.attr(node, "width").map(str::to_string),
                height: dom.attr(node, "height").map(str::to_string),
                filename: dom.attr(node, "filename").map(str::to_string),
            };
        }
    }
    Attachable::Missing {
        signed_model: match signed {
            SignedLookup::MissingRecord { model_name } => Some(model_name),
            _ => None,
        },
    }
}

/// `attachable_from_possibly_expired_sgid`: reads the GlobalID out of an SGID without checking its
/// signature, and only ever returns a User.
fn attachable_from_possibly_expired_sgid(sgid: Option<&str>, ctx: &RenderContext) -> Result<Option<MentionUser>, Error> {
    let Some(sgid) = sgid else { return Ok(None) };
    // `sgid.split("--").first`: Ruby drops trailing empty fields, so "" and "--" have no first
    let Some(message) = sgid.split("--").collect::<Vec<_>>().into_iter().rev().skip_while(|f| f.is_empty()).last() else {
        return Ok(None);
    };
    let decoded = decode_base64(message)?;
    // Ruby's JSON parser takes the bytes as UTF-8 without validating what's inside strings
    let valid_utf8 = std::str::from_utf8(&decoded).is_ok();
    let json = match crate::ruby::json_parse(&String::from_utf8_lossy(&decoded)) {
        Some(json) => json,
        None if valid_utf8 => return Err(Error::Raised("JSON::ParserError")),
        // The parser error quotes the invalid bytes, and logging that message raises in turn
        None => return Err(Error::Unrenderable("JSON::ParserError")),
    };
    let rails = match &json {
        serde_json::Value::Object(map) => map.get("_rails"),
        _ => return Err(Error::Raised("NoMethodError: dig")),
    };
    let rails = match rails {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Object(map)) => Some(map),
        Some(_) => return Err(Error::Raised("TypeError: dig")),
    };
    let truthy =
        |v: Option<&serde_json::Value>| v.filter(|v| !matches!(v, serde_json::Value::Null | serde_json::Value::Bool(false))).cloned();
    let gid: Option<String> = if let Some(data) = truthy(rails.and_then(|r| r.get("data"))) {
        // GlobalID.find of anything but a string finds nothing
        data.as_str().map(str::to_string)
    } else if let Some(message) = truthy(rails.and_then(|r| r.get("message"))) {
        // Rails 7 Marshal-dumped the GID. The signature isn't verified, so the dump can't be
        // safely loaded; the GID is matched out of its bytes instead.
        let serde_json::Value::String(message) = message else { return Err(Error::Raised("NoMethodError: unpack1")) };
        let bytes = decode_base64(&message)?;
        MARSHALED_GID_RE.find(&bytes).map(|m| String::from_utf8_lossy(m.as_bytes()).into_owned())
    } else {
        None
    };
    let Some(gid) = gid else { return Ok(None) };
    match ctx.resolver.find_gid(&gid) {
        GidLookup::User(user) => Ok(Some(user)),
        GidLookup::OtherModel | GidLookup::NotFound => Ok(None),
        GidLookup::Raises => Err(Error::Raised("GlobalID.find")),
    }
}

/// `Base64.strict_decode64(message) rescue Base64.urlsafe_decode64(message)`
fn decode_base64(message: &str) -> Result<Vec<u8>, Error> {
    encoding::strict_decode(message).or_else(|| encoding::urlsafe_decode(message)).ok_or(Error::Raised("ArgumentError: invalid base64"))
}

// --- Opengraph embeds --------------------------------------------------------------------------

/// `ActionText::Attachment::OpengraphEmbed.from_node`
pub fn opengraph_embed_from_node(dom: &Dom, node: NodeId, ctx: &RenderContext) -> Result<Option<OpengraphEmbed>, Error> {
    let Some(content_type) = dom.attr(node, "content-type") else { return Ok(None) };
    if !OPENGRAPH_CONTENT_TYPE_RE.is_match(content_type) {
        return Ok(None);
    }
    let host = ctx.request_host.as_deref().unwrap_or("");
    let embed = if presence(dom.attr(node, "filename")).is_some() {
        OpengraphEmbed {
            href: web_url(dom.attr(node, "href"), host)?,
            url: web_url(dom.attr(node, "url"), host)?,
            filename: dom.attr(node, "filename").map(str::to_string),
            description: dom.attr(node, "caption").map(str::to_string),
        }
    } else {
        embed_from_content(dom.attr(node, "content").unwrap_or(""), host)?
    };
    Ok(Some(embed))
}

/// `attributes_from_content`: the details Lexxy serializes as the embed's content markup.
fn embed_from_content(content: &str, host: &str) -> Result<OpengraphEmbed, Error> {
    // Rails parses this with Nokogiri::HTML (libxml2's HTML4 parser); html5ever agrees with it
    // on the markup the embed partial and Lexxy produce.
    let mut dom = Dom::new();
    let fragment = dom.parse_fragment(content).map_err(Error::Parse)?;
    let all = dom.descendants(fragment);
    let with_class = |class: &str| all.iter().copied().find(|&n| has_class(&dom, n, class));
    let title = with_class("og-embed__title");
    let link = title.and_then(|t| dom.descendants(t).into_iter().find(|&n| dom.local_name(n) == Some("a")));
    let image = all
        .iter()
        .copied()
        .find(|&n| dom.local_name(n) == Some("img") && dom.ancestors(n).iter().any(|&a| has_class(&dom, a, "og-embed__image")));
    let description = with_class("og-embed__description");
    Ok(OpengraphEmbed {
        href: web_url(link.and_then(|l| dom.attr(l, "href")), host)?,
        url: web_url(image.and_then(|i| dom.attr(i, "src")), host)?,
        filename: link.or(title).map(|n| strip(&dom.text_content(n)).to_string()),
        description: description.map(|n| strip(&dom.text_content(n)).to_string()),
    })
}

fn has_class(dom: &Dom, node: NodeId, class: &str) -> bool {
    dom.attr(node, "class").is_some_and(|c| c.split([' ', '\t', '\n', '\r']).any(|token| token == class))
}

/// `web_url`: an absolute http(s) URL on a named host other than this Ember's.
pub fn web_url(value: Option<&str>, request_host: &str) -> Result<Option<String>, Error> {
    let Some(value) = value.filter(|v| !is_blank(v)) else { return Ok(None) };
    match uri::parse(value) {
        Err(UriError::InvalidUri) => Ok(None),
        Err(UriError::InvalidComponent) => Err(Error::Raised("URI::InvalidComponentError")),
        Ok(parsed) => {
            if parsed.is_http() && elsewhere(parsed.host.as_deref(), request_host)? {
                Ok(Some(value.to_string()))
            } else {
                Ok(None)
            }
        }
    }
}

fn elsewhere(host: Option<&str>, request_host: &str) -> Result<bool, Error> {
    let Some(host) = host else { return Ok(false) };
    if !named_host(host)? {
        return Ok(false);
    }
    Ok(canonical_host(host) != canonical_host(request_host))
}

fn named_host(host: &str) -> Result<bool, Error> {
    if is_blank(host) || host.contains('%') || !host.contains('.') {
        return Ok(false);
    }
    // `host.split(".").last`: Ruby drops trailing empty labels, and nil.match? raises
    let trimmed = host.trim_end_matches('.');
    if trimmed.is_empty() {
        return Err(Error::Raised("NoMethodError: match?"));
    }
    let label = trimmed.rsplit('.').next().unwrap_or("");
    Ok(label.chars().any(|c| c.is_ascii_alphabetic()) && !label.to_ascii_lowercase().starts_with("0x"))
}

fn canonical_host(host: &str) -> String {
    let lower = host.to_lowercase();
    lower.strip_suffix('.').map(str::to_string).unwrap_or(lower)
}

impl OpengraphEmbed {
    pub fn twitter_avatar(&self) -> bool {
        self.url.as_deref().unwrap_or("").starts_with(TWITTER_AVATAR_URL_PREFIX)
    }
}

// --- Partials ----------------------------------------------------------------------------------

/// `render_action_text_attachment(attachment)`: the attachable's partial, chomped. `render_content`
/// renders a nested content attachment's own content (`ContentAttachment#to_html`).
pub fn render_attachment(attachment: &Attachment, render_content: &dyn Fn(&str) -> Result<String, Error>) -> Result<String, Error> {
    let html = match &attachment.attachable {
        Attachable::User(user) => render_mention(user),
        Attachable::OpengraphEmbed(embed) => render_opengraph_embed(embed),
        // Rails asks the SGID's model for its missing partial, which only models that include
        // ActionText::Attachable as a concern have. User doesn't, so a mention of a deleted user
        // raised and blanked the whole message; every missing attachable is Action Text's ☒ here.
        Attachable::Missing { .. } => "☒".to_string(),
        Attachable::Content { content } => {
            format!("<figure class=\"attachment attachment--content\">\n  {}\n</figure>\n", render_content(content)?)
        }
        Attachable::RemoteImage { url, width, height } => {
            let mut html = String::from("<figure class=\"attachment attachment--preview\">\n  ");
            html.push_str(&image_tag(url, width.as_deref(), height.as_deref())?);
            html.push('\n');
            if let Some(caption) = &attachment.caption {
                html.push_str(&format!(
                    "    <figcaption class=\"attachment__caption\">\n      {}\n    </figcaption>\n",
                    html_escape(caption)
                ));
            }
            html.push_str("</figure>\n");
            html
        }
        Attachable::RemoteVideo { url, content_type, width, height, .. } => {
            let mut html =
                String::from("<figure class=\"attachment attachment--preview attachment--video\">\n  <video controls=\"controls\"");
            for (name, value) in [("width", width), ("height", height)] {
                if let Some(v) = value {
                    html.push_str(&format!(" {name}=\"{}\"", html_escape(v)));
                }
            }
            html.push_str(&format!(">\n    <source src=\"{}\" type=\"{}\">\n</video>", html_escape(url), html_escape(content_type)));
            if let Some(caption) = &attachment.caption {
                html.push_str(&format!(
                    "    <figcaption class=\"attachment__caption\">\n      {}\n    </figcaption>\n",
                    html_escape(caption)
                ));
            }
            html.push_str("</figure>\n");
            html
        }
    };
    Ok(crate::ruby::chomp(&html).to_string())
}

/// reference/app/views/users/_mention.html.erb, with `avatar_tag` (users/avatars_helper.rb).
pub fn render_mention(user: &MentionUser) -> String {
    format!(
        "<span class=\"mention\" sgid=\"{}\"><a title=\"{}\" class=\"btn avatar\" data-turbo-frame=\"_top\" href=\"{}\"><img aria-hidden=\"true\" src=\"{}\" width=\"48\" height=\"48\" /></a> {}</span>\n",
        html_escape(&user.attachable_sgid),
        html_escape(&user.title),
        html_escape(&user.user_path),
        html_escape(&user.avatar_path),
        html_escape(&user.name),
    )
}

/// reference/app/views/action_text/attachables/_opengraph_embed.html.erb
pub fn render_opengraph_embed(embed: &OpengraphEmbed) -> String {
    let title = match (&embed.href, &embed.filename) {
        (Some(href), filename) => {
            let text = match filename {
                Some(f) => html_escape(&truncate(f, 280, "…")),
                None => html_escape(href),
            };
            format!("<a rel=\"noreferrer\" target=\"_blank\" href=\"{}\">{}</a>", html_escape(href), text)
        }
        (None, Some(f)) => html_escape(&truncate(f, 280, "…")),
        (None, None) => String::new(),
    };
    let mut html = format!(
        "<figure class=\"attachment attachment--content attachment--og\">\n  <actiontext-opengraph-embed>\n    <div class=\"og-embed gap {}\">\n      <div class=\"og-embed__content\">\n        <div class=\"og-embed__title\">\n          {}\n        </div>\n        <div class=\"og-embed__description\">{}</div>\n      </div>\n",
        if embed.twitter_avatar() { "og-embed--twitter-avatar" } else { "" },
        title,
        html_escape(&truncate(embed.description.as_deref().unwrap_or(""), 560, "…")),
    );
    if let Some(url) = &embed.url {
        html.push_str(&format!(
            "        <div class=\"og-embed__image\">\n          <img src=\"{}\" class=\"image center\" alt=\"\">\n        </div>\n",
            html_escape(url)
        ));
    }
    html.push_str("    </div>\n  </actiontext-opengraph-embed>\n</figure>\n");
    html
}

static ASSET_URI_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?mi)^[-a-z]+://|^(?:cid|data):|^//").unwrap());

/// `image_tag(url, width:, height:)` for a remote image. Sources that aren't URLs go through the
/// asset pipeline, which raises for anything it doesn't know; a rooted path passes through.
fn image_tag(url: &str, width: Option<&str>, height: Option<&str>) -> Result<String, Error> {
    let src = if is_blank(url) {
        String::new()
    } else if ASSET_URI_RE.is_match(url) || url.starts_with('/') {
        url.to_string()
    } else {
        return Err(Error::Raised("Propshaft::MissingAssetError"));
    };
    let mut html = String::from("<img");
    for (name, value) in [("width", width), ("height", height)] {
        if let Some(v) = value {
            html.push_str(&format!(" {name}=\"{}\"", html_escape(v)));
        }
    }
    html.push_str(&format!(" src=\"{}\" />", html_escape(&src)));
    Ok(html)
}

/// `Attachment#to_plain_text`
pub fn attachment_plain_text(attachment: &Attachment) -> PlainTextRepresentation {
    let caption = attachment.caption.clone();
    match &attachment.attachable {
        Attachable::User(user) => PlainTextRepresentation::Html(format!("@{}", user.name)),
        Attachable::OpengraphEmbed(_) => PlainTextRepresentation::Html(String::new()),
        Attachable::Content { content } => PlainTextRepresentation::Content(content.clone()),
        Attachable::RemoteImage { .. } => PlainTextRepresentation::Html(format!("[{}]", caption.unwrap_or_else(|| "Image".into()))),
        Attachable::RemoteVideo { filename, .. } => {
            PlainTextRepresentation::Html(format!("[{}]", caption.or_else(|| filename.clone()).unwrap_or_else(|| "Video".into())))
        }
        Attachable::Missing { .. } => PlainTextRepresentation::Html(caption.unwrap_or_default()),
    }
}

/// A plain-text representation replaces the attachment node: strings are parsed as markup in the
/// node's parent, while a content attachment's fragment is moved in as is.
pub enum PlainTextRepresentation {
    Html(String),
    Content(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `GlobalID.find` over the reference's records the vectors name: users 1 and 2 exist, and the
    /// default locator ignores the GID's app.
    struct VectorRecords;

    impl AttachableResolver for VectorRecords {
        fn locate_signed(&self, _sgid: &str) -> SignedLookup {
            SignedLookup::Invalid
        }

        fn find_gid(&self, gid: &str) -> GidLookup {
            let Some((_app, path)) = gid.strip_prefix("gid://").and_then(|rest| rest.split('?').next()?.split_once('/')) else {
                return GidLookup::NotFound;
            };
            match path.strip_prefix("User/") {
                Some(id @ ("1" | "2")) => GidLookup::User(MentionUser {
                    id: id.parse().unwrap(),
                    name: String::new(),
                    title: String::new(),
                    attachable_sgid: String::new(),
                    user_path: String::new(),
                    avatar_path: String::new(),
                }),
                Some(_) => GidLookup::NotFound,
                None => GidLookup::OtherModel,
            }
        }
    }

    /// The reference's answers for SGIDs whose signature doesn't verify (`unverified_sgids` in
    /// `vectors/rails_compat.json`, from `reference-tools/rails_compat_vectors.rb`).
    #[test]
    fn possibly_expired_sgids_find_users_like_rails() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../vectors/rails_compat.json");
        let vectors: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).expect(path)).unwrap();
        let ctx = RenderContext { resolver: &VectorRecords, request_host: None };
        for case in vectors["unverified_sgids"].as_array().unwrap() {
            let label = &case["case"];
            let found = attachable_from_possibly_expired_sgid(case["sgid"].as_str(), &ctx).map(|user| user.map(|user| user.id));
            match &case["expected"] {
                serde_json::Value::Null => assert_eq!(found, Ok(None), "{label}"),
                serde_json::Value::String(gid) => {
                    let id = gid.rsplit('/').next().unwrap().parse().unwrap();
                    assert_eq!(found, Ok(Some(id)), "{label}");
                }
                raises => assert!(found.is_err(), "{label} should raise like Rails ({raises})"),
            }
        }
    }
}
