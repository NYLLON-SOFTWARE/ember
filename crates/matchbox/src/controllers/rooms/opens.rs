//! `Rooms::OpensController` (reference/app/controllers/rooms/opens_controller.rb). `index` is
//! RoomsController's; `destroy` is RoomsController's without `set_room`
//! (`super::destroy_without_room`).

use matchbox_db::{Room, RoomType, User};
use matchbox_kit::{Ctx, Result, StatusCode};
use matchbox_views::rooms::{FormRoom, OpenFormView, OpensEdit, OpensNew};

use super::{
    Scope, ensure_can_administer, ensure_permission_to_create_rooms, redirect_to_room, render_shared_room, room_icon, room_icon_param,
    room_name_param, set_room,
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
    let form = OpenFormView {
        room: FormRoom { id: None, name: Some(DEFAULT_ROOM_NAME.into()), icon: None },
        can_administer: true,
        users: active_users(c).await?,
    };
    page::framed_page!(c, StatusCode::OK, |ctx| OpensNew { ctx, form: &form }).await
}

pub async fn create(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    ensure_permission_to_create_rooms(c).await?;
    let name = room_name_param(c)?.flatten();
    let icon = room_icon_param(c)?;
    let user_id = require_current_user(c)?.id;
    // Rooms::Open.create_for(room_params, users: Current.user)
    let room = c
        .app()
        .write(move |tx| {
            let mut room = Room::create_for(tx, RoomType::Open, name.as_deref(), user_id, &[user_id])?;
            if let Some(icon) = icon {
                room.set_icon(tx, icon.as_deref())?;
            }
            Ok(room)
        })
        .await?;
    let partials = render_shared_room(c, &room).await?;
    c.app().broadcasts.open_room_create(&room, &partials);
    redirect_to_room(c, room.id)
}

pub async fn edit(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let mut room = set_room(c, Scope::WithoutDirects).await?;
    room.room_type = RoomType::Open; // force_room_type
    let form = OpenFormView {
        room: FormRoom { id: Some(room.id), name: room.name.clone(), icon: room_icon(c, room.id).await? },
        can_administer: require_current_user(c)?.can_administer(Some(room.creator_id), false),
        users: active_users(c).await?,
    };
    page::framed_page!(c, StatusCode::OK, |ctx| OpensEdit { ctx, form: &form }).await
}

pub async fn update(c: &mut Ctx) -> Result {
    before_actions(c, Before::default()).await?;
    let room = set_room(c, Scope::WithoutDirects).await?;
    ensure_can_administer(c, &room)?;
    let name = room_name_param(c)?;
    let icon = room_icon_param(c)?;
    // force_room_type, then `@room.update! room_params` saves the name and the new type.
    let room = c
        .app()
        .write(move |tx| {
            let mut room = room;
            room.update(tx, name.as_ref().map(|name| name.as_deref()), Some(RoomType::Open))?;
            if let Some(icon) = icon {
                room.set_icon(tx, icon.as_deref())?;
            }
            Ok(room)
        })
        .await?;
    let partials = render_shared_room(c, &room).await?;
    c.app().broadcasts.open_room_update(&room, &partials);
    redirect_to_room(c, room.id)
}

/// `User.active.ordered`
pub(super) async fn active_users(c: &Ctx) -> Result<Vec<matchbox_views::messages::UserView>> {
    let secrets = c.app().secrets.clone();
    c.app().read_offloaded(move |conn| Ok(User::active_ordered(conn)?.iter().map(|user| user_view(&secrets, user)).collect())).await
}
