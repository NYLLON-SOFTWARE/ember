//! Rows of `active_storage_blobs` and `active_storage_attachments`, for the models that
//! have attachments. Uploading, analysis and variants belong to `ember_storage`.

use rusqlite::{Connection, Row, params};

use crate::database::Tx;
use crate::error::{OptionalExt, Result};
use crate::sql::{CachedStatements, query_one};
use crate::time::Timestamp;

#[derive(Debug, Clone, PartialEq)]
pub struct Blob {
    pub id: i64,
    pub key: String,
    pub filename: String,
    pub content_type: Option<String>,
    /// JSON text, e.g. `{"identified":true,"width":800,"height":600,"analyzed":true}`.
    pub metadata: Option<String>,
    pub service_name: String,
    pub byte_size: i64,
    pub checksum: Option<String>,
    pub created_at: Timestamp,
}

impl Blob {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            key: row.get("key")?,
            filename: row.get("filename")?,
            content_type: row.get("content_type")?,
            metadata: row.get("metadata")?,
            service_name: row.get("service_name")?,
            byte_size: row.get("byte_size")?,
            checksum: row.get("checksum")?,
            created_at: row.get("created_at")?,
        })
    }

    pub fn find(conn: &Connection, id: i64) -> Result<Self> {
        query_one(conn, r#"SELECT * FROM "active_storage_blobs" WHERE "active_storage_blobs"."id" = ? LIMIT 1"#, [id], Self::from_row)?
            .or_not_found("ActiveStorage::Blob")
    }

    /// Inserts a blob row; `self.id` and `created_at` are ignored and assigned.
    pub fn create(tx: &Tx<'_>, blob: &Blob) -> Result<Self> {
        let now = tx.now();
        let id: i64 = tx.conn().query_row_cached(
            r#"INSERT INTO "active_storage_blobs" ("byte_size", "checksum", "content_type", "created_at", "filename", "key", "metadata", "service_name") VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING "id""#,
            params![blob.byte_size, blob.checksum, blob.content_type, now, blob.filename, blob.key, blob.metadata, blob.service_name],
            |r| r.get(0),
        )?;
        Ok(Self { id, created_at: now, ..blob.clone() })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub id: i64,
    pub name: String,
    pub record_type: String,
    pub record_id: i64,
    pub blob_id: i64,
    pub created_at: Timestamp,
}

impl Attachment {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            name: row.get("name")?,
            record_type: row.get("record_type")?,
            record_id: row.get("record_id")?,
            blob_id: row.get("blob_id")?,
            created_at: row.get("created_at")?,
        })
    }

    /// `has_one_attached`'s lookup.
    pub fn find_for(conn: &Connection, record_type: &str, record_id: i64, name: &str) -> Result<Option<Self>> {
        query_one(
            conn,
            r#"SELECT * FROM "active_storage_attachments" WHERE "active_storage_attachments"."record_id" = ? AND "active_storage_attachments"."record_type" = ? AND "active_storage_attachments"."name" = ? LIMIT 1"#,
            params![record_id, record_type, name],
            Self::from_row,
        )
    }

    pub fn create(tx: &Tx<'_>, record_type: &str, record_id: i64, name: &str, blob_id: i64) -> Result<Self> {
        let now = tx.now();
        let id: i64 = tx.conn().query_row_cached(
            r#"INSERT INTO "active_storage_attachments" ("blob_id", "created_at", "name", "record_id", "record_type") VALUES (?, ?, ?, ?, ?) RETURNING "id""#,
            params![blob_id, now, name, record_id, record_type],
            |r| r.get(0),
        )?;
        Ok(Self { id, name: name.into(), record_type: record_type.into(), record_id, blob_id, created_at: now })
    }

    pub fn delete(&self, tx: &Tx<'_>) -> Result<()> {
        tx.conn().execute_cached(r#"DELETE FROM "active_storage_attachments" WHERE "active_storage_attachments"."id" = ?"#, [self.id])?;
        Ok(())
    }

    pub fn blob(&self, conn: &Connection) -> Result<Blob> {
        Blob::find(conn, self.blob_id)
    }
}
