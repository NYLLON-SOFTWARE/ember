//! Views for `reference/app/views/users`.

use askama::Template;

use crate::ViewContext;
use crate::accounts::HelpContact;
use crate::helpers::{self as h, filters};
use crate::layouts::Page;

mod summary;
pub use summary::*;

/// `users/new.html.erb` (the join page).
#[derive(Template)]
#[template(path = "users/new.html", blocks = ["head", "content"])]
pub struct New<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub join_code: String,
    pub help_contact: Option<HelpContact>,
}

impl Page for New<'_> {
    fn page_title(&self) -> Option<String> {
        Some("Sign up".into())
    }
    fn body_class(&self) -> Option<&str> {
        Some("signup")
    }
}

/// `users/show.html.erb`.
#[derive(Template)]
#[template(path = "users/show.html", blocks = ["head", "nav", "content"])]
pub struct Show<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub user: UserSummary,
    /// `user.transfer_id`, for `users/profiles/_transfer` (shown to administrators).
    pub transfer_id: String,
}

impl Page for Show<'_> {
    fn page_title(&self) -> Option<String> {
        Some(self.user.name.clone())
    }
}

/// `users/_ban_button.html.erb` on its own.
#[derive(Template)]
#[template(path = "users/_ban_button.html")]
pub struct BanButton<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub user: UserSummary,
}

/// A user as `users/_mention` and the autocompletable views see it.
#[derive(Clone, Debug, Default)]
pub struct MentionUser {
    pub user: UserSummary,
    /// `user.attachable_sgid`.
    pub attachable_sgid: String,
}

impl std::ops::Deref for MentionUser {
    type Target = UserSummary;
    fn deref(&self) -> &UserSummary {
        &self.user
    }
}

/// `users/_mention.html.erb`: the mention attachment's HTML.
#[derive(Template)]
#[template(path = "users/_mention.html")]
pub struct Mention<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub user: MentionUser,
}

/// `users/autocompletables/_template.html.erb`.
#[derive(Template)]
#[template(path = "users/autocompletables/_template.html")]
pub struct AutocompletableTemplate<'a> {
    pub ctx: &'a ViewContext<'a>,
}

/// `users/avatars/show.svg.erb`: the initials avatar for users without an uploaded one.
#[derive(Template)]
#[template(path = "users/avatars/show.svg")]
pub struct AvatarSvg {
    pub user_id: i64,
    /// `User#initials`.
    pub initials: String,
}

/// A membership row on the profile (`users/profiles/_membership`).
#[derive(Clone, Debug)]
pub struct ProfileMembership {
    pub room_id: i64,
    /// "rooms_open", "rooms_closed" or "rooms_direct".
    pub room_param_key: String,
    /// `room_display_name(membership.room)`.
    pub room_display_name: String,
    pub involvement: String,
    pub direct: bool,
}

impl ProfileMembership {
    pub fn involvement_room(&self) -> h::InvolvementRoom<'_> {
        h::InvolvementRoom { id: self.room_id, param_key: &self.room_param_key, direct: self.direct }
    }
}

/// `users/profiles/show.html.erb`.
#[derive(Template)]
#[template(path = "users/profiles/show.html", blocks = ["head", "content"])]
pub struct ProfileShow<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub user: UserSummary,
    pub avatar_attached: bool,
    pub transfer_id: String,
    pub shared_memberships: Vec<ProfileMembership>,
    pub direct_memberships: Vec<ProfileMembership>,
}

impl<'a> ProfileShow<'a> {
    /// `profile_form_with(@user, **params)`.
    fn profile_form(&self) -> h::FormWith {
        h::form_with(h::routes::user_profile()).model("user").method("patch").data("controller", "form")
    }
}

impl Page for ProfileShow<'_> {
    fn page_title(&self) -> Option<String> {
        Some(self.user.name.clone())
    }
}

/// `users/profiles/_transfer.html.erb` on its own.
#[derive(Template)]
#[template(path = "users/profiles/_transfer.html")]
pub struct Transfer<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub user: UserSummary,
    pub transfer_id: String,
}

/// A `Push::Subscription`, with its user agent parsed (`UserAgent.parse`).
#[derive(Clone, Debug)]
pub struct PushSubscription {
    pub id: i64,
    pub endpoint: String,
    pub browser: String,
    pub version: String,
    pub platform: String,
}

/// `users/push_subscriptions/index.html.erb`.
#[derive(Template)]
#[template(path = "users/push_subscriptions/index.html", blocks = ["head", "content"])]
pub struct PushSubscriptionsIndex<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub push_subscriptions: Vec<PushSubscription>,
}

impl Page for PushSubscriptionsIndex<'_> {
    fn page_title(&self) -> Option<String> {
        Some("Push notification subscriptions".into())
    }
}

/// A direct room in the sidebar (`users/sidebars/rooms/_direct`).
#[derive(Clone, Debug)]
pub struct SidebarDirect {
    pub room_id: i64,
    pub unread: bool,
    /// `room.updated_at.to_fs(:epoch)`.
    pub updated_at_epoch: String,
    /// `room.users.without(membership.user).presence || [ membership.user ]`, in that order.
    pub members: Vec<UserSummary>,
    /// The membership's id and `updated_at`: the partial is `cache membership`.
    pub membership_id: i64,
    pub membership_updated_at: jiff::Timestamp,
}

/// A direct room on its way into the sidebar: the `users/sidebars/rooms/_direct` fragment when
/// the cache already holds this membership version (`cache membership` wraps the whole partial,
/// so Rails evaluates none of it then), else the view to render it from.
#[derive(Clone, Debug)]
pub enum SidebarDirectItem {
    Fragment(crate::fragment_cache::Fragment),
    View(SidebarDirect),
}

impl From<SidebarDirect> for SidebarDirectItem {
    fn from(direct: SidebarDirect) -> Self {
        SidebarDirectItem::View(direct)
    }
}

/// `users/sidebars/rooms/_direct` for `membership`, whose body is `cache membership` (and which
/// `users/sidebars/show` renders with `cached: true`): the first rendering of a membership
/// version is what later renders reuse.
pub fn direct_room(ctx: &ViewContext, membership: &SidebarDirect) -> crate::fragment_cache::Fragment {
    crate::fragment_cache::fetch(
        |key| direct_room_fragment_key(key, membership.membership_id, membership.membership_updated_at),
        || SidebarDirectPartial { ctx, membership: membership.clone() }.render().expect("users/sidebars/rooms/_direct renders"),
    )
}

/// [`direct_room`] where a template renders the partial.
pub fn cached_direct_room(ctx: &ViewContext, item: &SidebarDirectItem) -> askama::filters::Safe<crate::fragment_cache::Fragment> {
    askama::filters::Safe(match item {
        SidebarDirectItem::Fragment(html) => html.clone(),
        SidebarDirectItem::View(membership) => direct_room(ctx, membership),
    })
}

/// The `users/sidebars/rooms/_direct` fragment for this membership version, if the current store
/// holds it.
pub fn cached_direct_room_fragment(membership_id: i64, updated_at: jiff::Timestamp) -> Option<crate::fragment_cache::Fragment> {
    crate::fragment_cache::read(|key| direct_room_fragment_key(key, membership_id, updated_at))
}

fn direct_room_fragment_key(key: &mut String, membership_id: i64, updated_at: jiff::Timestamp) {
    crate::fragment_cache::push_record_fragment_key(
        key,
        "users/sidebars/rooms/_direct",
        direct_room_digest(),
        "memberships",
        membership_id,
        updated_at,
    );
}

fn direct_room_digest() -> &'static str {
    static DIGEST: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| crate::fragment_cache::digest(&[include_str!("../templates/users/sidebars/rooms/_direct.html")]));
    &DIGEST
}

impl SidebarDirect {
    fn class_names(&self) -> &'static str {
        if self.unread { "direct unread" } else { "direct" }
    }
}

/// A shared room in the sidebar (`users/sidebars/rooms/_shared`).
#[derive(Clone, Debug, serde::Deserialize)]
pub struct SidebarRoom {
    pub id: i64,
    /// "rooms_open" or "rooms_closed".
    pub param_key: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub unread: bool,
}

impl SidebarRoom {
    fn class_names(&self) -> &'static str {
        if self.unread { "align-center gap room btn txt-nowrap unread" } else { "align-center gap room btn txt-nowrap" }
    }
}

/// `users/sidebars/show.html.erb`.
#[derive(Template)]
#[template(path = "users/sidebars/show.html", blocks = ["head", "content"])]
pub struct SidebarShow<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub current_user: UserSummary,
    /// `Turbo::StreamsChannel.signed_stream_name(:rooms)`.
    pub rooms_stream: String,
    /// `Turbo::StreamsChannel.signed_stream_name([ Current.user, :rooms ])`.
    pub user_rooms_stream: String,
    pub direct_memberships: Vec<SidebarDirectItem>,
    pub direct_placeholder_users: Vec<UserSummary>,
    pub other_memberships: Vec<SidebarRoom>,
    /// `Current.user.administrator? || !Current.account.settings.restrict_room_creation_to_administrators?`.
    pub can_create_rooms: bool,
    /// JSON array of shared room IDs in the current user's custom ordering (empty: alphabetic).
    pub channel_order: String,
}

impl Page for SidebarShow<'_> {}

/// `users/sidebars/rooms/_direct.html.erb` on its own (broadcast when a direct room appears).
#[derive(Template)]
#[template(path = "users/sidebars/rooms/_direct.html")]
pub struct SidebarDirectPartial<'a> {
    pub ctx: &'a ViewContext<'a>,
    pub membership: SidebarDirect,
}

/// `users/sidebars/rooms/_shared.html.erb` on its own (broadcast and rendered by rooms controllers).
#[derive(Template)]
#[template(path = "users/sidebars/rooms/_shared.html")]
pub struct SidebarSharedPartial {
    pub room: SidebarRoom,
}
