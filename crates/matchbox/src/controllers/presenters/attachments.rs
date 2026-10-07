//! `has_one_attached` as `User::Avatar` (`:avatar`) and `Account` (`:logo`) use it, over
//! `matchbox_storage` (reference/app/models/user/avatar.rb, account.rb, and Active Storage's
//! `Attached::Changes::CreateOne` / `Attachment`).
//!
//! Assigning an uploaded file and saving the record, in the record's transaction:
//! the old attachment is destroyed first (`has_one ... dependent: :destroy` replacing its target;
//! its blob is purged after commit, `dependent: :purge_later`), then the new blob and attachment
//! rows are inserted, and each attachment change touches the record
//! (`belongs_to :record, touch: true`). Since a fresh blob isn't analyzed, `ActiveStorage::AnalyzeJob`
//! runs after commit (analysis touches the record again).
//!
//! Unlike Rails, which uploads after commit, the file is uploaded before the transaction
//! ([`Assignment::stage`]), so the writer never waits on copying and checksumming it; a
//! transaction that rolls back deletes it again.

use std::sync::Arc;

use matchbox_db::{CachedStatements, Connection, Event, Tx};
use matchbox_kit::{Param, Result, UploadedFile};
use matchbox_storage::{Blob, Filename, Staged, Variation};
use rusqlite::OptionalExtension;

use crate::active_storage::{analyzed_metadata, keep_after_commit, stage_file, storage_error};
use crate::app::App;

/// An uploaded file (`ActionDispatch::Http::UploadedFile`), still in its multipart tempfile.
#[derive(Debug, Clone)]
pub struct Upload {
    pub file: Arc<UploadedFile>,
    pub filename: String,
    pub content_type: Option<String>,
}

impl Upload {
    /// The upload in `param`, if it is one. `""` and nil mean "no change" to the controllers
    /// here (`params.permit(...).compact` / `avatar=` with nil or "" deletes, see [`Assignment`]).
    pub fn from_param(param: Option<&Param>) -> Option<Upload> {
        let file = param.and_then(Param::as_file)?;
        Some(Upload { file: file.clone(), filename: file.original_filename.clone(), content_type: file.content_type.clone() })
    }

    /// Uploads the file to storage for a blob whose row is saved next, off the async threads.
    pub async fn stage(self, app: &App) -> Result<Staged> {
        let path = self.file.path().to_path_buf();
        stage_file(app, path, Filename::new(self.filename), self.content_type).await
    }
}

/// What `record.avatar = value` does with a permitted param value: `Create` holds the [`Upload`],
/// then, once staged, the [`Staged`] blob.
#[derive(Debug, Clone)]
pub enum Assignment<U = Upload> {
    /// The key wasn't given.
    Unchanged,
    /// `nil` or `""`: `Attached::Changes::DeleteOne` (the attachment is destroyed on save).
    Delete,
    /// An uploaded file: `Attached::Changes::CreateOne`.
    Create(U),
    /// Anything else (e.g. a plain string that isn't a signed blob id): Rails raises.
    Invalid,
}

impl Assignment {
    pub fn from_params(params: &matchbox_kit::ParamMap, key: &str) -> Result<Assignment> {
        if !params.contains_key(key) {
            return Ok(Assignment::Unchanged);
        }
        match params.get(key) {
            None => Ok(Assignment::Delete),
            Some(param) if param.is_null() || param.as_str() == Some("") => Ok(Assignment::Delete),
            Some(param) => Ok(Upload::from_param(Some(param)).map_or(Assignment::Invalid, Assignment::Create)),
        }
    }

    /// Uploads a new file, so the save only has rows to write.
    pub async fn stage(self, app: &App) -> Result<Assignment<Staged>> {
        Ok(match self {
            Assignment::Unchanged => Assignment::Unchanged,
            Assignment::Delete => Assignment::Delete,
            Assignment::Create(upload) => Assignment::Create(upload.stage(app).await?),
            Assignment::Invalid => Assignment::Invalid,
        })
    }
}

/// A blob inserted for an attachment, to analyze after commit.
#[derive(Debug)]
pub struct Pending {
    pub blob: Blob,
}

/// `record.<name>.attached?`'s blob: the attachment's blob, if any.
pub fn attached_blob(conn: &Connection, record_type: &str, record_id: i64, name: &str) -> matchbox_db::Result<Option<Blob>> {
    Blob::attached(conn, record_type, record_id, name).map_err(storage_error)
}

/// Applies an assignment inside the record's save. Returns the blob to analyze after commit.
pub fn assign(tx: &mut Tx<'_>, record: Record, name: &str, assignment: Assignment<Staged>) -> matchbox_db::Result<Option<Pending>> {
    match assignment {
        Assignment::Unchanged => Ok(None),
        Assignment::Delete => {
            destroy(tx, record, name)?;
            Ok(None)
        }
        Assignment::Create(staged) => attach(tx, record, name, staged).map(Some),
        Assignment::Invalid => Err(matchbox_db::Error::other("Could not find or build blob: expected attachable")),
    }
}

/// The record an attachment belongs to: its polymorphic type and its table.
#[derive(Debug, Clone, Copy)]
pub struct Record {
    pub record_type: &'static str,
    pub table: &'static str,
    pub id: i64,
}

impl Record {
    pub fn user(id: i64) -> Self {
        Self { record_type: "User", table: "users", id }
    }

    pub fn account(id: i64) -> Self {
        Self { record_type: "Account", table: "accounts", id }
    }
}

/// `record.<name> = uploaded_file; record.save`: replaces any current attachment.
pub fn attach(tx: &mut Tx<'_>, record: Record, name: &str, staged: Staged) -> matchbox_db::Result<Pending> {
    destroy(tx, record, name)?;
    let now = tx.now();
    let blob = staged.insert(tx.conn(), now.jiff()).map_err(storage_error)?;
    keep_after_commit(tx, staged);
    matchbox_storage::blob::insert_attachment(tx.conn(), name, record.record_type, record.id, blob.id, now.jiff())
        .map_err(storage_error)?;
    super::accounts::touch(tx.conn(), record.table, record.id, tx.now())?;
    Ok(Pending { blob })
}

/// `record.<name>.destroy` (the attachment): delete it, touch the record, and purge its blob
/// after commit. Nothing happens without an attachment (`delegate_missing_to :attachment, allow_nil: true`).
pub fn destroy(tx: &mut Tx<'_>, record: Record, name: &str) -> matchbox_db::Result<bool> {
    let attachment: Option<(i64, i64)> = tx
        .conn()
        .query_row_cached(
            "SELECT id, blob_id FROM active_storage_attachments WHERE record_type = ?1 AND record_id = ?2 AND name = ?3 LIMIT 1",
            rusqlite::params![record.record_type, record.id, name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((attachment_id, blob_id)) = attachment else { return Ok(false) };
    tx.conn().execute_cached("DELETE FROM active_storage_attachments WHERE id = ?1", [attachment_id])?;
    super::accounts::touch(tx.conn(), record.table, record.id, tx.now())?;
    tx.emit_after_commit(Event::PurgeBlob { blob_id });
    Ok(true)
}

/// After commit: `analyze_blob_later` (the file was uploaded before the save).
pub fn analyze_later(app: &App, pending: Option<Pending>) {
    let Some(Pending { blob }) = pending else { return };
    let job_app = app.clone();
    app.jobs.perform_later("ActiveStorage::AnalyzeJob", async move { analyze(&job_app, blob.id).await });
}

/// `ActiveStorage::AnalyzeJob`: `blob.analyze`, then `touch_attachment_records`. The file is
/// analyzed off the writer; only the metadata update and touches run on it.
pub async fn analyze(app: &App, blob_id: i64) -> anyhow::Result<()> {
    let blob = app.db.read(move |conn| Blob::find(conn, blob_id).map_err(storage_error)).await?;
    let Some(blob) = blob else { return Ok(()) };
    let metadata = analyzed_metadata(app, &blob).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
    app.db
        .write(move |tx| {
            let mut blob = blob;
            blob.update_metadata(tx.conn(), metadata).map_err(storage_error)?;
            for (record_type, record_id) in matchbox_storage::blob::attachment_records(tx.conn(), blob_id).map_err(storage_error)? {
                if let Some(table) = table_for(&record_type) {
                    super::accounts::touch(tx.conn(), table, record_id, tx.now())?;
                }
            }
            Ok(())
        })
        .await?;
    Ok(())
}

fn table_for(record_type: &str) -> Option<&'static str> {
    match record_type {
        "User" => Some("users"),
        "Account" => Some("accounts"),
        "Message" => Some("messages"),
        _ => None,
    }
}

/// `record.<name>.variant(name).processed if record.<name>.variable?`: the processed variant's
/// blob, or `None` when there's no attachment or it can't be transformed.
pub async fn processed_variant(app: &App, record: Record, name: &str, transformations: Variation) -> Result<Option<Blob>> {
    let name = name.to_string();
    let blob = app.read(move |conn| attached_blob(conn, record.record_type, record.id, &name)).await?;
    let Some(blob) = blob.filter(Blob::is_variable) else { return Ok(None) };
    crate::active_storage::processed_representation(app, blob, transformations).await.map(Some)
}
