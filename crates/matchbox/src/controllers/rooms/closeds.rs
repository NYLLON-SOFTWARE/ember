//! `Rooms::ClosedsController` (reference/app/controllers/rooms/closeds_controller.rb). `index` is
//! RoomsController's; `destroy` is RoomsController's without `set_room`
//! (`super::destroy_without_room`).

use matchbox_db::{Room, RoomType, User};
use matchbox_kit::{Ctx, Result, StatusCode};
use matchbox_views::rooms::{ClosedFormView, ClosedsEdit, ClosedsNew, FormRoom};

use super::{
    Scope, ensure_can_administer, ensure_permission_to_create_rooms, existing_user_ids, redirect_to_room, render_shared_room, room_icon,
    room_icon_param, room_name_param, room_saved, set_room, user_ids_param,
};
use crate::app::AppCtx;
use crate::concerns::{self, Before, before_actions, require_current_user};
use crate::controllers::presenters::page;
use crate::controllers::presenters::user_view;

/// `DEFAULT_ROOM_NAME`
const DEFAULT_ROOM_NAME: &str = "New room";

pub async fn show(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let room = set_room(c, Scope::WithoutDirects).await?;
    concerns::remember_last_room_visited(c, room.id);
    redirect_to_room(c, room.id)
}

pub async fn new(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    ensure_permission_to_create_rooms(c).await?;
    let current_user_id = require_current_user(c)?.id;
    // `@users = User.active.ordered`; the form shows them all as unselected.
    let form = ClosedFormView {
        room: FormRoom { id: None, name: Some(DEFAULT_ROOM_NAME.into()), icon: None },
        can_administer: true,
        current_user_id,
        selected_users: Vec::new(),
        unselected_users: super::opens::active_users(c).await?,
    };
    page::framed_page!(c, StatusCode::OK, |ctx| ClosedsNew { ctx, form: &form }).await
}

pub async fn create(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    ensure_permission_to_create_rooms(c).await?;
    let name = room_name_param(c)?.flatten();
    let icon = room_icon_param(c)?;
    let user_id = require_current_user(c)?.id;
    let grantee_ids = user_ids_param(c);
    // Rooms::Closed.create_for(room_params, users: grantees)
    let room = c
        .app()
        .write(move |tx| {
            let grantees = existing_user_ids(tx.conn(), &grantee_ids)?;
            let mut room = Room::create_for(tx, RoomType::Closed, name.as_deref(), user_id, &grantees)?;
            if let Some(icon) = icon {
                room.set_icon(tx, icon.as_deref())?;
            }
            Ok(room)
        })
        .await?;
    broadcast_to_members(c, &room, false).await?;
    room_saved(c, room.id)
}

pub async fn edit(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let mut room = set_room(c, Scope::WithoutDirects).await?;
    room.room_type = RoomType::Closed; // force_room_type
    let current_user = require_current_user(c)?.clone();
    let secrets = c.app().secrets.clone();
    let room_id = room.id;
    let (selected_users, unselected_users) = c
        .app()
        .read_offloaded(move |conn| {
            let selected_ids = Room::find(conn, room_id)?.user_ids(conn)?;
            let (selected, unselected): (Vec<User>, Vec<User>) =
                User::active_ordered(conn)?.into_iter().partition(|user| selected_ids.contains(&user.id));
            let views = |users: Vec<User>| users.iter().map(|user| user_view(&secrets, user)).collect::<Vec<_>>();
            Ok((views(selected), views(unselected)))
        })
        .await?;
    let form = ClosedFormView {
        room: FormRoom { id: Some(room.id), name: room.name.clone(), icon: room_icon(c, room.id).await? },
        can_administer: current_user.can_administer(Some(room.creator_id), false),
        current_user_id: current_user.id,
        selected_users,
        unselected_users,
    };
    page::framed_page!(c, StatusCode::OK, |ctx| ClosedsEdit { ctx, form: &form }).await
}

pub async fn update(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let room = set_room(c, Scope::WithoutDirects).await?;
    ensure_can_administer(c, &room)?;
    let name = room_name_param(c)?;
    let icon = room_icon_param(c)?;
    let grantee_ids = user_ids_param(c);
    // force_room_type, then `@room.update! room_params`
    let room = c
        .app()
        .write(move |tx| {
            let mut room = room;
            room.update(tx, name.as_ref().map(|name| name.as_deref()), Some(RoomType::Closed))?;
            if let Some(icon) = icon {
                room.set_icon(tx, icon.as_deref())?;
            }
            Ok(room)
        })
        .await?;
    // `@room.memberships.revise(granted: grantees, revoked: revokees)`
    let revised = room.clone();
    c.app()
        .write(move |tx| {
            let granted = existing_user_ids(tx.conn(), &grantee_ids)?;
            let revoked: Vec<i64> = revised.user_ids(tx.conn())?.into_iter().filter(|id| !grantee_ids.contains(id)).collect();
            revised.revise(tx, &granted, &revoked)
        })
        .await?;
    broadcast_to_members(c, &room, true).await?;
    room_saved(c, room.id)
}

/// `broadcast_create_room` / `broadcast_update_room`: the shared-room partial, rendered once, to
/// every member's own rooms stream.
async fn broadcast_to_members(c: &Ctx, room: &Room, update: bool) -> Result<()> {
    let partials = render_shared_room(c, room).await?;
    let (broadcasts, room) = (c.app().broadcasts.clone(), room.clone());
    c.app()
        .read(move |conn| {
            if update {
                broadcasts.closed_room_update(conn, &room, &partials)
            } else {
                broadcasts.closed_room_create(conn, &room, &partials)
            }
        })
        .await
}
