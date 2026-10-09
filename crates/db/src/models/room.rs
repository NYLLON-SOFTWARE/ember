//! `reference/app/models/room.rb`, `rooms/*.rb` and `room/message_pusher.rb`'s queries.

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::{Connection, params};

use crate::database::Tx;
use crate::error::{Errors, OptionalExt, Result};
use crate::events::Event;
use crate::models::membership::membership_columns;
use crate::models::user::user_columns;
use crate::models::{Membership, Message, User};
use crate::sql::{self, CachedStatements, columns, placeholders, query_all, query_one};
use crate::time::{SQLITE_NOW, Timestamp};

/// The STI `type` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoomType {
    Open,
    Closed,
    Direct,
}

impl RoomType {
    pub fn class_name(self) -> &'static str {
        match self {
            RoomType::Open => "Rooms::Open",
            RoomType::Closed => "Rooms::Closed",
            RoomType::Direct => "Rooms::Direct",
        }
    }

    pub fn from_class_name(name: &str) -> Option<Self> {
        match name {
            "Rooms::Open" => Some(RoomType::Open),
            "Rooms::Closed" => Some(RoomType::Closed),
            "Rooms::Direct" => Some(RoomType::Direct),
            _ => None,
        }
    }

    /// `default_involvement`: "everything" in direct rooms, "mentions" elsewhere.
    pub fn default_involvement(self) -> &'static str {
        match self {
            RoomType::Direct => "everything",
            _ => "mentions",
        }
    }
}

impl ToSql for RoomType {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.class_name()))
    }
}

impl FromSql for RoomType {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let name = value.as_str()?;
        RoomType::from_class_name(name).ok_or_else(|| FromSqlError::Other(format!("unknown room type {name:?}").into()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Room {
    pub id: i64,
    pub name: Option<String>,
    pub room_type: RoomType,
    pub creator_id: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

columns! {
    Room, "rooms", room_columns {
        id: "id",
        name: "name",
        room_type: "type",
        creator_id: "creator_id",
        created_at: "created_at",
        updated_at: "updated_at",
    }
}

const SELECT_FOR_USER: &str = concat!(
    "SELECT ",
    room_columns!(),
    r#" FROM "rooms" INNER JOIN "memberships" ON "rooms"."id" = "memberships"."room_id" WHERE "memberships"."user_id" = ?"#
);

impl Room {
    pub fn find(conn: &Connection, id: i64) -> Result<Self> {
        Self::find_by_id(conn, id)?.or_not_found("Room")
    }

    pub fn find_by_id(conn: &Connection, id: i64) -> Result<Option<Self>> {
        query_one(conn, concat!("SELECT ", room_columns!(), r#" FROM "rooms" WHERE "rooms"."id" = ? LIMIT 1"#), [id], Self::from_row)
    }

    pub fn all(conn: &Connection) -> Result<Vec<Self>> {
        query_all(conn, concat!("SELECT ", room_columns!(), r#" FROM "rooms""#), [], Self::from_row)
    }

    /// `Room.opens` / `closeds` / `directs`
    pub fn of_type(conn: &Connection, room_type: RoomType) -> Result<Vec<Self>> {
        query_all(conn, concat!("SELECT ", room_columns!(), r#" FROM "rooms" WHERE "rooms"."type" = ?"#), [room_type], Self::from_row)
    }

    pub fn count_of_type(conn: &Connection, room_type: RoomType) -> Result<i64> {
        sql::count(conn, r#"SELECT COUNT(*) FROM "rooms" WHERE "rooms"."type" = ?"#, [room_type])
    }

    /// `Room.original`: the oldest room.
    pub fn original(conn: &Connection) -> Result<Option<Self>> {
        query_one(
            conn,
            concat!("SELECT ", room_columns!(), r#" FROM "rooms" ORDER BY "rooms"."created_at" ASC LIMIT 1"#),
            [],
            Self::from_row,
        )
    }

    // `Current.user.rooms` and its scopes

    /// `user.rooms`
    pub fn for_user(conn: &Connection, user_id: i64) -> Result<Vec<Self>> {
        query_all(conn, SELECT_FOR_USER, [user_id], Self::from_row)
    }

    /// `user.rooms.find_by(id:)`
    pub fn find_for_user(conn: &Connection, user_id: i64, room_id: i64) -> Result<Option<Self>> {
        let sql = format!(r#"{SELECT_FOR_USER} AND "rooms"."id" = ? LIMIT 1"#);
        query_one(conn, &sql, [user_id, room_id], Self::from_row)
    }

    /// `user.rooms.directs` / `.opens` / `.closeds`
    pub fn for_user_of_type(conn: &Connection, user_id: i64, room_type: RoomType) -> Result<Vec<Self>> {
        let sql = format!(r#"{SELECT_FOR_USER} AND "rooms"."type" = ?"#);
        query_all(conn, &sql, params![user_id, room_type], Self::from_row)
    }

    /// `user.rooms.without_directs`
    pub fn for_user_without_directs(conn: &Connection, user_id: i64) -> Result<Vec<Self>> {
        let sql = format!(r#"{SELECT_FOR_USER} AND "rooms"."type" != ?"#);
        query_all(conn, &sql, params![user_id, RoomType::Direct], Self::from_row)
    }

    /// `user.rooms.original`
    pub fn original_for_user(conn: &Connection, user_id: i64) -> Result<Option<Self>> {
        let sql = format!(r#"{SELECT_FOR_USER} ORDER BY "rooms"."created_at" ASC LIMIT 1"#);
        query_one(conn, &sql, [user_id], Self::from_row)
    }

    /// `user.rooms.last`
    pub fn last_for_user(conn: &Connection, user_id: i64) -> Result<Option<Self>> {
        let sql = format!(r#"{SELECT_FOR_USER} ORDER BY "rooms"."id" DESC LIMIT 1"#);
        query_one(conn, &sql, [user_id], Self::from_row)
    }

    // Creating

    /// `Rooms::<Type>.create!(name:, creator:)`. An open room grants itself to every active
    /// user after commit (`Rooms::Open#grant_access_to_all_users`).
    pub fn create(tx: &mut Tx<'_>, room_type: RoomType, name: Option<&str>, creator_id: i64) -> Result<Self> {
        let now = tx.now();
        let id: i64 = tx.conn().query_row_cached(
            r#"INSERT INTO "rooms" ("created_at", "creator_id", "name", "type", "updated_at") VALUES (?, ?, ?, ?, ?) RETURNING "id""#,
            params![now, creator_id, name, room_type, now],
            |r| r.get(0),
        )?;
        if room_type == RoomType::Open {
            tx.after_commit(move |tx| grant_to_active_users(tx, id));
        }
        Self::find(tx.conn(), id)
    }

    /// `Room.create_for(attributes, users:)`
    pub fn create_for(tx: &mut Tx<'_>, room_type: RoomType, name: Option<&str>, creator_id: i64, user_ids: &[i64]) -> Result<Self> {
        let room = Self::create(tx, room_type, name, creator_id)?;
        room.grant_to(tx, user_ids)?;
        Ok(room)
    }

    /// `Rooms::Direct.find_or_create_for(users)`: the direct room whose members are exactly
    /// `user_ids`, created (by `creator_id`, i.e. `Current.user`) if there isn't one.
    pub fn find_or_create_direct_for(tx: &mut Tx<'_>, user_ids: &[i64], creator_id: i64) -> Result<Self> {
        match Self::find_direct_for(tx.conn(), user_ids)? {
            Some(room) => Ok(room),
            None => Self::create_for(tx, RoomType::Direct, None, creator_id, user_ids),
        }
    }

    /// `Rooms::Direct.find_for`: walks the direct rooms (joined to their users, as Rails
    /// does, so rooms repeat) and returns the first with the same set of members.
    pub fn find_direct_for(conn: &Connection, user_ids: &[i64]) -> Result<Option<Self>> {
        let wanted: std::collections::BTreeSet<i64> = user_ids.iter().copied().collect();
        let candidates = query_all(
            conn,
            concat!(
                "SELECT ",
                room_columns!(),
                r#" FROM "rooms" INNER JOIN "memberships" ON "memberships"."room_id" = "rooms"."id" INNER JOIN "users" ON "users"."id" = "memberships"."user_id" WHERE "rooms"."type" = ?"#
            ),
            [RoomType::Direct],
            Self::from_row,
        )?;
        for room in candidates {
            let members: std::collections::BTreeSet<i64> = room.user_ids(conn)?.into_iter().collect();
            if members == wanted {
                return Ok(Some(room));
            }
        }
        Ok(None)
    }

    // Updating

    /// `room.update!(name:, type:)`. A direct room can't change type
    /// (`direct_rooms_keep_their_type`). Becoming open grants every active user after commit.
    pub fn update(&mut self, tx: &mut Tx<'_>, name: Option<Option<&str>>, room_type: Option<RoomType>) -> Result<()> {
        let name = name.map(|n| n.map(str::to_string)).filter(|n| *n != self.name);
        let room_type = room_type.filter(|t| *t != self.room_type);

        if room_type.is_some() && self.room_type == RoomType::Direct {
            let mut errors = Errors::default();
            errors.add("type", "can't be changed for a direct room");
            return errors.into_result();
        }
        if name.is_none() && room_type.is_none() {
            return Ok(());
        }

        let now = tx.now();
        if let Some(name) = &name {
            self.name = name.clone();
        }
        if let Some(room_type) = room_type {
            self.room_type = room_type;
        }
        self.updated_at = now;
        tx.conn().execute_cached(
            r#"UPDATE "rooms" SET "name" = ?, "type" = ?, "updated_at" = ? WHERE "rooms"."id" = ?"#,
            params![self.name, self.room_type, now, self.id],
        )?;
        if room_type == Some(RoomType::Open) {
            let id = self.id;
            tx.after_commit(move |tx| grant_to_active_users(tx, id));
        }
        Ok(())
    }

    /// `touch`: `belongs_to :room, touch: true` on messages.
    pub fn touch(tx: &Tx<'_>, room_id: i64) -> Result<()> {
        tx.conn().execute_cached(r#"UPDATE "rooms" SET "updated_at" = ? WHERE "rooms"."id" = ?"#, params![tx.now(), room_id])?;
        Ok(())
    }

    /// Ember channel appearance is shared across members; no Rails table changes are needed.
    /// `icon` has already been validated against the embedded catalog by the controller.
    pub fn set_icon(&mut self, tx: &mut Tx<'_>, icon: Option<&str>) -> Result<()> {
        if self.direct() {
            let mut errors = Errors::default();
            errors.add("icon", "cannot be set for direct messages");
            return errors.into_result();
        }
        if crate::Account::set_channel_icon(tx, self.id, icon)? {
            self.updated_at = tx.now();
            tx.conn().execute_cached(r#"UPDATE "rooms" SET "updated_at" = ? WHERE "id" = ?"#, params![self.updated_at, self.id])?;
        }
        Ok(())
    }

    /// `room.destroy`: memberships are deleted without callbacks, messages are destroyed.
    pub fn destroy(&self, tx: &mut Tx<'_>) -> Result<()> {
        crate::Account::set_channel_icon(tx, self.id, None)?;
        tx.conn().execute_cached(r#"DELETE FROM "memberships" WHERE "memberships"."room_id" = ?"#, [self.id])?;
        for message in Message::for_room(tx.conn(), self.id)? {
            message.destroy(tx)?;
        }
        tx.conn().execute_cached(r#"DELETE FROM "rooms" WHERE "rooms"."id" = ?"#, [self.id])?;
        Ok(())
    }

    // Memberships

    pub fn memberships(&self, conn: &Connection) -> Result<Vec<Membership>> {
        Membership::for_room(conn, self.id)
    }

    /// `memberships.grant_to(users)`: `Membership.insert_all` with the room's default
    /// involvement, skipping existing members.
    pub fn grant_to(&self, tx: &mut Tx<'_>, user_ids: &[i64]) -> Result<()> {
        insert_memberships(tx, self.id, self.room_type.default_involvement(), user_ids)
    }

    /// `memberships.revoke_from(users)`: `destroy_by`, so each removed member is disconnected
    /// (with reconnect) after commit.
    pub fn revoke_from(&self, tx: &mut Tx<'_>, user_ids: &[i64]) -> Result<()> {
        let sql = format!(
            r#"SELECT {} FROM "memberships" WHERE "memberships"."room_id" = ? AND "memberships"."user_id" IN ({})"#,
            membership_columns!(),
            placeholders(user_ids.len())
        );
        let values: Vec<i64> = std::iter::once(self.id).chain(user_ids.iter().copied()).collect();
        for membership in query_all(tx.conn(), &sql, rusqlite::params_from_iter(values), Membership::from_row)? {
            membership.destroy(tx)?;
        }
        Ok(())
    }

    /// `memberships.revise(granted:, revoked:)`
    pub fn revise(&self, tx: &mut Tx<'_>, granted: &[i64], revoked: &[i64]) -> Result<()> {
        if !granted.is_empty() {
            self.grant_to(tx, granted)?;
        }
        if !revoked.is_empty() {
            self.revoke_from(tx, revoked)?;
        }
        Ok(())
    }

    /// `room.users`
    pub fn users(&self, conn: &Connection) -> Result<Vec<User>> {
        query_all(
            conn,
            concat!(
                "SELECT ",
                user_columns!(),
                r#" FROM "users" INNER JOIN "memberships" ON "users"."id" = "memberships"."user_id" WHERE "memberships"."room_id" = ?"#
            ),
            [self.id],
            User::from_row,
        )
    }

    /// `room.user_ids`
    pub fn user_ids(&self, conn: &Connection) -> Result<Vec<i64>> {
        query_all(
            conn,
            r#"SELECT "users"."id" FROM "users" INNER JOIN "memberships" ON "users"."id" = "memberships"."user_id" WHERE "memberships"."room_id" = ?"#,
            [self.id],
            |r| r.get(0),
        )
    }

    /// `room.users.active_bots`
    pub fn active_bots(&self, conn: &Connection) -> Result<Vec<User>> {
        query_all(
            conn,
            concat!(
                "SELECT ",
                user_columns!(),
                r#" FROM "users" INNER JOIN "memberships" ON "users"."id" = "memberships"."user_id" WHERE "memberships"."room_id" = ? AND "users"."status" = 0 AND "users"."role" = 2"#
            ),
            [self.id],
            User::from_row,
        )
    }

    /// `room.receive(message)`, from the message's `after_create_commit`: marks members
    /// unread, then enqueues the push.
    pub(crate) fn receive(tx: &mut Tx<'_>, room_id: i64, message: &Message) -> Result<()> {
        Self::unread_memberships(tx, room_id, message)?;
        tx.emit_after_commit(Event::PushMessage { room_id, message_id: message.id });
        Ok(())
    }

    fn unread_memberships(tx: &Tx<'_>, room_id: i64, message: &Message) -> Result<()> {
        let now = tx.now();
        tx.conn().execute_cached(
            r#"UPDATE "memberships" SET "unread_at" = ?, "updated_at" = ? WHERE "memberships"."room_id" = ? AND "memberships"."involvement" != ? AND ("memberships"."connected_at" IS NULL OR "memberships"."connected_at" < ?) AND "memberships"."user_id" != ?"#,
            params![message.created_at, now, room_id, "invisible", Membership::connection_cutoff(now), message.creator_id],
        )?;
        Ok(())
    }

    pub fn open(&self) -> bool {
        self.room_type == RoomType::Open
    }

    pub fn closed(&self) -> bool {
        self.room_type == RoomType::Closed
    }

    pub fn direct(&self) -> bool {
        self.room_type == RoomType::Direct
    }

    pub fn default_involvement(&self) -> &'static str {
        self.room_type.default_involvement()
    }

    pub fn reload(&mut self, conn: &Connection) -> Result<()> {
        *self = Self::find(conn, self.id)?;
        Ok(())
    }
}

/// `Membership.insert_all(... { room_id:, user_id:, involvement: })`
fn insert_memberships(tx: &Tx<'_>, room_id: i64, involvement: &str, user_ids: &[i64]) -> Result<()> {
    // In batches: SQLite binds at most 32,766 variables per statement, 3 per row here.
    for user_ids in user_ids.chunks(MEMBERSHIP_INSERT_BATCH) {
        let rows: Vec<String> = user_ids.iter().map(|_| format!("({SQLITE_NOW}, ?, ?, {SQLITE_NOW}, ?)")).collect();
        let sql = format!(
            r#"INSERT INTO "memberships" ("created_at","involvement","room_id","updated_at","user_id") VALUES {} ON CONFLICT  DO NOTHING RETURNING "id""#,
            rows.join(", ")
        );
        let mut values: Vec<rusqlite::types::Value> = Vec::new();
        for user_id in user_ids {
            values.push(involvement.to_string().into());
            values.push(room_id.into());
            values.push((*user_id).into());
        }
        let mut stmt = tx.conn().prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(values))?;
        while rows.next()?.is_some() {}
    }
    Ok(())
}

/// Rows per `INSERT` of memberships, well under SQLite's bound-variable limit.
pub(crate) const MEMBERSHIP_INSERT_BATCH: usize = 1_000;

/// `memberships.grant_to(User.active)`, from `Rooms::Open`'s `after_save_commit`.
fn grant_to_active_users(tx: &mut Tx<'_>, room_id: i64) -> Result<()> {
    let user_ids: Vec<i64> = query_all(tx.conn(), r#"SELECT "users"."id" FROM "users" WHERE "users"."status" = ?"#, [0], |r| r.get(0))?;
    let room = Room::find(tx.conn(), room_id)?;
    insert_memberships(tx, room_id, room.default_involvement(), &user_ids)
}
