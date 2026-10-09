//! `ActiveStorage::Service::DiskService`: files at `<root>/<key[0..2]>/<key[2..4]>/<key>`, and the
//! signed disk URLs (`/rails/active_storage/disk/:encoded_key/*filename`) and upload tokens
//! (`PUT /rails/active_storage/disk/:encoded_token`).

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use rails_compat::MessageVerifier;

use crate::disposition::{content_disposition_with, escape_path, escape_segment};
use crate::filename::Filename;
use crate::json::Json;
use crate::key::checksum_file;
use crate::{Error, Result};

#[derive(Clone, Debug)]
pub struct DiskService {
    root: PathBuf,
    name: String,
}

/// The payload of a disk URL's `encoded_key` (purpose "blob_key").
#[derive(Clone, Debug, PartialEq)]
pub struct DiskKey {
    pub key: String,
    pub disposition: String,
    pub content_type: Option<String>,
    pub service_name: String,
}

/// The payload of a direct-upload `encoded_token` (purpose "blob_token").
#[derive(Clone, Debug, PartialEq)]
pub struct DiskToken {
    pub key: String,
    pub content_type: Option<String>,
    pub content_length: i64,
    pub checksum: String,
    pub service_name: String,
}

impl DiskService {
    /// Ember's `local` service: `root: Rails.root.join("storage", "files")` (config/storage.yml).
    pub fn new(root: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        Self { root: root.into(), name: name.into() }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path_for(&self, key: &str) -> PathBuf {
        self.root.join(folder_for(key)).join(key)
    }

    /// `upload(key, io, checksum:)`: write, then verify the MD5 and delete on mismatch.
    pub fn upload(&self, key: &str, mut reader: impl Read, checksum: Option<&str>) -> Result<()> {
        let path = self.make_path_for(key)?;
        let mut file = fs::File::create(&path)?;
        io::copy(&mut reader, &mut file)?;
        file.flush()?;
        drop(file);
        if let Some(checksum) = checksum {
            self.ensure_integrity_of(key, checksum)?;
        }
        Ok(())
    }

    pub fn download(&self, key: &str) -> Result<Vec<u8>> {
        fs::read(self.path_for(key)).map_err(not_found)
    }

    pub fn delete(&self, key: &str) -> Result<()> {
        match fs::remove_file(self.path_for(key)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// `delete_prefixed(prefix)`: `rm_rf` everything matching `path_for("#{prefix}*")`.
    pub fn delete_prefixed(&self, prefix: &str) -> Result<()> {
        let pattern = self.path_for(prefix);
        let pattern = pattern.to_string_lossy();
        let (dir, stem) = pattern.rsplit_once('/').unwrap_or((".", &pattern));
        let Ok(entries) = fs::read_dir(dir) else { return Ok(()) };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(stem) {
                let path = entry.path();
                if path.is_dir() { fs::remove_dir_all(path)? } else { fs::remove_file(path)? }
            }
        }
        Ok(())
    }

    pub fn exist(&self, key: &str) -> bool {
        self.path_for(key).exists()
    }

    /// The path of `service.url(key, expires_in:, filename:, content_type:, disposition:)`
    /// (the caller prefixes `ActiveStorage::Current.url_options`' protocol and host).
    pub fn url_path(
        &self,
        verifier: &MessageVerifier,
        key: &str,
        expires_at: Option<jiff::Timestamp>,
        filename: &Filename,
        content_type: Option<&str>,
        disposition: &str,
    ) -> String {
        let sanitized = filename.sanitized();
        let payload = Json::Object(vec![
            ("key".into(), key.into()),
            ("disposition".into(), content_disposition_with(disposition, &sanitized).into()),
            ("content_type".into(), content_type.map_or(Json::Null, Json::from)),
            ("service_name".into(), self.name.as_str().into()),
        ]);
        let encoded_key = verifier.generate_raw(&payload.encode(), Some("blob_key"), expires_at);
        format!("/rails/active_storage/disk/{}/{}", escape_segment(&encoded_key), escape_path(&sanitized))
    }

    /// The path of `url_for_direct_upload`.
    pub fn url_path_for_direct_upload(
        &self,
        verifier: &MessageVerifier,
        key: &str,
        expires_at: jiff::Timestamp,
        content_type: Option<&str>,
        content_length: i64,
        checksum: &str,
    ) -> String {
        let payload = Json::Object(vec![
            ("key".into(), key.into()),
            ("content_type".into(), content_type.map_or(Json::Null, Json::from)),
            ("content_length".into(), content_length.into()),
            ("checksum".into(), checksum.into()),
            ("service_name".into(), self.name.as_str().into()),
        ]);
        let token = verifier.generate_raw(&payload.encode(), Some("blob_token"), Some(expires_at));
        format!("/rails/active_storage/disk/{}", escape_segment(&token))
    }

    fn make_path_for(&self, key: &str) -> Result<PathBuf> {
        let path = self.path_for(key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(path)
    }

    fn ensure_integrity_of(&self, key: &str, checksum: &str) -> Result<()> {
        if checksum_file(&self.path_for(key))? != checksum {
            self.delete(key)?;
            return Err(Error::Integrity);
        }
        Ok(())
    }
}

/// `DiskController#decode_verified_key`.
pub fn decode_verified_key(verifier: &MessageVerifier, encoded_key: &str, now: jiff::Timestamp) -> Option<DiskKey> {
    let data = Json::parse(&verifier.verify_raw(encoded_key, Some("blob_key"), now).ok()?).ok()?;
    Some(DiskKey {
        key: data.get("key")?.as_str()?.to_string(),
        disposition: data.get("disposition")?.as_str()?.to_string(),
        content_type: data.get("content_type").and_then(Json::as_str).map(str::to_string),
        service_name: data.get("service_name")?.as_str()?.to_string(),
    })
}

/// `DiskController#decode_verified_token`.
pub fn decode_verified_token(verifier: &MessageVerifier, encoded_token: &str, now: jiff::Timestamp) -> Option<DiskToken> {
    let data = Json::parse(&verifier.verify_raw(encoded_token, Some("blob_token"), now).ok()?).ok()?;
    Some(DiskToken {
        key: data.get("key")?.as_str()?.to_string(),
        content_type: data.get("content_type").and_then(Json::as_str).map(str::to_string),
        content_length: data.get("content_length")?.as_i64()?,
        checksum: data.get("checksum")?.as_str()?.to_string(),
        service_name: data.get("service_name")?.as_str()?.to_string(),
    })
}

/// `[key[0..1], key[2..3]].join("/")`.
fn folder_for(key: &str) -> String {
    let a = key.get(0..2).unwrap_or(key);
    let b = key.get(2..4).or_else(|| key.get(2..)).unwrap_or("");
    format!("{a}/{b}")
}

fn not_found(e: io::Error) -> Error {
    if e.kind() == io::ErrorKind::NotFound { Error::FileNotFound } else { e.into() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::checksum;

    #[test]
    fn an_upload_with_a_checksum_is_verified() {
        let root = tempfile::tempdir().unwrap();
        let service = DiskService::new(root.path(), "local");
        service.upload("matching", &b"bytes"[..], Some(&checksum(b"bytes"))).unwrap();
        assert_eq!(service.download("matching").unwrap(), b"bytes");

        let mismatched = service.upload("mismatched", &b"bytes"[..], Some(&checksum(b"other bytes")));
        assert!(matches!(mismatched, Err(Error::Integrity)), "{mismatched:?}");
        assert!(!service.exist("mismatched"));

        service.upload("unchecked", &b"bytes"[..], None).unwrap();
        assert_eq!(service.download("unchecked").unwrap(), b"bytes");
    }
}
