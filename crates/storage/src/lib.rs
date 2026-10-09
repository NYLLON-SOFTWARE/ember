//! Active Storage-compatible storage for Ember: blobs and their rows, the disk service and its
//! signed URLs, Marcel content type identification, analyzers, tracked variants processed with
//! libvips (as image_processing does) and video previews via ffmpeg (as VideoPreviewer does).
//!
//! Golden vectors come from `reference-tools/storage/generate.rb` (`vectors/storage.json` and
//! `vectors/storage/`).

pub mod analyze;
pub mod blob;
pub mod content_types;
pub mod disk;
pub mod disposition;
pub mod file_server;
pub mod filename;
pub mod json;
pub mod key;
pub mod marcel;
pub mod marshal;
pub mod paths;
pub mod process;
pub mod storage;
#[rustfmt::skip]
mod tables;
pub mod variation;
pub mod vips;

pub use blob::{Blob, NewBlob};
pub use disk::DiskService;
pub use filename::Filename;
pub use json::Json;
pub use storage::{Staged, Storage};
pub use variation::Variation;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("file not found")]
    FileNotFound,
    #[error("checksum mismatch")]
    Integrity,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("invalid variation: {0}")]
    InvalidVariation(String),
    #[error("can't transform blob with content_type={0}")]
    Invariable(String),
    #[error("no previewer found for content_type={0}")]
    Unpreviewable(String),
    #[error("no previewer found and can't transform blob with content_type={0}")]
    Unrepresentable(String),
    #[error("libvips: {0}")]
    Vips(String),
    #[error("{0}")]
    Preview(String),
    #[error("analysis failed: {0}")]
    Analyze(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
