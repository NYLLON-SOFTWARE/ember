//! `Users::SidebarsController` (reference/app/controllers/users/sidebars_controller.rb): the room
//! list, loaded into the `user_sidebar` turbo frame.

use matchbox_db::Account;
use matchbox_kit::{Ctx, Error, Param, Result, StatusCode, format};
use matchbox_views::users;

use crate::app::AppCtx;
use crate::channels::user_gid;
use crate::concerns::{self, Before};
use crate::controllers::presenters::{self, view_context};

pub async fn show(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    c.respond_to(&[&format::HTML])?;
    let user = concerns::require_current_user(c)?.clone();
    let secrets = c.app().secrets.clone();
    let (mut sidebar, channel_order, favorite_channels) = {
        let (user, secrets, fragments) = (user.clone(), secrets.clone(), c.app().fragment_cache.clone());
        c.app()
            .read(move |conn| {
                // The direct rooms' fragments come from the store the render then uses.
                let settings = Account::first(conn)?.map(|account| account.settings());
                let sidebar = matchbox_views::fragment_cache::with(&fragments, || {
                    presenters::accounts::sidebar(conn, &secrets, &user, settings.as_ref())
                })?;
                let visible: std::collections::HashSet<_> = sidebar.other_memberships.iter().map(|room| room.id).collect();
                let defaults: Vec<_> = settings
                    .as_ref()
                    .map(|settings| settings.default_room_order())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|id| visible.contains(id))
                    .collect();
                let favorites = settings.map(|settings| settings.favorite_channels(user.id)).unwrap_or_default();
                Ok((sidebar, defaults, favorites))
            })
            .await?
    };
    // Personal favorites come first; every other room follows the shared workspace order.
    let positions: std::collections::HashMap<_, _> = channel_order.iter().enumerate().map(|(index, id)| (*id, index)).collect();
    let favorites: std::collections::HashMap<_, _> = favorite_channels.iter().enumerate().map(|(index, id)| (*id, index)).collect();
    sidebar.other_memberships.sort_by_key(|room| {
        (favorites.get(&room.id).copied().unwrap_or(usize::MAX), positions.get(&room.id).copied().unwrap_or(usize::MAX))
    });

    let data = SidebarData {
        current_user: presenters::user_summary(&secrets, &user),
        // turbo_stream_from :rooms / turbo_stream_from Current.user, :rooms
        rooms_stream: rails_compat::turbo::signed_stream_name(&secrets, &["rooms"]),
        user_rooms_stream: rails_compat::turbo::signed_stream_name(&secrets, &[&user_gid(user.id).to_param(), "rooms"]),
        sidebar,
        favorite_channels: serde_json::to_string(&favorite_channels).expect("favorite channels are JSON"),
        channel_order: serde_json::to_string(&channel_order).expect("channel order is JSON"),
    };
    view_context::page_or_frame(
        c,
        StatusCode::OK,
        |ctx| matchbox_views::render_sized!(data.page(ctx)),
        |ctx| {
            let page = data.page(ctx);
            matchbox_views::layouts::frame(ctx, page.as_head(), page.as_content())
        },
    )
    .await
}

/// A personal starred-room preference; the URL and the authenticated user, never a submitted
/// user ID, determine whose order changes. Normal before-actions retain authentication/CSRF.
pub async fn update_order(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    let user_id = concerns::require_current_user(c)?.id;
    let order = parse_order(c.params.get("room_ids"))?;
    if !c.app().write(move |tx| Account::set_favorite_order(tx, user_id, &order)).await? {
        return Err(Error::Status(StatusCode::FORBIDDEN));
    }
    Ok(c.head(StatusCode::NO_CONTENT))
}

pub async fn update_default_order(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    concerns::ensure_can_administer(c)?;
    let user_id = concerns::require_current_user(c)?.id;
    let order = parse_order(c.params.get("room_ids"))?;
    if !c.app().write(move |tx| Account::set_default_room_order(tx, user_id, &order)).await? {
        return Err(Error::Status(StatusCode::FORBIDDEN));
    }
    c.app().broadcasts.room_order_changed();
    Ok(c.head(StatusCode::NO_CONTENT))
}

fn parse_order(value: Option<&Param>) -> Result<Vec<i64>> {
    let values = value.and_then(Param::as_array).ok_or_else(|| Error::BadRequest("room_ids must be an array".into()))?;
    if values.len() > matchbox_db::models::account::MAX_CHANNEL_ORDER {
        return Err(Error::BadRequest("too many rooms".into()));
    }
    let mut seen = std::collections::HashSet::new();
    values
        .iter()
        .map(|value| {
            let id = match value {
                Param::Number(number) => number.as_i64(),
                Param::Str(string) => string.parse::<i64>().ok(),
                _ => None,
            }
            .filter(|id| *id > 0 && seen.insert(*id));
            id.ok_or_else(|| Error::BadRequest("room IDs must be unique positive integers".into()))
        })
        .collect()
}

struct SidebarData {
    current_user: matchbox_views::users::UserSummary,
    rooms_stream: String,
    user_rooms_stream: String,
    sidebar: presenters::accounts::Sidebar,
    favorite_channels: String,
    channel_order: String,
}

impl SidebarData {
    fn page<'a>(&self, ctx: &'a matchbox_views::ViewContext<'a>) -> users::SidebarShow<'a> {
        users::SidebarShow {
            ctx,
            current_user: self.current_user.clone(),
            rooms_stream: self.rooms_stream.clone(),
            user_rooms_stream: self.user_rooms_stream.clone(),
            direct_memberships: self.sidebar.direct_memberships.clone(),
            direct_placeholder_users: self.sidebar.direct_placeholder_users.clone(),
            other_memberships: self.sidebar.other_memberships.clone(),
            favorite_channels: self.favorite_channels.clone(),
            channel_order: self.channel_order.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_order_requires_a_bounded_list_of_unique_positive_ids() {
        assert_eq!(parse_order(Some(&Param::from_json(serde_json::json!([2, 1, "3"])))).unwrap(), vec![2, 1, 3]);
        assert!(parse_order(Some(&Param::from_json(serde_json::json!([])))).unwrap().is_empty());
        for value in [
            serde_json::json!([1, 1]),
            serde_json::json!([0]),
            serde_json::json!([-1]),
            serde_json::json!([1.5]),
            serde_json::json!([true]),
            serde_json::json!({}),
        ] {
            assert!(parse_order(Some(&Param::from_json(value))).is_err());
        }
        assert!(parse_order(None).is_err());
        assert!(parse_order(Some(&Param::Array(vec![Param::from("1"); 4097]))).is_err());
    }
}
