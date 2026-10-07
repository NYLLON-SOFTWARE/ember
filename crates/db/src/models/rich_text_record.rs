//! `ActionText::RichText` rows (`action_text_rich_texts`). Matchbox has one: `Message#body`.

use rusqlite::{Connection, params};

use crate::database::Tx;
use crate::error::Result;
use crate::sql::{CachedStatements, columns, query_one};
use crate::time::Timestamp;

#[derive(Debug, Clone, PartialEq)]
pub struct RichTextRecord {
    pub id: i64,
    pub name: String,
    pub body: Option<String>,
    pub record_type: String,
    pub record_id: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

columns! {
    RichTextRecord, "action_text_rich_texts", rich_text_record_columns {
        id: "id",
        name: "name",
        body: "body",
        record_type: "record_type",
        record_id: "record_id",
        created_at: "created_at",
        updated_at: "updated_at",
    }
}

impl RichTextRecord {
    pub fn find_for(conn: &Connection, record_type: &str, record_id: i64, name: &str) -> Result<Option<Self>> {
        query_one(
            conn,
            concat!(
                "SELECT ",
                rich_text_record_columns!(),
                r#" FROM "action_text_rich_texts" WHERE "action_text_rich_texts"."record_id" = ? AND "action_text_rich_texts"."record_type" = ? AND "action_text_rich_texts"."name" = ? LIMIT 1"#
            ),
            params![record_id, record_type, name],
            Self::from_row,
        )
    }

    pub fn create(tx: &Tx<'_>, record_type: &str, record_id: i64, name: &str, body: &str) -> Result<Self> {
        let now = tx.now();
        let id: i64 = tx.conn().query_row_cached(
            r#"INSERT INTO "action_text_rich_texts" ("body", "created_at", "name", "record_id", "record_type", "updated_at") VALUES (?, ?, ?, ?, ?, ?) RETURNING "id""#,
            params![body, now, name, record_id, record_type, now],
            |r| r.get(0),
        )?;
        Ok(Self {
            id,
            name: name.into(),
            body: Some(body.into()),
            record_type: record_type.into(),
            record_id,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn update_body(&mut self, tx: &Tx<'_>, body: &str) -> Result<()> {
        let now = tx.now();
        tx.conn().execute_cached(
            r#"UPDATE "action_text_rich_texts" SET "body" = ?, "updated_at" = ? WHERE "action_text_rich_texts"."id" = ?"#,
            params![body, now, self.id],
        )?;
        self.body = Some(body.into());
        self.updated_at = now;
        Ok(())
    }

    pub fn delete(&self, tx: &Tx<'_>) -> Result<()> {
        tx.conn().execute_cached(r#"DELETE FROM "action_text_rich_texts" WHERE "action_text_rich_texts"."id" = ?"#, [self.id])?;
        Ok(())
    }
}
