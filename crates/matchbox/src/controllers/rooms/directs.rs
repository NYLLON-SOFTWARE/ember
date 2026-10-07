//! `Rooms::DirectsController` (reference/app/controllers/rooms/directs_controller.rb). `index`
//! and `destroy` are RoomsController's (`destroy` with this controller's `set_room` and an
//! `ensure_can_administer` that always passes); `show` is RoomsController's without `set_room`,
//! so `remember_last_room_visited` raises on the nil `@room` ([`show`]).

use matchbox_db::{Account, Membership, Room, User};
use matchbox_kit::{Ctx, Error, Result, StatusCode};
use matchbox_views::rooms::{DirectEditView, DirectsEdit, DirectsNew};

use super::{Scope, destroy_room, existing_user_ids, redirect_to_room, set_room, user_ids_param};
use crate::app::AppCtx;
use crate::concerns::{Before, before_actions, require_current_user};
use crate::controllers::presenters::page::{self, Rendered};
use crate::controllers::presenters::{Presenter, user_view};

/// `show`: the room page, which checks membership. Rails inherits RoomsController#show without
/// setting `@room`, so `remember_last_room_visited` raises (a 500).
pub async fn show(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let id = c.param_str("id").and_then(ruby_compat::integer_cast).ok_or(Error::NotFound)?;
    redirect_to_room(c, id)
}

pub async fn new(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    page::framed_page!(c, StatusCode::OK, |ctx| DirectsNew { ctx }).await
}

pub async fn create(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let user_id = require_current_user(c)?.id;
    // selected_users: `User.where(id: selected_users_ids.including(Current.user.id))`
    let mut ids = user_ids_param(c);
    ids.push(user_id);
    let room = c
        .app()
        .write(move |tx| {
            let users = existing_user_ids(tx.conn(), &ids)?;
            Room::find_or_create_direct_for(tx, &users, user_id)
        })
        .await?;
    broadcast_create_room(c, &room).await?;
    redirect_to_room(c, room.id)
}

pub async fn edit(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let room = set_room(c, Scope::Directs).await?;
    let current_user = require_current_user(c)?.clone();
    let app = c.app().clone();
    let edit = c
        .app()
        .read(move |conn| {
            let presenter = Presenter::new(conn, &app, None);
            // `@room.users.many? ? @room.users.without(Current.user) : @room.users`
            let users = room.users(conn)?;
            let users: Vec<User> =
                if users.len() > 1 { users.into_iter().filter(|user| user.id != current_user.id).collect() } else { users };
            Ok(DirectEditView {
                room_id: room.id,
                display_name: presenter.room_display_name(&room, Some(&current_user))?,
                users: users.iter().map(|user| user_view(&app.secrets, user)).collect(),
            })
        })
        .await?;
    page::framed_page!(c, StatusCode::OK, |ctx| DirectsEdit { ctx, edit: &edit }).await
}

pub async fn destroy(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let room = set_room(c, Scope::Directs).await?;
    // ensure_can_administer: every member of a direct room can.
    destroy_room(c, room).await
}

/// `broadcast_create_room`: `users/sidebars/rooms/_direct` for each membership, to its user.
async fn broadcast_create_room(c: &Ctx, room: &Room) -> Result<()> {
    let (app, room) = (c.app().clone(), room.clone());
    let base_url = page::renderer_base_url(c);
    c.app()
        .read(move |conn| {
            let presenter = Presenter::new(conn, &app, None);
            let account = Account::first(conn)?;
            let mut partials = Rendered::default();
            for membership in Membership::for_room(conn, room.id)? {
                let direct = presenter.sidebar_direct(&membership)?;
                let html =
                    page::render_detached_at(&app, account.as_ref(), &base_url, |ctx| matchbox_views::users::direct_room(ctx, &direct));
                partials.direct_rooms.push((membership.id, html));
            }
            app.broadcasts.direct_room_create(conn, &room, &partials)
        })
        .await
}
