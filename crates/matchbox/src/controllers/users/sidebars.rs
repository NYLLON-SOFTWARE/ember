//! `Users::SidebarsController` (reference/app/controllers/users/sidebars_controller.rb): the room
//! list, loaded into the `user_sidebar` turbo frame.

use matchbox_db::Account;
use matchbox_kit::{Ctx, Result, StatusCode, format};
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
    let (sidebar, restricted) = {
        let (user, secrets, fragments) = (user.clone(), secrets.clone(), c.app().fragment_cache.clone());
        c.app()
            .read(move |conn| {
                // The direct rooms' fragments come from the store the render then uses.
                let sidebar = matchbox_views::fragment_cache::with(&fragments, || presenters::accounts::sidebar(conn, &secrets, &user))?;
                let restricted = Account::first(conn)?.is_some_and(|account| account.settings().restrict_room_creation_to_administrators());
                Ok((sidebar, restricted))
            })
            .await?
    };

    let data = SidebarData {
        current_user: presenters::user_summary(&secrets, &user),
        // turbo_stream_from :rooms / turbo_stream_from Current.user, :rooms
        rooms_stream: rails_compat::turbo::signed_stream_name(&secrets, &["rooms"]),
        user_rooms_stream: rails_compat::turbo::signed_stream_name(&secrets, &[&user_gid(user.id).to_param(), "rooms"]),
        sidebar,
        // `Current.user.administrator? || !Current.account.settings.restrict_room_creation_to_administrators?`
        can_create_rooms: user.is_administrator() || !restricted,
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

struct SidebarData {
    current_user: matchbox_views::users::UserSummary,
    rooms_stream: String,
    user_rooms_stream: String,
    sidebar: presenters::accounts::Sidebar,
    can_create_rooms: bool,
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
            can_create_rooms: self.can_create_rooms,
        }
    }
}
