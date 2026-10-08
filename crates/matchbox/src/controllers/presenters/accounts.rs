//! Maps database rows into the view models `matchbox_views` renders for the account, session and
//! user screens (sessions, first run, users, accounts, autocompletable, pwa): what the Rails views
//! read off `@user`, `Current.account` and friends, computed up front. The `ViewContext` builder
//! for every page is [`super::view_context::Layout`].
//!
//! Queries the db crate doesn't have yet are written here against the reference's SQL.

#[cfg(test)]
mod tests;

use matchbox_db::{
    Account, AccountSettings, CachedStatements, Connection, Membership, PushSubscription, Room, RoomType, User, placeholders, query_all,
};
use matchbox_kit::Ctx;
use matchbox_views::Platform;
use matchbox_views::accounts::{Bot, BotForm, BotRoom, HelpContact};
use matchbox_views::users::{
    MentionUser, ProfileMembership, PushSubscription as PushSubscriptionView, SidebarDirect, SidebarDirectItem, SidebarRoom, UserSummary,
};
use rails_compat::Secrets;
use rails_compat::global_id::{self, GlobalId};
use rusqlite::{OptionalExtension, params};

use super::{attachments, epoch_string, to_fs_number, user_summary};

/// `User::Transferable::TRANSFER_LINK_EXPIRY_DURATION`
pub const TRANSFER_LINK_EXPIRY: jiff::SignedDuration = jiff::SignedDuration::from_hours(4);

/// `user.transfer_id`: `signed_id(purpose: :transfer, expires_in: 4.hours)`.
pub fn transfer_id(secrets: &Secrets, user_id: i64, now: jiff::Timestamp) -> String {
    rails_compat::signed_id::generate(secrets, "User", user_id, Some("transfer"), Some(now + TRANSFER_LINK_EXPIRY))
}

/// `User.find_by_transfer_id(id)`: the user id, if the signature and expiry check out.
pub fn user_id_from_transfer_id(secrets: &Secrets, transfer_id: &str, now: jiff::Timestamp) -> Option<i64> {
    rails_compat::signed_id::verify(secrets, "User", transfer_id, Some("transfer"), now)
}

/// `User.from_avatar_token(sid)`'s verification half (`find_signed!(sid, purpose: :avatar)`).
pub fn user_id_from_avatar_token(secrets: &Secrets, token: &str, now: jiff::Timestamp) -> Option<i64> {
    rails_compat::signed_id::verify(secrets, "User", token, Some("avatar"), now)
}

/// `user.attachable_sgid`.
pub fn attachable_sgid(secrets: &Secrets, user_id: i64) -> String {
    global_id::attachable_sgid(secrets, &GlobalId::new("User", user_id))
}

/// `fresh_account_logo_path(size:)`: `v` is `Current.account&.updated_at&.to_fs(:number)`.
pub fn fresh_account_logo_path(account: Option<&Account>, size: Option<&str>) -> String {
    let v = account.map(|account| to_fs_number(account.updated_at.jiff()));
    matchbox_routes::fresh_account_logo(v.as_deref(), size)
}

/// `platform` as the views see it (`ApplicationPlatform.new(request.user_agent)`).
pub fn platform(c: &Ctx) -> Platform {
    crate::concerns::platform(c).to_view()
}

/// `User.administrator.first`, for `accounts/_help_contact`.
pub fn help_contact(conn: &Connection) -> matchbox_db::Result<Option<HelpContact>> {
    let owner: Option<(String, Option<String>)> = conn
        .query_row_cached(
            r#"SELECT "users"."name", "users"."email_address" FROM "users" WHERE "users"."role" = 1 ORDER BY "users"."id" ASC LIMIT 1"#,
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(owner.map(|(name, email_address)| HelpContact { name, email_address: email_address.unwrap_or_default() }))
}

/// `User.none?`
pub fn no_users(conn: &Connection) -> matchbox_db::Result<bool> {
    Ok(User::count(conn)? == 0)
}

/// `record.touch`: bumps `updated_at` (what `belongs_to :record, touch: true` does to an
/// attachment's record, and `Blob#touch_attachments` after analysis).
pub fn touch(conn: &Connection, table: &str, id: i64, now: matchbox_db::Timestamp) -> matchbox_db::Result<()> {
    conn.execute_cached(&format!(r#"UPDATE "{table}" SET "updated_at" = ? WHERE "{table}"."id" = ?"#), params![now, id])?;
    Ok(())
}

// --- Users -----------------------------------------------------------------------------------------

/// A user for `users/_mention` and the autocompletable views.
pub fn mention_user(secrets: &Secrets, user: &User) -> MentionUser {
    MentionUser { user: user_summary(secrets, user), attachable_sgid: attachable_sgid(secrets, user.id) }
}

/// `room_display_name(room, for_user:)`: a direct room is named after its other members.
pub fn room_display_name(conn: &Connection, room: &Room, for_user: &User) -> matchbox_db::Result<String> {
    let names: Vec<String> = if room.direct() {
        room.users(conn)?.into_iter().filter(|user| user.id != for_user.id).map(|user| user.name).collect()
    } else {
        Vec::new()
    };
    Ok(matchbox_views::rooms::room_display_name(room.name.as_deref(), room.direct(), &names, Some(&for_user.name)))
}

/// `Room.model_name.param_key` for the room's STI class.
pub fn room_param_key(room_type: RoomType) -> &'static str {
    match room_type {
        RoomType::Open => "rooms_open",
        RoomType::Closed => "rooms_closed",
        RoomType::Direct => "rooms_direct",
    }
}

/// Users::ProfilesController#show: `Current.user.memberships.with_ordered_room.partition { |m| m.room.direct? }`,
/// returned as `(direct, shared)`.
pub fn profile_memberships(conn: &Connection, user: &User) -> matchbox_db::Result<(Vec<ProfileMembership>, Vec<ProfileMembership>)> {
    let mut direct = Vec::new();
    let mut shared = Vec::new();
    for (membership, room) in Membership::with_ordered_room(conn, user.id)? {
        let view = ProfileMembership {
            room_id: room.id,
            room_param_key: room_param_key(room.room_type).to_string(),
            room_display_name: room_display_name(conn, &room, user)?,
            involvement: membership.involvement.map(|involvement| involvement.name()).unwrap_or_default().to_string(),
            direct: room.direct(),
        };
        if room.direct() { direct.push(view) } else { shared.push(view) }
    }
    Ok((direct, shared))
}

// --- Sidebar ---------------------------------------------------------------------------------------

/// `Users::SidebarsController::DIRECT_PLACEHOLDERS`
pub const DIRECT_PLACEHOLDERS: i64 = 20;

#[derive(Debug, Clone)]
pub struct Sidebar {
    pub direct_memberships: Vec<SidebarDirectItem>,
    pub other_memberships: Vec<SidebarRoom>,
    pub direct_placeholder_users: Vec<UserSummary>,
}

/// Users::SidebarsController#show.
pub fn sidebar(conn: &Connection, secrets: &Secrets, user: &User, settings: Option<&AccountSettings>) -> matchbox_db::Result<Sidebar> {
    let all_memberships = Membership::visible_with_ordered_room(conn, user.id)?;

    // `select { direct }.sort_by { |m| m.room.updated_at }.reverse`
    let mut direct: Vec<&(Membership, Room)> = all_memberships.iter().filter(|(_, room)| room.direct()).collect();
    direct.sort_by_key(|(_, room)| room.updated_at);
    direct.reverse();

    let direct_memberships = direct
        .iter()
        .map(|(membership, room)| {
            // `cache membership` wraps the whole partial: on a hit Rails loads none of its members.
            Ok(match matchbox_views::users::cached_direct_room_fragment(membership.id, membership.updated_at.jiff()) {
                Some(html) => SidebarDirectItem::Fragment(html),
                None => SidebarDirectItem::View(sidebar_direct(conn, secrets, membership, room)?),
            })
        })
        .collect::<matchbox_db::Result<_>>()?;
    let other_memberships = all_memberships
        .iter()
        .filter(|(membership, _)| !direct.iter().any(|(direct, _)| direct.id == membership.id))
        .map(|(membership, room)| SidebarRoom {
            id: room.id,
            param_key: room_param_key(room.room_type).to_string(),
            name: room.name.clone().unwrap_or_default(),
            icon: settings.and_then(|settings| settings.channel_icon(room.id)).map(str::to_owned),
            unread: membership.unread(),
        })
        .collect();

    Ok(Sidebar { direct_memberships, other_memberships, direct_placeholder_users: direct_placeholder_users(conn, secrets, user)? })
}

/// `users/sidebars/rooms/_direct` locals: `room.users.without(membership.user).presence || [ membership.user ]`.
pub fn sidebar_direct(conn: &Connection, secrets: &Secrets, membership: &Membership, room: &Room) -> matchbox_db::Result<SidebarDirect> {
    let users = room.users(conn)?;
    let mut members: Vec<&User> = users.iter().filter(|user| user.id != membership.user_id).collect();
    let own;
    if members.is_empty() {
        own = User::find(conn, membership.user_id)?;
        members.push(&own);
    }
    Ok(SidebarDirect {
        room_id: room.id,
        unread: membership.unread(),
        updated_at_epoch: epoch_string(room.updated_at.jiff()),
        members: members.into_iter().map(|user| user_summary(secrets, user)).collect(),
        membership_id: membership.id,
        membership_updated_at: membership.updated_at.jiff(),
    })
}

/// `find_direct_placeholder_users`. `exclude_user_ids` is `Membership.where(room_id: directs).pluck(:user_id).uniq`
/// `.including(Current.user.id)`: `including` appends even when the id is already there, and the
/// limit counts that duplicate.
fn direct_placeholder_users(conn: &Connection, secrets: &Secrets, user: &User) -> matchbox_db::Result<Vec<UserSummary>> {
    let direct_room_ids: Vec<i64> = Room::for_user_of_type(conn, user.id, RoomType::Direct)?.iter().map(|room| room.id).collect();
    let mut exclude_user_ids: Vec<i64> = Vec::new();
    if !direct_room_ids.is_empty() {
        let sql = format!(
            r#"SELECT "memberships"."user_id" FROM "memberships" WHERE "memberships"."room_id" IN ({})"#,
            placeholders(direct_room_ids.len())
        );
        let ids: Vec<i64> = query_all(conn, &sql, rusqlite::params_from_iter(&direct_room_ids), |row| row.get(0))?;
        for id in ids {
            if !exclude_user_ids.contains(&id) {
                exclude_user_ids.push(id);
            }
        }
    }
    exclude_user_ids.push(user.id);

    // The limit goes in the SQL: under ORDER BY, SQLite recompiles a statement with a bound LIMIT
    // every time it runs.
    let limit = (DIRECT_PLACEHOLDERS - exclude_user_ids.len() as i64).max(0);
    let sql = format!(
        r#"{} WHERE "users"."status" = 0 AND "users"."id" NOT IN ({}) ORDER BY "users"."created_at" ASC LIMIT {limit}"#,
        User::SELECT,
        placeholders(exclude_user_ids.len())
    );
    let users = User::find_by_sql(conn, &sql, rusqlite::params_from_iter(&exclude_user_ids))?;
    Ok(users.iter().map(|user| user_summary(secrets, user)).collect())
}

// --- Account ---------------------------------------------------------------------------------------

/// AccountsController#account_users: `User.where(status: [ :active, :banned ])` for
/// administrators, `User.active` otherwise; `.ordered.without_bots`.
pub fn account_users(conn: &Connection, can_administer: bool) -> matchbox_db::Result<Vec<User>> {
    let status = if can_administer { r#""users"."status" IN (0, 2)"# } else { r#""users"."status" = 0"# };
    let sql = format!(r#"{} WHERE {status} AND "users"."role" != 2 ORDER BY LOWER(name)"#, User::SELECT);
    User::find_by_sql(conn, &sql, [])
}

/// A bot row for `accounts/bots/_bot`: its key and `bot.rooms.without_directs.ordered`.
pub fn bot(conn: &Connection, secrets: &Secrets, bot: &User) -> matchbox_db::Result<Bot> {
    let mut rooms = Room::for_user_without_directs(conn, bot.id)?;
    sort_by_lower_name(&mut rooms, |room| room.name.as_deref().unwrap_or(""));
    Ok(Bot {
        user: user_summary(secrets, bot),
        bot_key: bot.bot_key(),
        rooms: rooms.into_iter().map(|room| BotRoom { id: room.id, name: room.name.unwrap_or_default() }).collect(),
    })
}

/// The fields `accounts/bots/_form` fills in: `image_tag bot.avatar` is the blob's absolute
/// redirect URL.
pub fn bot_form(conn: &Connection, storage: &matchbox_storage::Storage, base_url: &str, bot: &User) -> matchbox_db::Result<BotForm> {
    let avatar = attachments::attached_blob(conn, "User", bot.id, "avatar")?;
    Ok(BotForm {
        name: Some(bot.name.clone()),
        webhook_url: bot.webhook_url(conn)?,
        avatar_attachment_url: avatar
            .map(|blob| format!("{base_url}{}", matchbox_storage::paths::blob_redirect_path(&storage.verifier, &blob, None))),
    })
}

/// A push subscription with its user agent parsed like `UserAgent.parse(push_subscription.user_agent)`.
pub fn push_subscription(subscription: &PushSubscription) -> PushSubscriptionView {
    let agent = crate::concerns::user_agent::parse(subscription.user_agent.as_deref().unwrap_or(""));
    PushSubscriptionView {
        id: subscription.id,
        endpoint: subscription.endpoint.clone().unwrap_or_default(),
        browser: agent.browser(),
        version: agent.version().to_string(),
        platform: agent.platform().unwrap_or_default(),
    }
}

// --- Helpers ---------------------------------------------------------------------------------------

/// `ORDER BY LOWER(name)`: SQLite lowercases ASCII only.
pub fn sort_by_lower_name<T>(items: &mut [T], name: impl Fn(&T) -> &str) {
    items.sort_by_cached_key(|item| name(item).to_ascii_lowercase());
}

/// A permitted string attribute: `Some` when the key was given (its value may be nil).
pub fn string_attribute(params: &matchbox_kit::ParamMap, key: &str) -> Option<Option<String>> {
    if !params.contains_key(key) {
        return None;
    }
    Some(params.get(key).and_then(|param| param.to_s()))
}
