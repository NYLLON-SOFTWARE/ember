//! `Autocompletable::UsersController` (reference/app/controllers/autocompletable/users_controller.rb):
//! mention and user-picker suggestions.

pub mod users {
    use askama::Template;
    use matchbox_db::{Connection, Room, User};
    use matchbox_kit::{Ctx, Error, Result, StatusCode, format};
    use matchbox_views::autocompletable;
    use ruby_compat::integer_cast;
    use rusqlite::types::Value;

    use crate::app::AppCtx;
    use crate::concerns::{self, Before};
    use crate::controllers::presenters;
    use crate::controllers::presenters::pagination::Page;
    use crate::controllers::presenters::view_context::Layout;

    /// `set_page_and_extract_portion_from find_autocompletable_users.with_attached_avatar.ordered, per_page: 20`
    pub async fn index(c: &mut Ctx) -> Result {
        concerns::before_actions(c, Before::default()).await?;
        let user_id = concerns::require_current_user(c)?.id;

        // `params[:room_id].present? ? Current.user.rooms.find(params[:room_id]).users : User.all`
        let room_id = match c.params.get("room_id").filter(|param| param.is_present()) {
            Some(param) => {
                let id = param.as_str().and_then(integer_cast).ok_or(Error::NotFound)?;
                let room = c.app().read(move |conn| Room::find_for_user(conn, user_id, id)).await?;
                Some(room.ok_or(Error::NotFound)?.id)
            }
            None => None,
        };
        // The rich text editor's mentions prompt filters with `filter`, the autocomplete inputs with `query`
        let query = [c.params.get("filter"), c.params.get("query")]
            .into_iter()
            .flatten()
            .find(|param| param.is_present())
            .and_then(|param| param.to_s());

        let users = c.app().read_offloaded(move |conn| autocompletable_users(conn, room_id, query.as_deref())).await?;
        let page = Page::new(c.param_str("page"), users.len() as i64, &[20]);
        let secrets = c.app().secrets.clone();
        let users: Vec<_> = page.records(&users).iter().map(|user| presenters::accounts::mention_user(&secrets, user)).collect();

        let format = c.respond_to(&[&format::HTML, &format::JSON])?;
        page.apply_headers(c);
        if *format == format::JSON {
            let body = autocompletable::users_index_json(&users, &c.url_for(""));
            Ok(c.render_as(StatusCode::OK, "application/json; charset=utf-8", body))
        } else {
            // `render layout: false`: <lexxy-prompt-item> elements for the mentions prompt
            let layout = Layout::load(c).await?;
            let html = layout.render(c, |ctx| autocompletable::UsersIndex { ctx, users }.render())?;
            Ok(c.render(StatusCode::OK, &format::HTML, html))
        }
    }

    /// `users_scope.active[.filtered_by(query)].ordered`
    fn autocompletable_users(conn: &Connection, room_id: Option<i64>, query: Option<&str>) -> matchbox_db::Result<Vec<User>> {
        let mut sql = String::from(User::SELECT);
        let mut values = Vec::new();
        if let Some(room_id) = room_id {
            sql.push_str(r#" INNER JOIN "memberships" ON "users"."id" = "memberships"."user_id" WHERE "memberships"."room_id" = ? AND"#);
            values.push(Value::Integer(room_id));
        } else {
            sql.push_str(" WHERE");
        }
        sql.push_str(r#" "users"."status" = 0"#);
        if let Some(query) = query {
            sql.push_str(" AND (name like ?)");
            values.push(Value::Text(format!("%{query}%")));
        }
        sql.push_str(" ORDER BY LOWER(name)");
        User::find_by_sql(conn, &sql, rusqlite::params_from_iter(values))
    }
}
