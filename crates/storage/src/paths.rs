//! Active Storage route helpers (`activestorage/config/routes.rb`) and blob signed ids.
//!
//! `rails_blob_path` and `url_for(representation)` resolve through
//! `resolve_model_to_route = :rails_storage_redirect`; `rails_storage_proxy_path` gives the
//! proxy variants. `urls_expire_in` is unset in Matchbox, so these signed ids never expire.

use rails_compat::MessageVerifier;

use crate::blob::Blob;
use crate::disposition::{escape_path, escape_segment};
use crate::filename::Filename;
use crate::json::Json;
use crate::variation::Variation;

pub const PREFIX: &str = "/rails/active_storage";

/// `blob.signed_id`: the Active Storage verifier with purpose "blob_id".
pub fn signed_blob_id(verifier: &MessageVerifier, blob_id: i64, expires_at: Option<jiff::Timestamp>) -> String {
    verifier.generate_raw(&blob_id.to_string(), Some("blob_id"), expires_at)
}

/// `ActiveStorage::Blob.find_signed(id)`'s verification half.
pub fn verify_signed_blob_id(verifier: &MessageVerifier, signed_id: &str, now: jiff::Timestamp) -> Option<i64> {
    Json::parse(&verifier.verify_raw(signed_id, Some("blob_id"), now).ok()?).ok()?.as_i64()
}

/// `rails_blob_path(blob, disposition:)` → `/rails/active_storage/blobs/redirect/:signed_id/*filename`.
pub fn blob_redirect_path(verifier: &MessageVerifier, blob: &Blob, disposition: Option<&str>) -> String {
    blob_path("redirect", verifier, blob.id, &blob.filename, disposition)
}

/// `rails_storage_proxy_path(blob)` → `/rails/active_storage/blobs/proxy/:signed_id/*filename`.
pub fn blob_proxy_path(verifier: &MessageVerifier, blob: &Blob, disposition: Option<&str>) -> String {
    blob_path("proxy", verifier, blob.id, &blob.filename, disposition)
}

/// `url_for(blob.representation(...))`/`url_for(variant)`/`url_for(preview)` → the path of
/// `/rails/active_storage/representations/redirect/:signed_blob_id/:variation_key/*filename`.
/// `blob` is the *original* blob (the video for previews), `variation` is exactly the one the
/// representation holds (defaulted for variants, as given for previews).
pub fn representation_redirect_path(verifier: &MessageVerifier, blob: &Blob, variation: &Variation) -> String {
    representation_path("redirect", verifier, blob, variation)
}

pub fn representation_proxy_path(verifier: &MessageVerifier, blob: &Blob, variation: &Variation) -> String {
    representation_path("proxy", verifier, blob, variation)
}

fn blob_path(kind: &str, verifier: &MessageVerifier, blob_id: i64, filename: &Filename, disposition: Option<&str>) -> String {
    let mut path = format!(
        "{PREFIX}/blobs/{kind}/{}/{}",
        escape_segment(&signed_blob_id(verifier, blob_id, None)),
        escape_path(&filename.sanitized())
    );
    if let Some(disposition) = disposition {
        // `Hash#to_query`, which escapes with `CGI.escape`.
        path.push_str("?disposition=");
        path.push_str(&ruby_compat::cgi_escape(disposition));
    }
    path
}

fn representation_path(kind: &str, verifier: &MessageVerifier, blob: &Blob, variation: &Variation) -> String {
    format!(
        "{PREFIX}/representations/{kind}/{}/{}/{}",
        escape_segment(&signed_blob_id(verifier, blob.id, None)),
        escape_segment(&variation.key(verifier)),
        escape_path(&blob.filename.sanitized())
    )
}
