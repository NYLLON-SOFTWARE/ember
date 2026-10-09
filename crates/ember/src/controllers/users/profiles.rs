//! `Users::ProfilesController` (reference/app/controllers/users/profiles_controller.rb): the
//! signed-in user's own profile.

use ember_db::UserChanges;
use ember_kit::{Ctx, Redirect, Result, StatusCode, format, permit_keys};
use ember_views::users;

use crate::app::AppCtx;
use crate::concerns::{self, Before};
use crate::controllers::presenters::attachments::{self, Assignment, Record};
use crate::controllers::presenters::page::framed_page;
use crate::controllers::presenters::{self, accounts::string_attribute};

/// `set_user` (`Current.user`); memberships partitioned into direct and shared rooms.
pub async fn show(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    c.respond_to(&[&format::HTML])?;
    let user = concerns::require_current_user(c)?.clone();
    let secrets = c.app().secrets.clone();
    let transfer_id = presenters::accounts::transfer_id(&secrets, user.id, c.now());
    let (avatar_attached, (direct_memberships, shared_memberships)) = {
        let user = user.clone();
        c.app()
            .read(move |conn| {
                let attached = attachments::attached_blob(conn, "User", user.id, "avatar")?.is_some();
                Ok((attached, presenters::accounts::profile_memberships(conn, &user)?))
            })
            .await?
    };
    let user = presenters::user_summary(&secrets, &user);
    framed_page!(c, StatusCode::OK, |ctx| users::ProfileShow {
        ctx,
        user: user.clone(),
        avatar_attached,
        transfer_id: transfer_id.clone(),
        shared_memberships: shared_memberships.clone(),
        direct_memberships: direct_memberships.clone(),
    })
    .await
}

/// `@user.update user_params`, then `redirect_to user_profile_url, notice: update_notice`.
pub async fn update(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    let mut user = concerns::require_current_user(c)?.clone();

    // params.require(:user).permit(:name, :avatar, :email_address, :password, :bio).compact
    let params = c.params.require("user")?.permit(&permit_keys(&["name", "avatar", "email_address", "password", "bio"]));
    let present = |key: &str| string_attribute(&params, key).flatten();
    let changes = UserChanges {
        name: present("name"),
        email_address: present("email_address").map(Some),
        // `password=` ignores a blank password.
        password_digest: concerns::password_digest(c, present("password").filter(|password| !password.is_empty())).await?,
        bio: present("bio").map(Some),
        ..UserChanges::default()
    };
    let avatar = match Assignment::from_params(&params, "avatar")? {
        // `.compact` drops a nil avatar before it's assigned.
        Assignment::Delete if params.get("avatar").is_none_or(|p| p.is_null()) => Assignment::Unchanged,
        assignment => assignment,
    };
    // `params[:user][:avatar] ? ... : "✓"`: any non-nil value counts.
    let notice = match c.params.get("user").and_then(|user| user.get("avatar")) {
        Some(avatar) if !avatar.is_null() => "It may take up to 30 minutes to change everywhere.",
        _ => "✓",
    };

    let avatar = avatar.stage(c.app()).await?;
    let pending = c
        .app()
        .write(move |tx| {
            user.update(tx, changes)?;
            attachments::assign(tx, Record::user(user.id), "avatar", avatar)
        })
        .await?;
    attachments::analyze_later(c.app(), pending);

    let location = c.url_for(&ember_routes::user_profile());
    c.redirect_to_with(&location, Redirect { notice: Some(notice.into()), ..Redirect::default() })
}
