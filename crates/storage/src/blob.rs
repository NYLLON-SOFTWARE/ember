//! `active_storage_blobs`, `active_storage_attachments` and `active_storage_variant_records` rows.
//!
//! Every function takes a `rusqlite::Connection` (a `Transaction` derefs to one), so callers
//! decide the transaction boundaries the way the Rails models' callbacks would.

use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::filename::Filename;
use crate::json::Json;
use crate::key::{checksum, checksum_file, generate_key};
use crate::{Result, content_types, marcel};

#[derive(Clone, Debug, PartialEq)]
pub struct Blob {
    pub id: i64,
    pub key: String,
    pub filename: Filename,
    pub content_type: Option<String>,
    /// The `metadata` column, an ordered JSON object (`store :metadata, coder: JSON`).
    pub metadata: Json,
    pub service_name: String,
    pub byte_size: i64,
    pub checksum: Option<String>,
    pub created_at: String,
}

/// A blob built from uploaded bytes, not yet saved (`Blob.build_after_unfurling`).
#[derive(Clone, Debug, PartialEq)]
pub struct NewBlob {
    pub key: String,
    pub filename: Filename,
    pub content_type: Option<String>,
    pub metadata: Json,
    pub service_name: String,
    pub byte_size: i64,
    pub checksum: String,
}

impl NewBlob {
    /// `build_after_unfurling(io:, filename:, content_type:, identify:)`: generates the key,
    /// computes the checksum, identifies the content type with Marcel (unless a declared type
    /// is given with `identify: false`) and marks the blob `identified`.
    pub fn unfurl(data: &[u8], filename: Filename, declared_type: Option<&str>, service_name: &str, identify: bool) -> NewBlob {
        let content_type = Self::content_type(data, &filename, declared_type, identify);
        Self::build(filename, content_type, service_name, data.len() as u64, checksum(data))
    }

    /// [`NewBlob::unfurl`] for a file, reading only as much of it as identification needs, and
    /// streaming it through the checksum.
    pub fn unfurl_file(
        path: &Path,
        filename: Filename,
        declared_type: Option<&str>,
        service_name: &str,
        identify: bool,
    ) -> Result<NewBlob> {
        let mut head = Vec::new();
        std::fs::File::open(path)?.take(marcel::magic_prefix_len() as u64).read_to_end(&mut head)?;
        let content_type = Self::content_type(&head, &filename, declared_type, identify);
        let byte_size = std::fs::metadata(path)?.len();
        Ok(Self::build(filename, content_type, service_name, byte_size, checksum_file(path)?))
    }

    fn content_type(head: &[u8], filename: &Filename, declared_type: Option<&str>, identify: bool) -> Option<String> {
        if declared_type.is_none() || identify {
            Some(marcel::identify(head, Some(&filename.sanitized()), declared_type))
        } else {
            declared_type.map(str::to_string)
        }
    }

    fn build(filename: Filename, content_type: Option<String>, service_name: &str, byte_size: u64, checksum: String) -> NewBlob {
        NewBlob {
            key: generate_key(),
            filename,
            content_type,
            metadata: Json::Object(vec![("identified".into(), Json::Bool(true))]),
            service_name: service_name.to_string(),
            byte_size: byte_size as i64,
            checksum,
        }
    }

    pub fn insert(self, conn: &Connection, created_at: jiff::Timestamp) -> Result<Blob> {
        let created_at = format_timestamp(created_at);
        conn.execute(
            "INSERT INTO active_storage_blobs (key, filename, content_type, metadata, service_name, byte_size, checksum, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                self.key,
                self.filename.raw(),
                self.content_type,
                self.metadata.encode(),
                self.service_name,
                self.byte_size,
                self.checksum,
                created_at
            ],
        )?;
        Ok(Blob {
            id: conn.last_insert_rowid(),
            key: self.key,
            filename: self.filename,
            content_type: self.content_type,
            metadata: self.metadata,
            service_name: self.service_name,
            byte_size: self.byte_size,
            checksum: Some(self.checksum),
            created_at,
        })
    }
}

const BLOB_COLUMNS: &str = "id, key, filename, content_type, metadata, service_name, byte_size, checksum, created_at";

impl Blob {
    fn from_row(row: &Row) -> rusqlite::Result<Blob> {
        let metadata: Option<String> = row.get(4)?;
        Ok(Blob {
            id: row.get(0)?,
            key: row.get(1)?,
            filename: Filename::new(row.get::<_, String>(2)?),
            content_type: row.get(3)?,
            metadata: metadata.and_then(|m| Json::parse(&m).ok()).unwrap_or_else(Json::object),
            service_name: row.get(5)?,
            byte_size: row.get(6)?,
            checksum: row.get(7)?,
            created_at: row.get(8)?,
        })
    }

    pub fn find(conn: &Connection, id: i64) -> Result<Option<Blob>> {
        static SQL: LazyLock<String> = LazyLock::new(|| format!("SELECT {BLOB_COLUMNS} FROM active_storage_blobs WHERE id = ?1"));
        Ok(conn.prepare_cached(&SQL)?.query_row([id], Blob::from_row).optional()?)
    }

    pub fn find_by_key(conn: &Connection, key: &str) -> Result<Option<Blob>> {
        static SQL: LazyLock<String> = LazyLock::new(|| format!("SELECT {BLOB_COLUMNS} FROM active_storage_blobs WHERE key = ?1"));
        Ok(conn.prepare_cached(&SQL)?.query_row([key], Blob::from_row).optional()?)
    }

    /// The blob attached to `record` under `name` (`has_one_attached`).
    pub fn attached(conn: &Connection, record_type: &str, record_id: i64, name: &str) -> Result<Option<Blob>> {
        static SQL: LazyLock<String> = LazyLock::new(|| {
            format!(
                "SELECT {} FROM active_storage_blobs b JOIN active_storage_attachments a ON a.blob_id = b.id \
                 WHERE a.record_type = ?1 AND a.record_id = ?2 AND a.name = ?3 ORDER BY a.id LIMIT 1",
                BLOB_COLUMNS.split(", ").map(|c| format!("b.{c}")).collect::<Vec<_>>().join(", ")
            )
        });
        Ok(conn.prepare_cached(&SQL)?.query_row(params![record_type, record_id, name], Blob::from_row).optional()?)
    }

    pub fn content_type(&self) -> &str {
        self.content_type.as_deref().unwrap_or("")
    }

    pub fn is_image(&self) -> bool {
        self.content_type().starts_with("image")
    }

    pub fn is_video(&self) -> bool {
        self.content_type().starts_with("video")
    }

    pub fn is_audio(&self) -> bool {
        self.content_type().starts_with("audio")
    }

    /// `variable?`
    pub fn is_variable(&self) -> bool {
        content_types::is_variable(self.content_type())
    }

    /// `previewable?`: only the video previewer can accept in Matchbox's image (no poppler or
    /// mupdf binaries are installed, so the PDF previewers never accept).
    pub fn is_previewable(&self) -> bool {
        self.is_video() && crate::process::ffmpeg_exists()
    }

    /// `representable?`
    pub fn is_representable(&self) -> bool {
        self.is_variable() || self.is_previewable()
    }

    pub fn is_analyzed(&self) -> bool {
        self.metadata.get("analyzed").is_some_and(|v| !matches!(v, Json::Null | Json::Bool(false)))
    }

    /// `metadata[:width]`/`[:height]` as the views read them.
    pub fn dimension(&self, name: &str) -> Option<f64> {
        match self.metadata.get(name)? {
            Json::Int(i) => Some(*i as f64),
            Json::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// `update!(metadata:)`.
    pub fn update_metadata(&mut self, conn: &Connection, metadata: Json) -> Result<()> {
        conn.execute("UPDATE active_storage_blobs SET metadata = ?1 WHERE id = ?2", params![metadata.encode(), self.id])?;
        self.metadata = metadata;
        Ok(())
    }

    /// `default_variant_format`: web images keep their format, everything else becomes PNG.
    pub fn default_variant_format(&self) -> String {
        if content_types::is_web_image(self.content_type()) { self.format().unwrap_or_else(|| "png".into()) } else { "png".into() }
    }

    /// `Representable#format`: the filename's extension when Marcel agrees it names the
    /// content type, otherwise the content type's first registered extension.
    fn format(&self) -> Option<String> {
        let extension = self.filename.extension();
        if !extension.is_empty() && marcel::for_extension(extension) == self.content_type() {
            Some(extension.to_string())
        } else {
            marcel::extensions(self.content_type()).first().map(|e| e.to_string())
        }
    }

    /// `delete`: the file, plus any legacy untracked variants under `variants/<key>/`.
    pub fn delete_files(&self, service: &crate::DiskService) -> Result<()> {
        service.delete(&self.key)?;
        if self.is_image() {
            service.delete_prefixed(&format!("variants/{}/", self.key))?;
        }
        Ok(())
    }
}

/// Inserts an `active_storage_attachments` row. Rails then touches the record (and whatever it
/// touches in turn, e.g. `Message belongs_to :room, touch: true`); that's up to the caller.
pub fn insert_attachment(
    conn: &Connection,
    name: &str,
    record_type: &str,
    record_id: i64,
    blob_id: i64,
    created_at: jiff::Timestamp,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO active_storage_attachments (name, record_type, record_id, blob_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![name, record_type, record_id, blob_id, format_timestamp(created_at)],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The `(record_type, record_id)` of every attachment of a blob: the records `Blob#touch_attachments`
/// touches after the blob is updated (e.g. by analysis).
pub fn attachment_records(conn: &Connection, blob_id: i64) -> Result<Vec<(String, i64)>> {
    let mut statement = conn.prepare("SELECT record_type, record_id FROM active_storage_attachments WHERE blob_id = ?1 ORDER BY id")?;
    let rows = statement.query_map([blob_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `blob.variant_records.find_by(variation_digest:)`.
pub fn find_variant_record(conn: &Connection, blob_id: i64, variation_digest: &str) -> Result<Option<i64>> {
    Ok(conn
        .prepare_cached("SELECT id FROM active_storage_variant_records WHERE blob_id = ?1 AND variation_digest = ?2")?
        .query_row(params![blob_id, variation_digest], |row| row.get(0))
        .optional()?)
}

/// `INSERT` half of `create_or_find_by!`: `None` when another writer created it first.
pub fn insert_variant_record(conn: &Connection, blob_id: i64, variation_digest: &str) -> Result<Option<i64>> {
    match conn.execute(
        "INSERT INTO active_storage_variant_records (blob_id, variation_digest) VALUES (?1, ?2)",
        params![blob_id, variation_digest],
    ) {
        Ok(_) => Ok(Some(conn.last_insert_rowid())),
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Active Record's SQLite datetime format: UTC, microseconds only when non-zero.
pub fn format_timestamp(t: jiff::Timestamp) -> String {
    let t = jiff::Timestamp::from_microsecond(t.as_microsecond()).unwrap_or(t);
    let micros = t.subsec_microsecond();
    let base = t.strftime("%Y-%m-%d %H:%M:%S").to_string();
    if micros == 0 { base } else { format!("{base}.{micros:06}") }
}
