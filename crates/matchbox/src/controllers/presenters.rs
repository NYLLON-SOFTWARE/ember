//! Maps database rows into the view models `matchbox_views` renders: what the Rails views read
//! off the records (`message.creator`, `room_display_name`, `message_presentation`, the Jbuilder
//! partials) computed up front.

pub mod accounts;
pub mod attachments;
pub mod page;
pub mod pagination;
pub mod rich_text;
#[cfg(test)]
pub mod test_support;
pub mod view_context;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::LazyLock;

use matchbox_db::{Account, Boost, Connection, Membership, Message, RichText, Room, RoomType, User};
use matchbox_richtext::Presentation;
use matchbox_storage::{Storage, Variation};
use matchbox_views::fragment_cache;
use matchbox_views::messages::json::{BoostJson, BoostMessageJson, IdJson, MessageBodyJson, MessageJson, UserJson};
use matchbox_views::messages::support::RubyNumber;
use matchbox_views::messages::support::json_time;
use matchbox_views::messages::{
    AttachmentPreview, AttachmentView, BoostView, MessageContent, MessageItem, MessageView, RoomKind, SoundImage, SoundView, UserView,
};
use matchbox_views::rooms::{RoomView, room_display_name};
use rails_compat::Secrets;
use regex::Regex;

use crate::active_storage::storage_error;
use crate::app::AppState;

pub use rich_text::DbResolver;

/// `Message::THUMBNAIL_MAX_WIDTH` / `THUMBNAIL_MAX_HEIGHT`.
const THUMBNAIL_MAX_WIDTH: i64 = 1200;
const THUMBNAIL_MAX_HEIGHT: i64 = 800;

pub type Result<T> = std::result::Result<T, matchbox_db::Error>;

/// `String#all_emoji?` (reference/lib/rails_ext/string.rb).
pub fn all_emoji(text: &str) -> bool {
    static ALL_EMOJI: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\A(\p{Emoji_Presentation}|\p{Extended_Pictographic}|\x{FE0F})+\z").unwrap());
    ALL_EMOJI.is_match(text)
}

/// `Time#to_fs(:number)`: `%Y%m%d%H%M%S` in UTC (the app's time zone).
pub fn to_fs_number(time: jiff::Timestamp) -> String {
    time.strftime("%Y%m%d%H%M%S").to_string()
}

/// `record.cache_key_with_version`: `"messages/1-20240601120000000000"`.
pub use matchbox_views::fragment_cache::cache_key_with_version;

/// `user.avatar_token`: `signed_id(purpose: :avatar)`.
pub fn avatar_token(secrets: &Secrets, user_id: i64) -> String {
    rails_compat::signed_id::generate(secrets, "User", user_id, Some("avatar"), None)
}

/// `fresh_user_avatar_path(user)`.
pub fn avatar_path(secrets: &Secrets, user: &User) -> String {
    matchbox_routes::fresh_user_avatar(avatar_token(secrets, user.id), to_fs_number(user.updated_at.jiff()))
}

pub fn room_kind(room_type: RoomType) -> RoomKind {
    match room_type {
        RoomType::Open => RoomKind::Open,
        RoomType::Closed => RoomKind::Closed,
        RoomType::Direct => RoomKind::Direct,
    }
}

pub fn user_view(secrets: &Secrets, user: &User) -> UserView {
    UserView { id: user.id, name: user.name.clone(), title: user.title(), avatar_url: avatar_path(secrets, user) }
}

/// `users/_user.json.jbuilder` (`json.cache! user`).
fn cached_user_json(secrets: &Secrets, base_url: &str, user: &User) -> UserJson {
    let key = |key: &mut String| jbuilder_key(key, "users/_user", ("users", user.id, user.updated_at.jiff()), base_url);
    fragment_cache::try_fetch_value(key, || Ok::<_, std::convert::Infallible>(user_json(secrets, base_url, user)))
        .unwrap_or_else(|never| match never {})
}

/// Jbuilder's `json.cache!` key: `jbuilder/views/<template>:<digest>/<record key>`. The digest
/// is the Rust build's (templates can't change while the process runs). The JSON carries absolute
/// URLs built from the request's `base_url`, which comes from its Host header, so the key does too:
/// Rails' key doesn't, and one request with a forged Host fed its URLs to every bot.
/// `record` is the table, id and `updated_at` of its `cache_key_with_version`.
fn jbuilder_key(key: &mut String, template: &str, record: (&str, i64, jiff::Timestamp), base_url: &str) {
    let (table, id, updated_at) = record;
    key.push_str("jbuilder/views/");
    key.push_str(template);
    key.push(':');
    key.push_str(env!("CARGO_PKG_VERSION"));
    key.push('/');
    fragment_cache::push_cache_key_with_version(key, table, id, updated_at);
    key.push('/');
    key.push_str(base_url);
}

/// `users/_user.json.jbuilder`.
pub fn user_json(secrets: &Secrets, base_url: &str, user: &User) -> UserJson {
    UserJson {
        id: user.id,
        name: user.name.clone(),
        role: user.role.name().to_string(),
        avatar_url: format!("{base_url}{}", avatar_path(secrets, user)),
    }
}

/// Everything a page of messages needs, with the rows it looks up along the way remembered
/// (Rails preloads them with `with_creator`, `with_boosts` and friends).
pub struct Presenter<'a> {
    pub conn: &'a Connection,
    pub secrets: &'a Secrets,
    pub storage: &'a Storage,
    pub rich_text: &'a dyn RichText,
    pub now: jiff::Timestamp,
    /// `Current.request_host`, which opengraph embeds are checked against.
    pub request_host: Option<String>,
    users: RefCell<HashMap<i64, User>>,
    room_names: RefCell<HashMap<i64, (Room, String)>>,
    channel_icons: RefCell<Option<HashMap<i64, String>>>,
}

impl<'a> Presenter<'a> {
    pub fn new(conn: &'a Connection, app: &'a AppState, request_host: Option<String>) -> Self {
        Self {
            conn,
            secrets: &app.secrets,
            storage: &app.storage,
            rich_text: &*app.db.env().rich_text,
            now: app.clock.now(),
            request_host,
            users: RefCell::default(),
            room_names: RefCell::default(),
            channel_icons: RefCell::default(),
        }
    }

    pub fn resolver(&self) -> DbResolver<'_> {
        DbResolver { conn: self.conn, secrets: self.secrets, now: self.now }
    }

    pub fn user(&self, id: i64) -> Result<User> {
        if let Some(user) = self.users.borrow().get(&id) {
            return Ok(user.clone());
        }
        let user = User::find(self.conn, id)?;
        self.users.borrow_mut().insert(id, user.clone());
        Ok(user)
    }

    pub fn user_view(&self, id: i64) -> Result<UserView> {
        Ok(user_view(self.secrets, &self.user(id)?))
    }

    /// `room_display_name(room, for_user:)`.
    pub fn room_display_name(&self, room: &Room, for_user: Option<&User>) -> Result<String> {
        let names: Vec<String> = if room.direct() {
            room.users(self.conn)?
                .into_iter()
                .filter(|user| for_user.is_none_or(|for_user| for_user.id != user.id))
                .map(|user| user.name)
                .collect()
        } else {
            Vec::new()
        };
        Ok(room_display_name(room.name.as_deref(), room.direct(), &names, for_user.map(|u| u.name.as_str())))
    }

    pub fn room_view(&self, room: &Room, for_user: &User) -> Result<RoomView> {
        Ok(RoomView {
            id: room.id,
            kind: room_kind(room.room_type),
            name: room.name.clone(),
            icon: if room.direct() { None } else { self.channel_icon(room.id)? },
            display_name: self.room_display_name(room, Some(for_user))?,
        })
    }

    fn channel_icon(&self, room_id: i64) -> Result<Option<String>> {
        if self.channel_icons.borrow().is_none() {
            let icons = Account::first(self.conn)?.map(|account| account.settings().channel_icons()).unwrap_or_default();
            *self.channel_icons.borrow_mut() = Some(icons);
        }
        Ok(self.channel_icons.borrow().as_ref().and_then(|icons| icons.get(&room_id)).cloned())
    }

    /// `message.room` with `room_display_name(message.room, for_user: nil)`.
    fn room_and_name(&self, room_id: i64) -> Result<(Room, String)> {
        if let Some(entry) = self.room_names.borrow().get(&room_id) {
            return Ok(entry.clone());
        }
        let room = Room::find(self.conn, room_id)?;
        let name = self.room_display_name(&room, None)?;
        self.room_names.borrow_mut().insert(room_id, (room.clone(), name.clone()));
        Ok((room, name))
    }

    pub fn plain_text_body(&self, message: &Message) -> Result<String> {
        message.plain_text_body(self.conn, self.rich_text)
    }

    /// `render @messages`: each message's cached fragment when the current store has its version
    /// (`cache [ message, "presentation-v3" ]` wraps the whole partial, so Rails evaluates none of
    /// it on a hit), else its view.
    pub fn messages(&self, messages: &[Message]) -> Result<Vec<MessageItem>> {
        messages.iter().map(|message| self.message_item(message)).collect()
    }

    /// `render message`, as [`Self::messages`] does it.
    pub fn message_item(&self, message: &Message) -> Result<MessageItem> {
        Ok(match matchbox_views::messages::cached_message_fragment(message.id, message.updated_at.jiff()) {
            Some(html) => MessageItem::Fragment { client_message_id: message.client_message_id.clone(), room_id: message.room_id, html },
            None => MessageItem::View(Box::new(self.message(message)?)),
        })
    }

    /// A message as `messages/_message` shows it.
    pub fn message(&self, message: &Message) -> Result<MessageView> {
        let (_, room_name) = self.room_and_name(message.room_id)?;
        match self.renderable_message(message, &room_name) {
            // `message_tag` rescues whatever its block raises, e.g. `avatar_tag message.creator`
            // for a creator that's gone (nil), and renders `messages/_unrenderable` instead.
            Err(matchbox_db::Error::RecordNotFound(_)) => Ok(MessageView {
                id: message.id,
                client_message_id: message.client_message_id.clone(),
                room_id: message.room_id,
                room_name,
                creator: UserView { id: message.creator_id, name: String::new(), title: String::new(), avatar_url: String::new() },
                created_at: message.created_at.jiff(),
                updated_at: message.updated_at.jiff(),
                all_emoji: false,
                content: MessageContent::Unrenderable,
                boosts: Vec::new(),
            }),
            rendered => rendered,
        }
    }

    fn renderable_message(&self, message: &Message, room_name: &str) -> Result<MessageView> {
        let plain_text = self.plain_text_body(message)?;
        Ok(MessageView {
            id: message.id,
            client_message_id: message.client_message_id.clone(),
            room_id: message.room_id,
            room_name: room_name.to_string(),
            creator: self.user_view(message.creator_id)?,
            created_at: message.created_at.jiff(),
            updated_at: message.updated_at.jiff(),
            all_emoji: all_emoji(&plain_text),
            content: self.content(message, &plain_text)?,
            boosts: self.boosts(message)?,
        })
    }

    /// `message.boosts.ordered`.
    pub fn boosts(&self, message: &Message) -> Result<Vec<BoostView>> {
        Boost::for_message_ordered(self.conn, message.id)?.iter().map(|boost| self.boost(boost)).collect()
    }

    pub fn boost(&self, boost: &Boost) -> Result<BoostView> {
        Ok(BoostView {
            id: boost.id,
            updated_at: boost.updated_at.jiff(),
            message_id: boost.message_id,
            content: boost.content.clone(),
            all_emoji: all_emoji(&boost.content),
            booster: self.user_view(boost.booster_id)?,
        })
    }

    /// `message.content_type`, with what `message_presentation` shows for it.
    fn content(&self, message: &Message, plain_text: &str) -> Result<MessageContent> {
        let body = message.body_html(self.conn)?.unwrap_or_default();
        let resolver = self.resolver();
        let ctx = resolver.render_context(self.request_host.clone());
        // `message_tag` evaluates `message.plain_text_body` first; where that raises, it rescues
        // and renders `messages/_unrenderable`, unless logging the exception raises again (a
        // message that isn't UTF-8): then the page fails (verified against the reference).
        match matchbox_richtext::to_plain_text(&body, &ctx) {
            Err(matchbox_richtext::Error::Unrenderable(error)) => {
                return Err(matchbox_db::Error::other(format!("message_tag's rescue raised logging {error}")));
            }
            Err(_) => return Ok(MessageContent::Unrenderable),
            Ok(_) => {}
        }
        if let Some(attachment) = self.attachment(message)? {
            return Ok(MessageContent::Attachment(attachment));
        }
        if let Some(sound) = matchbox_db::message::sound_in(plain_text) {
            return Ok(MessageContent::Sound(SoundView {
                url: matchbox_assets::asset_path(&sound.asset_path()),
                image: sound.image.map(|image| SoundImage {
                    src: matchbox_assets::image_path(&image.asset_path()),
                    width: image.width,
                    height: image.height,
                }),
                text: sound.text.map(str::to_string),
            }));
        }
        Ok(match matchbox_richtext::present_message(&body, &ctx) {
            Presentation::Html(html) => MessageContent::Text { html },
            Presentation::Unrenderable => MessageContent::Unrenderable,
        })
    }

    /// `message.attachment` as `Messages::AttachmentPresentation` needs it.
    fn attachment(&self, message: &Message) -> Result<Option<AttachmentView>> {
        let blob = matchbox_storage::Blob::attached(self.conn, "Message", message.id, "attachment").map_err(storage_error)?;
        let Some(blob) = blob else { return Ok(None) };
        let verifier = &self.storage.verifier;
        let preview = if blob.content_type() == "image/svg+xml" && blob.byte_size <= 5 * 1024 * 1024 {
            AttachmentPreview::Svg
        } else if blob.is_previewable() || blob.is_variable() {
            if blob.is_video() {
                // `attachment.preview(format: :webp, resize_to_limit: [...])`
                let poster = Variation::new(vec![
                    ("format".into(), matchbox_storage::marshal::Value::Symbol("webp".into())),
                    (
                        "resize_to_limit".into(),
                        matchbox_storage::marshal::Value::Array(vec![
                            matchbox_storage::marshal::Value::Int(THUMBNAIL_MAX_WIDTH),
                            matchbox_storage::marshal::Value::Int(THUMBNAIL_MAX_HEIGHT),
                        ]),
                    ),
                ]);
                AttachmentPreview::Video { poster_url: matchbox_storage::paths::representation_redirect_path(verifier, &blob, &poster) }
            } else {
                AttachmentPreview::Image { thumb_url: self.thumb_path(&blob)? }
            }
        } else {
            AttachmentPreview::File
        };
        Ok(Some(AttachmentView {
            filename: blob.filename.to_string(),
            blob_path: matchbox_storage::paths::blob_redirect_path(verifier, &blob, None),
            download_path: matchbox_storage::paths::blob_redirect_path(verifier, &blob, Some("attachment")),
            preview,
            width: dimension(&blob, "width"),
            height: dimension(&blob, "height"),
        }))
    }

    /// `polymorphic_url(attachment.representation(:thumb), only_path: true)`.
    fn thumb_path(&self, blob: &matchbox_storage::Blob) -> Result<String> {
        let thumb = Variation::resize_to_limit(THUMBNAIL_MAX_WIDTH, THUMBNAIL_MAX_HEIGHT, None);
        let variation = if blob.is_previewable() { thumb } else { self.storage.variation_for(blob, &thumb).map_err(storage_error)? };
        Ok(matchbox_storage::paths::representation_redirect_path(&self.storage.verifier, blob, &variation))
    }

    /// `message.body.to_s`: the stored rich text rendered inside its layout.
    pub fn body_html(&self, message: &Message) -> Result<String> {
        let Some(body) = message.body_html(self.conn)? else { return Ok(String::new()) };
        let resolver = self.resolver();
        let ctx = resolver.render_context(self.request_host.clone());
        Ok(matchbox_richtext::Content::load(&body, &ctx).and_then(|content| content.to_rendered_html_with_layout(&ctx)).unwrap_or_default())
    }

    /// `editable_body(message)` as the editor's `value`.
    pub fn editable_body(&self, message: &Message) -> Result<String> {
        let body = message.body_html(self.conn)?.unwrap_or_default();
        let resolver = self.resolver();
        let ctx = resolver.render_context(self.request_host.clone());
        // An `Err` is where the edit page raises in Rails (a missing attachment, say).
        matchbox_richtext::editable_value(&body, &ctx)
            .map(Option::unwrap_or_default)
            .map_err(|error| matchbox_db::Error::other(format!("editable_body raised: {error}")))
    }

    /// `messages/_message.json.jbuilder` (`json.cache! message`).
    pub fn message_json(&self, message: &Message, base_url: &str) -> Result<MessageJson> {
        let record = ("messages", message.id, message.updated_at.jiff());
        let key = |key: &mut String| jbuilder_key(key, "messages/_message", record, base_url);
        fragment_cache::try_fetch_value(key, || self.render_message_json(message, base_url))
    }

    fn render_message_json(&self, message: &Message, base_url: &str) -> Result<MessageJson> {
        Ok(MessageJson {
            id: message.id,
            created_at: json_time(message.created_at.jiff()),
            body: MessageBodyJson { plain_text: self.plain_text_body(message)?, html: self.body_html(message)? },
            creator: cached_user_json(self.secrets, base_url, &self.user(message.creator_id)?),
            room: IdJson { id: message.room_id },
            url: format!("{base_url}{}", matchbox_routes::room_message(message.room_id, message.id)),
        })
    }

    /// `messages/boosts/_boost.json.jbuilder` (`json.cache! boost`).
    pub fn boost_json(&self, boost: &Boost, message: &Message, base_url: &str) -> Result<BoostJson> {
        let record = ("boosts", boost.id, boost.updated_at.jiff());
        let key = |key: &mut String| jbuilder_key(key, "messages/boosts/_boost", record, base_url);
        fragment_cache::try_fetch_value(key, || self.render_boost_json(boost, message, base_url))
    }

    fn render_boost_json(&self, boost: &Boost, message: &Message, base_url: &str) -> Result<BoostJson> {
        Ok(BoostJson {
            id: boost.id,
            content: boost.content.clone(),
            created_at: json_time(boost.created_at.jiff()),
            booster: cached_user_json(self.secrets, base_url, &self.user(boost.booster_id)?),
            message: BoostMessageJson {
                id: boost.message_id,
                url: format!("{base_url}{}", matchbox_routes::room_message(message.room_id, message.id)),
            },
        })
    }

    /// `users/sidebars/rooms/_shared` locals.
    pub fn sidebar_room(&self, room: &Room) -> Result<matchbox_views::users::SidebarRoom> {
        Ok(matchbox_views::users::SidebarRoom {
            id: room.id,
            param_key: room_kind(room.room_type).param_key().to_string(),
            name: room.name.clone().unwrap_or_default(),
            icon: self.channel_icon(room.id)?,
            unread: false,
        })
    }

    /// `users/sidebars/rooms/_direct` locals for `membership`.
    pub fn sidebar_direct(&self, membership: &Membership) -> Result<matchbox_views::users::SidebarDirect> {
        let room = Room::find(self.conn, membership.room_id)?;
        let users = room.users(self.conn)?;
        let mut members: Vec<User> = users.iter().filter(|u| u.id != membership.user_id).cloned().collect();
        if members.is_empty() {
            members.push(self.user(membership.user_id)?);
        }
        Ok(matchbox_views::users::SidebarDirect {
            room_id: room.id,
            unread: membership.unread(),
            updated_at_epoch: epoch_string(room.updated_at.jiff()),
            members: members.iter().map(|user| self.user_summary(user)).collect(),
            membership_id: membership.id,
            membership_updated_at: membership.updated_at.jiff(),
        })
    }

    pub fn user_summary(&self, user: &User) -> matchbox_views::users::UserSummary {
        user_summary(self.secrets, user)
    }
}

/// A `User` row as the users views see it.
pub fn user_summary(secrets: &Secrets, user: &User) -> matchbox_views::users::UserSummary {
    use matchbox_views::users::{Role, Status};
    matchbox_views::users::UserSummary {
        id: user.id,
        name: user.name.clone(),
        bio: user.bio.clone(),
        email_address: user.email_address.clone(),
        role: match user.role {
            matchbox_db::Role::Member => Role::Member,
            matchbox_db::Role::Administrator => Role::Administrator,
            matchbox_db::Role::Bot => Role::Bot,
        },
        status: match user.status {
            matchbox_db::Status::Active => Status::Active,
            matchbox_db::Status::Deactivated => Status::Deactivated,
            matchbox_db::Status::Banned => Status::Banned,
        },
        avatar_path: avatar_path(secrets, user),
    }
}

/// `to_fs(:epoch)` as a string (milliseconds).
pub fn epoch_string(time: jiff::Timestamp) -> String {
    matchbox_views::messages::support::epoch_ms(time).to_string()
}

/// `attachment.metadata[:width]`: an Integer for images, a Float for videos.
fn dimension(blob: &matchbox_storage::Blob, name: &str) -> Option<RubyNumber> {
    match blob.metadata.get(name)? {
        matchbox_storage::Json::Int(value) => Some(RubyNumber::Int(*value)),
        matchbox_storage::Json::Float(value) => Some(RubyNumber::Float(*value)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_emoji_matches_ruby() {
        assert!(all_emoji("👍"));
        assert!(all_emoji("❤️"));
        assert!(!all_emoji("hi 👍"));
        assert!(!all_emoji(""));
    }

    #[test]
    fn cache_versions_use_usec() {
        let time: jiff::Timestamp = "2024-06-01T12:00:00.000123Z".parse().unwrap();
        assert_eq!(cache_key_with_version("messages", 1, time), "messages/1-20240601120000000123");
        assert_eq!(to_fs_number(time), "20240601120000");
    }

    #[test]
    fn jbuilder_keys_match_their_formatted_version() {
        let time: jiff::Timestamp = "2024-06-01T12:00:00.000123456Z".parse().unwrap();
        let mut key = String::new();
        jbuilder_key(&mut key, "messages/_message", ("messages", 7, time), "https://example.com");
        assert_eq!(
            key,
            format!(
                "jbuilder/views/messages/_message:{}/{}/https://example.com",
                env!("CARGO_PKG_VERSION"),
                cache_key_with_version("messages", 7, time)
            )
        );
    }
}
