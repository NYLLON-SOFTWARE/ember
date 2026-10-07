//! The Active Storage flows Matchbox drives, over one disk service and the app verifier:
//! uploads (`create_and_upload!`), analysis, tracked variants (`VariantWithRecord`) and video
//! previews (`ActiveStorage::Preview`).
//!
//! Each flow is split in two, so the slow part never holds the database's writer: the file work
//! (copying, checksumming, libvips, ffmpeg, analysis) takes no connection and is blocking, so
//! call it from a blocking task; it leaves a [`Staged`] blob whose file is already uploaded. The
//! record step then saves rows inside a transaction and is quick. The `&Connection` methods that
//! do both at once are for tests and tools.

use std::path::Path;

use rails_compat::MessageVerifier;
use rusqlite::Connection;
use tempfile::NamedTempFile;

use crate::analyze::Analyzer;
use crate::blob::{self, Blob, NewBlob};
use crate::disk::DiskService;
use crate::filename::Filename;
use crate::json::Json;
use crate::key::checksum_file;
use crate::marshal::Value;
use crate::process;
use crate::variation::Variation;
use crate::{Error, Result};

pub struct Storage {
    pub service: DiskService,
    /// `ActiveStorage.verifier`: `rails_compat::app_verifier(secrets, "ActiveStorage")`. It signs
    /// blob ids (purpose "blob_id": `ActiveStorage::Blob` overrides both the signed-id verifier
    /// and `combine_signed_id_purposes`, so this is *not* the Active Record signed-id scheme),
    /// variation keys ("variation"), disk URLs ("blob_key") and direct-upload tokens
    /// ("blob_token"). The data goes in and comes out as encoded JSON, because key order is part
    /// of the signed bytes (e.g. `{key:, disposition:, content_type:, service_name:}`).
    pub verifier: MessageVerifier,
}

/// A new blob whose file is already in the service but whose row isn't saved yet. Dropping it
/// deletes the file, so a write that rolls back (or never saves the row) leaves no orphan behind;
/// [`Staged::keep`] it once the row is committed.
#[derive(Debug)]
pub struct Staged {
    blob: NewBlob,
    service: DiskService,
    kept: bool,
}

impl Staged {
    pub fn blob(&self) -> &NewBlob {
        &self.blob
    }

    /// Inserts the blob's row.
    pub fn insert(&self, conn: &Connection, now: jiff::Timestamp) -> Result<Blob> {
        self.blob.clone().insert(conn, now)
    }

    /// The row is saved for good: keep the file.
    pub fn keep(mut self) {
        self.kept = true;
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.kept {
            let _ = self.service.delete(&self.blob.key);
        }
    }
}

impl Storage {
    pub fn new(service: DiskService, verifier: MessageVerifier) -> Self {
        Self { service, verifier }
    }

    // --- File work: no connection, blocking -------------------------------------------------------

    /// The file half of `Blob.create_and_upload!(io:, filename:, content_type:)` as attaching an
    /// uploaded file does (`identify: true`): unfurls the file at `source` and uploads it.
    pub fn stage_file(&self, source: &Path, filename: Filename, declared_type: Option<&str>) -> Result<Staged> {
        let blob = NewBlob::unfurl_file(source, filename, declared_type, self.service.name(), true)?;
        self.stage(blob, std::fs::File::open(source)?)
    }

    /// [`Self::stage_file`] for bytes in memory.
    pub fn stage_bytes(&self, data: &[u8], filename: Filename, declared_type: Option<&str>) -> Result<Staged> {
        let blob = NewBlob::unfurl(data, filename, declared_type, self.service.name(), true);
        self.stage(blob, data)
    }

    /// Copies the blob's bytes into the service. Unlike `DiskService#upload`, the copy isn't read
    /// back to verify its checksum: the checksum was just computed from these same local bytes, so
    /// reading the copy back would only compare them with themselves, and [`Self::open`] still
    /// verifies the file before it's analyzed or made into a variant or poster. The `Staged` comes
    /// first, so a failed copy deletes what it wrote.
    fn stage(&self, blob: NewBlob, reader: impl std::io::Read) -> Result<Staged> {
        let staged = Staged { blob, service: self.service.clone(), kept: false };
        self.service.upload(&staged.blob.key, reader, None)?;
        Ok(staged)
    }

    /// `blob.open`: a tempfile named `ActiveStorage-<id>-…<.ext>`, checksum-verified.
    pub fn open(&self, blob: &Blob) -> Result<NamedTempFile> {
        let file = tempfile::Builder::new()
            .prefix(&format!("ActiveStorage-{}-", blob.id))
            .suffix(blob.filename.extension_with_delimiter())
            .tempfile()?;
        std::fs::copy(self.service.path_for(&blob.key), file.path())
            .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { Error::FileNotFound } else { e.into() })?;
        if let Some(checksum) = &blob.checksum
            && &checksum_file(file.path())? != checksum
        {
            return Err(Error::Integrity);
        }
        Ok(file)
    }

    /// What `blob.analyze` saves: `metadata.merge(analyzer.metadata.merge(analyzed: true))`.
    pub fn analyzed_metadata(&self, blob: &Blob) -> Result<Json> {
        let analyzer = Analyzer::for_content_type(blob.content_type());
        let extracted = match analyzer {
            Analyzer::Null => Json::object(),
            _ => analyzer.metadata(self.open(blob)?.path())?,
        };
        Ok(analyzed(&blob.metadata, extracted))
    }

    /// The file half of `VariantWithRecord#processed` for an already-defaulted variation (see
    /// [`Self::variation_for`]): transforms the blob and stages the variant image, already
    /// analyzed (Rails analyzes it in an `AnalyzeJob` after commit).
    pub fn transform_variant(&self, blob: &Blob, variation: &Variation) -> Result<Staged> {
        let output = {
            let input = self.open(blob)?;
            process::transform(input.path(), variation)?
        };
        let filename = Filename::new(format!("{}.{}", blob.filename.base(), variation.format()?.to_lowercase()));
        self.stage_analyzed(output.path(), filename, &variation.content_type()?)
    }

    /// The file half of `Preview#process`: draws the video's frame with ffmpeg and stages it as
    /// `<base>.jpg` (`image/jpeg`), already analyzed.
    pub fn draw_preview_image(&self, blob: &Blob) -> Result<Staged> {
        if !blob.is_previewable() {
            return Err(Error::Unpreviewable(blob.content_type().to_string()));
        }
        let frame = {
            let input = self.open(blob)?;
            process::video_preview(input.path())?
        };
        let mut output = tempfile::Builder::new().prefix("ActiveStorage-").suffix(".jpg").tempfile()?;
        std::io::Write::write_all(&mut output, &frame)?;
        self.stage_analyzed(output.path(), Filename::new(format!("{}.jpg", blob.filename.base())), "image/jpeg")
    }

    /// Stages a generated image with the metadata its analysis would save.
    fn stage_analyzed(&self, path: &Path, filename: Filename, content_type: &str) -> Result<Staged> {
        let mut staged = self.stage_file(path, filename, Some(content_type))?;
        let analyzer = Analyzer::for_content_type(staged.blob.content_type.as_deref().unwrap_or(""));
        staged.blob.metadata = analyzed(&staged.blob.metadata, analyzer.metadata(path)?);
        Ok(staged)
    }

    // --- Record work: quick, inside the caller's transaction -------------------------------------

    /// `blob.variant(transformations)`: the variation defaulted to the blob's variant format.
    pub fn variation_for(&self, blob: &Blob, transformations: &Variation) -> Result<Variation> {
        if !blob.is_variable() {
            return Err(Error::Invariable(blob.content_type().to_string()));
        }
        Ok(transformations.default_to(&[("format".into(), Value::Str(blob.default_variant_format()))]))
    }

    /// The processed variant's image blob, if `variant_records` already has it.
    pub fn existing_variant(&self, conn: &Connection, blob: &Blob, variation: &Variation) -> Result<Option<Blob>> {
        match blob::find_variant_record(conn, blob.id, &variation.digest())? {
            Some(record_id) => Blob::attached(conn, "ActiveStorage::VariantRecord", record_id, "image"),
            None => Ok(None),
        }
    }

    /// The record half of `VariantWithRecord#processed`: the variant record, its image blob and
    /// attachment. `None` when another request recorded the variant first; that one is then
    /// [`Self::existing_variant`], and `image` should be dropped.
    pub fn record_variant(
        &self,
        conn: &Connection,
        blob: &Blob,
        variation: &Variation,
        image: &Staged,
        now: jiff::Timestamp,
    ) -> Result<Option<Blob>> {
        let Some(record_id) = blob::insert_variant_record(conn, blob.id, &variation.digest())? else { return Ok(None) };
        let image = image.insert(conn, now)?;
        blob::insert_attachment(conn, "image", "ActiveStorage::VariantRecord", record_id, image.id, now)?;
        Ok(Some(image))
    }

    /// `blob.preview_image`, if it has been generated.
    pub fn existing_preview_image(&self, conn: &Connection, blob: &Blob) -> Result<Option<Blob>> {
        Blob::attached(conn, "ActiveStorage::Blob", blob.id, "preview_image")
    }

    /// The record half of `Preview#process`: attaches the frame as the blob's `preview_image`.
    /// `None` when another request attached one first; `image` should then be dropped.
    pub fn record_preview_image(&self, conn: &Connection, blob: &Blob, image: &Staged, now: jiff::Timestamp) -> Result<Option<Blob>> {
        if self.existing_preview_image(conn, blob)?.is_some() {
            return Ok(None);
        }
        let image = image.insert(conn, now)?;
        blob::insert_attachment(conn, "preview_image", "ActiveStorage::Blob", blob.id, image.id, now)?;
        Ok(Some(image))
    }

    // --- Both at once, for tests and tools ---------------------------------------------------------

    /// `Blob.create_and_upload!(io:, filename:, content_type:)`.
    pub fn create_and_upload(
        &self,
        conn: &Connection,
        data: &[u8],
        filename: Filename,
        declared_type: Option<&str>,
        now: jiff::Timestamp,
    ) -> Result<Blob> {
        let staged = self.stage_bytes(data, filename, declared_type)?;
        let blob = staged.insert(conn, now)?;
        staged.keep();
        Ok(blob)
    }

    /// `blob.analyze`: `update!(metadata: metadata.merge(analyzer.metadata.merge(analyzed: true)))`.
    /// Rails then touches the blob's attachment records (see [`blob::attachment_records`]).
    pub fn analyze(&self, conn: &Connection, blob: &mut Blob) -> Result<()> {
        let metadata = self.analyzed_metadata(blob)?;
        blob.update_metadata(conn, metadata)
    }

    /// `VariantWithRecord#processed`: reuses the variant record when present, otherwise
    /// transforms the blob and records the variant.
    pub fn process_variant(&self, conn: &Connection, blob: &Blob, variation: &Variation, now: jiff::Timestamp) -> Result<Blob> {
        if let Some(image) = self.existing_variant(conn, blob, variation)? {
            return Ok(image);
        }
        let image = self.transform_variant(blob, variation)?;
        match self.record_variant(conn, blob, variation, &image, now)? {
            Some(recorded) => {
                image.keep();
                Ok(recorded)
            }
            None => self.existing_variant(conn, blob, variation)?.ok_or(Error::FileNotFound),
        }
    }

    /// `blob.preview_image`, generating it with ffmpeg when missing (`Preview#process`).
    pub fn preview_image(&self, conn: &Connection, blob: &Blob, now: jiff::Timestamp) -> Result<Blob> {
        if let Some(image) = self.existing_preview_image(conn, blob)? {
            return Ok(image);
        }
        let image = self.draw_preview_image(blob)?;
        match self.record_preview_image(conn, blob, &image, now)? {
            Some(recorded) => {
                image.keep();
                Ok(recorded)
            }
            None => self.existing_preview_image(conn, blob)?.ok_or(Error::FileNotFound),
        }
    }

    /// `blob.preview(transformations).processed`, returning the blob to serve: the preview image
    /// itself for empty transformations, otherwise its processed variant.
    pub fn process_preview(&self, conn: &Connection, blob: &Blob, transformations: &Variation, now: jiff::Timestamp) -> Result<Blob> {
        let image = self.preview_image(conn, blob, now)?;
        if transformations.is_empty() {
            return Ok(image);
        }
        let variation = self.variation_for(&image, transformations)?;
        self.process_variant(conn, &image, &variation, now)
    }

    /// `blob.representation(transformations).processed`: a preview for previewable blobs, a
    /// variant for variable ones.
    pub fn process_representation(
        &self,
        conn: &Connection,
        blob: &Blob,
        transformations: &Variation,
        now: jiff::Timestamp,
    ) -> Result<Blob> {
        if blob.is_previewable() {
            self.process_preview(conn, blob, transformations, now)
        } else if blob.is_variable() {
            let variation = self.variation_for(blob, transformations)?;
            self.process_variant(conn, blob, &variation, now)
        } else {
            Err(Error::Unrepresentable(blob.content_type().to_string()))
        }
    }

    pub fn path_for(&self, blob: &Blob) -> std::path::PathBuf {
        self.service.path_for(&blob.key)
    }

    /// Deletes the blob's files (`Blob#delete`); rows are the caller's.
    pub fn delete_files(&self, blob: &Blob) -> Result<()> {
        blob.delete_files(&self.service)
    }
}

/// `metadata.merge(extracted.merge(analyzed: true))`
fn analyzed(metadata: &Json, mut extracted: Json) -> Json {
    extracted.set("analyzed", Json::Bool(true));
    let mut metadata = metadata.clone();
    metadata.merge(&extracted);
    metadata
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    struct Broken;

    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("the disk went away"))
        }
    }

    #[test]
    fn a_copy_that_fails_leaves_no_file_behind() {
        let root = tempfile::tempdir().unwrap();
        let verifier = rails_compat::app_verifier(&rails_compat::Secrets::new("test"), "ActiveStorage");
        let storage = Storage::new(DiskService::new(root.path(), "local"), verifier);
        let blob = NewBlob::unfurl(b"partial and more", Filename::new("a.txt"), None, "local", true);
        let key = blob.key.clone();

        let Err(Error::Io(error)) = storage.stage(blob, b"partial".chain(Broken)) else { panic!("a broken copy was staged") };
        assert_eq!(error.to_string(), "the disk went away");
        assert!(!storage.service.exist(&key));
    }
}
