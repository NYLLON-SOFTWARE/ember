//! `reference/app/models/account.rb` and `account/joinable.rb`.

use rusqlite::{Connection, params};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

use crate::database::Tx;
use crate::error::{Error, OptionalExt, Result};
use crate::sql::{self, CachedStatements, columns, query_one};
use crate::time::Timestamp;

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub join_code: String,
    pub custom_styles: Option<String>,
    /// The raw `settings` JSON column; read it through [`Account::settings`].
    pub settings_json: Option<String>,
    pub singleton_guard: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// `has_json :settings, restrict_room_creation_to_administrators: false`
/// (`ActiveModel::SchematizedJson`): the stored hash with the schema defaults merged in.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountSettings {
    data: Map<String, Value>,
}

const RESTRICT_ROOM_CREATION: &str = "restrict_room_creation_to_administrators";
const HIDE_TRANSLATION_BUTTONS: &str = "hide_translation_buttons";
const DEFAULT_ROOM_ORDER: &str = "matchbox_default_room_order";
const CHANNEL_ORDER: &str = "matchbox_channel_order";
const FAVORITE_CHANNELS: &str = "matchbox_favorite_channels";
const CHANNEL_ICONS: &str = "matchbox_channel_icons";

pub const MAX_CHANNEL_ORDER: usize = 4096;

impl AccountSettings {
    fn from_column(raw: Option<&str>) -> Self {
        let mut data = raw.and_then(|r| serde_json::from_str::<Map<String, Value>>(r).ok()).unwrap_or_default();
        data.entry(RESTRICT_ROOM_CREATION).or_insert(Value::Bool(false));
        Self { data }
    }

    /// `restrict_room_creation_to_administrators?`: `present?` of the stored value.
    pub fn restrict_room_creation_to_administrators(&self) -> bool {
        present(self.data.get(RESTRICT_ROOM_CREATION))
    }

    /// `restrict_room_creation_to_administrators = value`, cast as a boolean.
    pub fn set_restrict_room_creation_to_administrators(&mut self, value: &str) {
        let cast = cast_boolean(value).map(Value::Bool).unwrap_or(Value::Null);
        self.data.insert(RESTRICT_ROOM_CREATION.into(), cast);
    }

    /// Ember defaults to hiding translation controls, including for existing accounts.
    /// Keep the default virtual so reading old settings does not rewrite their JSON.
    pub fn hide_translation_buttons(&self) -> bool {
        self.data.get(HIDE_TRANSLATION_BUTTONS).is_none_or(|value| present(Some(value)))
    }

    /// Personal preferences live under user IDs in the existing JSON column, without changing
    /// the Rails schema. Missing preferences leave channels in their normal alphabetical order.
    pub fn channel_order(&self, user_id: i64) -> Vec<i64> {
        self.personal_channels(CHANNEL_ORDER, user_id)
    }

    pub fn default_room_order(&self) -> Vec<i64> {
        self.data
            .get(DEFAULT_ROOM_ORDER)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_i64)
            .filter(|id| *id > 0)
            .take(MAX_CHANNEL_ORDER)
            .collect()
    }

    pub fn favorite_channels(&self, user_id: i64) -> Vec<i64> {
        self.personal_channels(FAVORITE_CHANNELS, user_id)
    }

    fn personal_channels(&self, key: &str, user_id: i64) -> Vec<i64> {
        self.data
            .get(key)
            .and_then(|orders| orders.get(user_id.to_string()))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_i64)
            .filter(|id| *id > 0)
            .take(MAX_CHANNEL_ORDER)
            .collect()
    }

    pub fn channel_icon(&self, room_id: i64) -> Option<&str> {
        self.data.get(CHANNEL_ICONS)?.get(room_id.to_string())?.as_str().filter(|icon| !icon.is_empty())
    }

    /// Load this once when presenting several channels, rather than querying each room.
    pub fn channel_icons(&self) -> HashMap<i64, String> {
        self.data
            .get(CHANNEL_ICONS)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(id, icon)| Some((id.parse().ok()?, icon.as_str().filter(|icon| !icon.is_empty())?.to_string())))
            .collect()
    }

    /// `assign_data_with_type_casting`: every key must be in the schema.
    pub fn assign(&mut self, values: &[(&str, &str)]) -> Result<()> {
        for (key, value) in values {
            match *key {
                RESTRICT_ROOM_CREATION => self.set_restrict_room_creation_to_administrators(value),
                HIDE_TRANSLATION_BUTTONS => {
                    self.data.insert(HIDE_TRANSLATION_BUTTONS.into(), cast_boolean(value).map(Value::Bool).unwrap_or(Value::Null));
                }
                other => {
                    return Err(Error::other(format!("undefined method '{other}=' for account settings")));
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.data.get(key)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.data).expect("settings encode")
    }
}

/// `ActiveModel::Type::Boolean#cast` of a form value: blank is nil, the `FALSE_VALUES` are
/// false, anything else is true.
pub fn cast_boolean(value: &str) -> Option<bool> {
    const FALSE_VALUES: &[&str] = &["0", "f", "F", "false", "FALSE", "off", "OFF"];
    if value.is_empty() { None } else { Some(!FALSE_VALUES.contains(&value)) }
}

fn present(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(_) => true,
    }
}

columns! {
    Account, "accounts", account_columns {
        id: "id",
        name: "name",
        join_code: "join_code",
        custom_styles: "custom_styles",
        settings_json: "settings",
        singleton_guard: "singleton_guard",
        created_at: "created_at",
        updated_at: "updated_at",
    }
}

impl Account {
    /// Display the former default workspace name with the current branding without rewriting data.
    pub fn display_name(&self) -> &str {
        if matches!(self.name.as_str(), "Campfire" | "Matchbox") { "Ember" } else { &self.name }
    }

    /// `Account.first` (`Current.account`).
    pub fn first(conn: &Connection) -> Result<Option<Self>> {
        query_one(
            conn,
            concat!("SELECT ", account_columns!(), r#" FROM "accounts" ORDER BY "accounts"."id" ASC LIMIT 1"#),
            [],
            Self::from_row,
        )
    }

    pub fn find(conn: &Connection, id: i64) -> Result<Self> {
        query_one(
            conn,
            concat!("SELECT ", account_columns!(), r#" FROM "accounts" WHERE "accounts"."id" = ? LIMIT 1"#),
            [id],
            Self::from_row,
        )?
        .or_not_found("Account")
    }

    pub fn count(conn: &Connection) -> Result<i64> {
        sql::count(conn, r#"SELECT COUNT(*) FROM "accounts""#, [])
    }

    pub fn settings(&self) -> AccountSettings {
        AccountSettings::from_column(self.settings_json.as_deref())
    }

    /// `Account.create!(name:)`: a fresh join code, and the settings defaults written out.
    pub fn create(tx: &mut Tx<'_>, name: &str) -> Result<Self> {
        let now = tx.now();
        let join_code = generate_join_code();
        let settings = AccountSettings::from_column(None).to_json();
        let id: i64 = tx.conn().query_row_cached(
            r#"INSERT INTO "accounts" ("created_at", "custom_styles", "join_code", "name", "settings", "singleton_guard", "updated_at") VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING "id""#,
            params![now, None::<String>, join_code, name, settings, 0, now],
            |r| r.get(0),
        )?;
        Self::find(tx.conn(), id)
    }

    /// `reset_join_code`
    pub fn reset_join_code(&mut self, tx: &mut Tx<'_>) -> Result<()> {
        let join_code = generate_join_code();
        let now = tx.now();
        tx.conn().execute_cached(
            r#"UPDATE "accounts" SET "join_code" = ?, "updated_at" = ? WHERE "accounts"."id" = ?"#,
            params![join_code, now, self.id],
        )?;
        self.join_code = join_code;
        self.updated_at = now;
        Ok(())
    }

    /// `update!(name:, custom_styles:, settings:)`. Only changed attributes are written; the
    /// settings column is written when its (defaulted) hash changes.
    pub fn update(
        &mut self,
        tx: &mut Tx<'_>,
        name: Option<&str>,
        custom_styles: Option<Option<&str>>,
        settings: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        let mut sets: Vec<(&str, Box<dyn rusqlite::ToSql>)> = Vec::new();
        if let Some(name) = name.filter(|n| *n != self.name) {
            self.name = name.into();
            sets.push(("name", Box::new(name.to_string())));
        }
        if let Some(styles) = custom_styles.map(|s| s.map(str::to_string)).filter(|s| *s != self.custom_styles) {
            self.custom_styles = styles.clone();
            sets.push(("custom_styles", Box::new(styles)));
        }
        if let Some(values) = settings {
            // Account changes may have been read before this write transaction was queued.
            // Merge into the latest settings so a concurrent personal preference is preserved.
            self.settings_json = Self::find(tx.conn(), self.id)?.settings_json;
            let original = self.settings();
            let mut updated = original.clone();
            updated.assign(values)?;
            if self.settings_json.is_none() || updated != original {
                let json = updated.to_json();
                self.settings_json = Some(json.clone());
                sets.push(("settings", Box::new(json)));
            }
        }
        if sets.is_empty() {
            return Ok(());
        }
        let now = tx.now();
        self.updated_at = now;
        sets.push(("updated_at", Box::new(now)));
        let assignments: Vec<String> = sets.iter().map(|(c, _)| format!(r#""{c}" = ?"#)).collect();
        let sql = format!(r#"UPDATE "accounts" SET {} WHERE "accounts"."id" = ?"#, assignments.join(", "));
        let mut values: Vec<&dyn rusqlite::ToSql> = sets.iter().map(|(_, v)| v.as_ref()).collect();
        values.push(&self.id);
        tx.conn().execute_cached(&sql, values.as_slice())?;
        Ok(())
    }

    pub fn reload(&mut self, conn: &Connection) -> Result<()> {
        *self = Self::find(conn, self.id)?;
        Ok(())
    }

    /// Set only this user's visible shared-channel order. Empty resets it to alphabetical.
    /// Returns false for invalid IDs or inaccessible rooms, without changing any preferences.
    pub fn set_channel_order(tx: &mut Tx<'_>, user_id: i64, room_ids: &[i64]) -> Result<bool> {
        if room_ids.len() > MAX_CHANNEL_ORDER {
            return Ok(false);
        }
        let accessible: HashSet<i64> = crate::Membership::visible_with_ordered_room(tx.conn(), user_id)?
            .into_iter()
            .filter(|(_, room)| !room.direct())
            .map(|(_, room)| room.id)
            .collect();
        let mut seen = HashSet::new();
        if room_ids.iter().any(|id| !accessible.contains(id) || !seen.insert(*id)) {
            return Ok(false);
        }

        let account = Self::first(tx.conn())?.or_not_found("Account")?;
        let mut settings = account.settings();
        let orders = settings.data.entry(CHANNEL_ORDER).or_insert_with(|| Value::Object(Map::new()));
        if !orders.is_object() {
            *orders = Value::Object(Map::new());
        }
        let orders = orders.as_object_mut().expect("channel orders are an object");
        if room_ids.is_empty() {
            orders.remove(&user_id.to_string());
        } else {
            orders.insert(user_id.to_string(), Value::Array(room_ids.iter().copied().map(Value::from).collect()));
        }
        if orders.is_empty() {
            settings.data.remove(CHANNEL_ORDER);
        }
        // This is a personal display preference, not an account/logo change: leave updated_at
        // and therefore account image URLs and shared message fragment keys untouched.
        tx.conn().execute_cached(r#"UPDATE "accounts" SET "settings" = ? WHERE "id" = ?"#, params![settings.to_json(), account.id])?;
        Ok(true)
    }

    /// Administrators set the workspace fallback without replacing members' personal orders.
    /// Recheck both the role and room visibility inside the write transaction.
    pub fn set_default_room_order(tx: &mut Tx<'_>, user_id: i64, room_ids: &[i64]) -> Result<bool> {
        if !crate::User::find_active(tx.conn(), user_id)?.is_administrator() || room_ids.len() > MAX_CHANNEL_ORDER {
            return Ok(false);
        }
        let accessible: HashSet<i64> = crate::Membership::visible_with_ordered_room(tx.conn(), user_id)?
            .into_iter()
            .filter(|(_, room)| !room.direct())
            .map(|(_, room)| room.id)
            .collect();
        let mut seen = HashSet::new();
        if room_ids.iter().any(|id| !accessible.contains(id) || !seen.insert(*id)) {
            return Ok(false);
        }
        let account = Self::first(tx.conn())?.or_not_found("Account")?;
        let mut settings = account.settings();
        if room_ids.is_empty() {
            settings.data.remove(DEFAULT_ROOM_ORDER);
        } else {
            // Another admin may have ordered private rooms this admin cannot see. Keep those
            // slots, replacing only the visible subset and appending newly ordered rooms.
            let mut incoming = room_ids.iter().copied();
            let mut merged = Vec::new();
            for id in settings.default_room_order() {
                if accessible.contains(&id) {
                    if let Some(next) = incoming.next() {
                        merged.push(next);
                    }
                } else {
                    merged.push(id);
                }
            }
            merged.extend(incoming);
            if merged.len() > MAX_CHANNEL_ORDER {
                return Ok(false);
            }
            settings.data.insert(DEFAULT_ROOM_ORDER.into(), serde_json::json!(merged));
        }
        // Display ordering must not invalidate account logo URLs or message fragment caches.
        tx.conn().execute_cached(r#"UPDATE "accounts" SET "settings" = ? WHERE "id" = ?"#, params![settings.to_json(), account.id])?;
        Ok(true)
    }

    /// Favorites are private to the signed-in user. Validate membership inside the same write
    /// transaction so losing access cannot race a preference update.
    pub fn set_channel_favorite(tx: &mut Tx<'_>, user_id: i64, room_id: i64, favorite: bool) -> Result<Option<Vec<i64>>> {
        let accessible: HashSet<i64> = crate::Membership::visible_with_ordered_room(tx.conn(), user_id)?
            .into_iter()
            .filter(|(_, room)| !room.direct())
            .map(|(_, room)| room.id)
            .collect();
        if !accessible.contains(&room_id) {
            return Ok(None);
        }
        let account = Self::first(tx.conn())?.or_not_found("Account")?;
        let mut settings = account.settings();
        let mut favorites = settings.favorite_channels(user_id);
        favorites.retain(|id| accessible.contains(id) && *id != room_id);
        if favorite {
            if favorites.len() >= MAX_CHANNEL_ORDER {
                return Ok(None);
            }
            favorites.push(room_id);
        }
        favorites.sort_unstable();
        favorites.dedup();
        let users = settings.data.entry(FAVORITE_CHANNELS).or_insert_with(|| Value::Object(Map::new()));
        if !users.is_object() {
            *users = Value::Object(Map::new());
        }
        let users = users.as_object_mut().expect("favorite channels are an object");
        if favorites.is_empty() {
            users.remove(&user_id.to_string());
        } else {
            users.insert(user_id.to_string(), Value::Array(favorites.iter().copied().map(Value::from).collect()));
        }
        if users.is_empty() {
            settings.data.remove(FAVORITE_CHANNELS);
        }
        // Personal favorites must not invalidate shared account/logo or message caches.
        tx.conn().execute_cached(r#"UPDATE "accounts" SET "settings" = ? WHERE "id" = ?"#, params![settings.to_json(), account.id])?;
        Ok(Some(favorites))
    }

    /// The room controller validates canonical icon names against the compiled Lucide catalog.
    /// Merge into current settings inside the write transaction and leave account caches alone.
    pub(crate) fn set_channel_icon(tx: &mut Tx<'_>, room_id: i64, icon: Option<&str>) -> Result<bool> {
        let Some(account) = Self::first(tx.conn())? else {
            return if icon.is_none() { Ok(false) } else { Err(Error::RecordNotFound("Account")) };
        };
        let mut settings = account.settings();
        if settings.channel_icon(room_id) == icon {
            return Ok(false);
        }
        let icons = settings.data.entry(CHANNEL_ICONS).or_insert_with(|| Value::Object(Map::new()));
        if !icons.is_object() {
            *icons = Value::Object(Map::new());
        }
        let icons = icons.as_object_mut().expect("channel icons are an object");
        match icon {
            Some(icon) => {
                icons.insert(room_id.to_string(), Value::String(icon.into()));
            }
            None => {
                icons.remove(&room_id.to_string());
            }
        }
        if icons.is_empty() {
            settings.data.remove(CHANNEL_ICONS);
        }
        tx.conn().execute_cached(r#"UPDATE "accounts" SET "settings" = ? WHERE "id" = ?"#, params![settings.to_json(), account.id])?;
        Ok(true)
    }
}

/// `SecureRandom.alphanumeric(12).scan(/.{4}/).join("-")`
pub fn generate_join_code() -> String {
    let code = sql::alphanumeric(12);
    format!("{}-{}-{}", &code[0..4], &code[4..8], &code[8..12])
}
