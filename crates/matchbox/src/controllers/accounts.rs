//! `AccountsController` (reference/app/controllers/accounts_controller.rb): account settings.

pub mod bots;
pub mod custom_styles;
pub mod join_codes;
pub mod logos;
pub mod users;

use matchbox_db::Account;
use matchbox_kit::params::Permit;
use matchbox_kit::{Ctx, Error, Param, Redirect, Result, StatusCode, format};
use matchbox_views::accounts;

use super::presenters;
use super::presenters::attachments::{self, Assignment, Record};
use super::presenters::pagination::Page;
use crate::app::AppCtx;
use crate::concerns::{self, Before, current_user};
use crate::controllers::presenters::page::framed_page;

/// `set_page_and_extract_portion_from users, per_page: 500`
const PER_PAGE: &[i64] = &[500];

/// Everyone, administrators first; the page only decides whether a next-page loader follows.
pub async fn edit(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    let account = current_account(c).await?;
    c.respond_to(&[&format::HTML])?;
    let can_administer = current_user(c).is_some_and(|user| user.can_administer(None, false));
    let users = c.app().read_offloaded(move |conn| presenters::accounts::account_users(conn, can_administer)).await?;
    let page = Page::new(c.param_str("page"), users.len() as i64, PER_PAGE);

    let secrets = c.app().secrets.clone();
    let (administrators, members): (Vec<_>, Vec<_>) =
        users.iter().map(|user| presenters::user_summary(&secrets, user)).partition(|user| user.administrator());
    let next_page = (!page.is_last()).then(|| page.next_param().to_string());
    let restrict_room_creation_to_administrators = account.settings().restrict_room_creation_to_administrators();
    let user_id = concerns::require_current_user(c)?.id;
    let settings = account.settings();
    let order = settings.default_room_order();
    let mut rooms = if can_administer {
        c.app()
            .read(move |conn| {
                Ok(matchbox_db::Membership::visible_with_ordered_room(conn, user_id)?
                    .into_iter()
                    .filter(|(_, room)| !room.direct())
                    .map(|(_, room)| matchbox_views::users::SidebarRoom {
                        id: room.id,
                        param_key: String::new(),
                        icon: settings.channel_icon(room.id).map(str::to_owned),
                        name: room.name.unwrap_or_default(),
                        unread: false,
                    })
                    .collect::<Vec<_>>())
            })
            .await?
    } else {
        Vec::new()
    };
    let positions: std::collections::HashMap<_, _> = order.iter().enumerate().map(|(index, id)| (*id, index)).collect();
    rooms.sort_by_key(|room| positions.get(&room.id).copied().unwrap_or(usize::MAX));
    let room_order = serde_json::to_string(&rooms.iter().map(|room| room.id).collect::<Vec<_>>()).expect("room IDs are JSON");
    framed_page!(c, StatusCode::OK, |ctx| accounts::Edit {
        ctx,
        account_id: account.id,
        join_code: account.join_code.clone(),
        restrict_room_creation_to_administrators,
        administrators: administrators.clone(),
        members: members.clone(),
        rooms: rooms.clone(),
        room_order: room_order.clone(),
        next_page: next_page.clone(),
    })
    .await
}

/// `@account.update!(params.require(:account).permit(:name, :logo, settings: {}))`
pub async fn update(c: &mut Ctx) -> Result {
    concerns::before_actions(c, Before::default()).await?;
    concerns::ensure_can_administer(c)?;
    let mut account = current_account(c).await?;

    let params = c.params.require("account")?.permit(&[Permit::from("name"), Permit::from("logo"), Permit::AnyHash("settings".into())]);
    let name = params.get("name").and_then(Param::to_s);
    let settings: Option<Vec<(String, String)>> = params
        .get("settings")
        .and_then(Param::as_hash)
        .map(|settings| settings.iter().map(|(key, value)| (key.clone(), value.to_s().unwrap_or_default())).collect());
    // Immediate settings switches confirm their state in place, without a success toast.
    let settings_only = settings.is_some() && name.is_none() && params.get("logo").is_none();
    let logo = Assignment::from_params(&params, "logo")?.stage(c.app()).await?;

    let pending = c
        .app()
        .write(move |tx| {
            let settings: Option<Vec<(&str, &str)>> = settings.as_ref().map(|s| s.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect());
            account.update(tx, name.as_deref(), None, settings.as_deref())?;
            attachments::assign(tx, Record::account(account.id), "logo", logo)
        })
        .await?;
    attachments::analyze_later(c.app(), pending);

    let location = c.url_for(&matchbox_routes::edit_account());
    let notice = (!settings_only).then(|| "Changes saved.".into());
    c.redirect_to_with(&location, Redirect { notice, ..Redirect::default() })
}

/// `Current.account` where the reference dereferences it (a nil account raises NoMethodError).
pub async fn current_account(c: &Ctx) -> Result<Account> {
    c.app().read(Account::first).await?.ok_or_else(|| Error::internal(anyhow::anyhow!("no account")))
}
