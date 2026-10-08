//! Path helpers mirroring `reference/config/routes.rb`. Names match the Rails `*_path` helpers
//! with the `_path` suffix dropped. Shared by controllers (redirects) and views (links).

use std::fmt::Display;

macro_rules! path {
    ($name:ident, $lit:literal) => {
        pub fn $name() -> String { $lit.to_string() }
    };
    ($name:ident($($arg:ident),+), $fmt:literal) => {
        pub fn $name($($arg: impl Display),+) -> String { format!($fmt) }
    };
}

path!(root, "/");
path!(first_run, "/first_run");

path!(session, "/session");
path!(new_session, "/session/new");
path!(session_transfer(id), "/session/transfers/{id}");

path!(account, "/account");
path!(edit_account, "/account/edit");
path!(account_users, "/account/users");
path!(account_user(id), "/account/users/{id}");
path!(edit_account_user(id), "/account/users/{id}/edit");
path!(account_bots, "/account/bots");
path!(new_account_bot, "/account/bots/new");
path!(account_bot(id), "/account/bots/{id}");
path!(edit_account_bot(id), "/account/bots/{id}/edit");
path!(account_bot_key(bot_id), "/account/bots/{bot_id}/key");
path!(account_join_code, "/account/join_code");
path!(account_logo, "/account/logo");
path!(account_custom_styles, "/account/custom_styles");
path!(edit_account_custom_styles, "/account/custom_styles/edit");

path!(join(join_code), "/join/{join_code}");
path!(qr_code(id), "/qr_code/{id}");

path!(user(id), "/users/{id}");
path!(user_avatar(user_id), "/users/{user_id}/avatar");
path!(user_ban(user_id), "/users/{user_id}/ban");
path!(user_sidebar, "/users/me/sidebar");
path!(user_sidebar_order, "/users/me/sidebar/order");
path!(user_profile, "/users/me/profile");
path!(edit_user_profile, "/users/me/profile/edit");
path!(user_push_subscriptions, "/users/me/push_subscriptions");
path!(user_push_subscription(id), "/users/me/push_subscriptions/{id}");
path!(
    user_push_subscription_test_notifications(push_subscription_id),
    "/users/me/push_subscriptions/{push_subscription_id}/test_notifications"
);

path!(autocompletable_users, "/autocompletable/users");

path!(rooms, "/rooms");
path!(new_room, "/rooms/new");
path!(room(id), "/rooms/{id}");
path!(edit_room(id), "/rooms/{id}/edit");
path!(room_messages(room_id), "/rooms/{room_id}/messages");
path!(new_room_message(room_id), "/rooms/{room_id}/messages/new");
path!(room_message(room_id, id), "/rooms/{room_id}/messages/{id}");
path!(edit_room_message(room_id, id), "/rooms/{room_id}/messages/{id}/edit");
path!(room_bot_messages(room_id, bot_key), "/rooms/{room_id}/{bot_key}/messages");
path!(room_bot_message(room_id, bot_key, id), "/rooms/{room_id}/{bot_key}/messages/{id}");
path!(room_bot_message_boosts(room_id, bot_key, message_id), "/rooms/{room_id}/{bot_key}/messages/{message_id}/boosts");
path!(room_bot_message_boost(room_id, bot_key, message_id, id), "/rooms/{room_id}/{bot_key}/messages/{message_id}/boosts/{id}");
path!(room_refresh(room_id), "/rooms/{room_id}/refresh");
path!(room_settings(room_id), "/rooms/{room_id}/settings");
path!(room_involvement(room_id), "/rooms/{room_id}/involvement");
path!(room_at_message(room_id, message_id), "/rooms/{room_id}/@{message_id}");

path!(rooms_opens, "/rooms/opens");
path!(new_rooms_open, "/rooms/opens/new");
path!(rooms_open(id), "/rooms/opens/{id}");
path!(edit_rooms_open(id), "/rooms/opens/{id}/edit");
path!(rooms_closeds, "/rooms/closeds");
path!(new_rooms_closed, "/rooms/closeds/new");
path!(rooms_closed(id), "/rooms/closeds/{id}");
path!(edit_rooms_closed(id), "/rooms/closeds/{id}/edit");
path!(rooms_directs, "/rooms/directs");
path!(new_rooms_direct, "/rooms/directs/new");
path!(rooms_direct(id), "/rooms/directs/{id}");
path!(edit_rooms_direct(id), "/rooms/directs/{id}/edit");

path!(messages, "/messages");
path!(message(id), "/messages/{id}");
path!(edit_message(id), "/messages/{id}/edit");
path!(message_boosts(message_id), "/messages/{message_id}/boosts");
path!(new_message_boost(message_id), "/messages/{message_id}/boosts/new");
path!(message_boost(message_id, id), "/messages/{message_id}/boosts/{id}");

path!(searches, "/searches");
path!(clear_searches, "/searches/clear");
path!(unfurl_link, "/unfurl_link");
path!(webmanifest, "/webmanifest");
path!(service_worker, "/service-worker");
path!(rails_health_check, "/up");

/// `direct :fresh_user_avatar` — cache-busting avatar URL keyed by the signed avatar token.
pub fn fresh_user_avatar(avatar_token: impl Display, updated_at_number: impl Display) -> String {
    format!("/users/{avatar_token}/avatar?v={updated_at_number}")
}

/// `direct :fresh_account_logo` — `v` is the account's `updated_at.to_fs(:number)`, when present.
pub fn fresh_account_logo(v: Option<&str>, size: Option<&str>) -> String {
    let mut query = Vec::new();
    if let Some(size) = size {
        query.push(format!("size={size}"));
    }
    if let Some(v) = v {
        query.push(format!("v={v}"));
    }
    if query.is_empty() { account_logo() } else { format!("{}?{}", account_logo(), query.join("&")) }
}
