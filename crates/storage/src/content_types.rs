//! The effective `config.active_storage` content type lists (Rails 8.2 engine defaults with
//! `load_defaults 8.2`, minus the types removed in `reference/config/initializers/vips.rb`).

/// `variable_content_types` (bmp, ico and psd removed by config/initializers/vips.rb).
pub const VARIABLE: &[&str] =
    &["image/png", "image/gif", "image/jpeg", "image/tiff", "image/webp", "image/avif", "image/heic", "image/heif"];

/// `web_image_content_types` (webp added by `load_defaults 7.2`).
pub const WEB_IMAGE: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

pub const ALLOWED_INLINE: &[&str] = &[
    "image/webp",
    "image/avif",
    "image/png",
    "image/gif",
    "image/jpeg",
    "image/tiff",
    "image/bmp",
    "image/vnd.adobe.photoshop",
    "image/vnd.microsoft.icon",
    "application/pdf",
];

pub const SERVE_AS_BINARY: &[&str] = &[
    "text/html",
    "image/svg+xml",
    "application/postscript",
    "application/x-shockwave-flash",
    "text/xml",
    "application/xml",
    "application/xhtml+xml",
    "application/mathml+xml",
    "text/cache-manifest",
];

pub const BINARY_CONTENT_TYPE: &str = "application/octet-stream";

/// `ActiveStorage.video_preview_arguments` from `load_defaults 7.0`, already shell-split.
pub const VIDEO_PREVIEW_ARGUMENTS: &[&str] =
    &["-vf", r"select=eq(n\,0)+eq(key\,1)+gt(scene\,0.015),loop=loop=-1:size=2,trim=start_frame=1", "-frames:v", "1", "-f", "image2"];

pub fn is_variable(content_type: &str) -> bool {
    VARIABLE.contains(&content_type)
}

pub fn is_web_image(content_type: &str) -> bool {
    WEB_IMAGE.contains(&content_type)
}

pub fn is_allowed_inline(content_type: &str) -> bool {
    ALLOWED_INLINE.contains(&content_type)
}

pub fn serve_as_binary(content_type: &str) -> bool {
    SERVE_AS_BINARY.contains(&content_type)
}

/// `content_type_for_serving`.
pub fn for_serving(content_type: &str) -> &str {
    if serve_as_binary(content_type) { BINARY_CONTENT_TYPE } else { content_type }
}

/// `forced_disposition_for_serving`: `Some("attachment")` for binary or non-inline types.
pub fn forced_disposition(content_type: &str) -> Option<&'static str> {
    (serve_as_binary(content_type) || !is_allowed_inline(content_type)).then_some("attachment")
}

/// `ActiveStorage.paths` is empty in Matchbox, so the binaries come from `PATH`.
pub fn ffmpeg_path() -> &'static str {
    "ffmpeg"
}

pub fn ffprobe_path() -> &'static str {
    "ffprobe"
}
