//! The Active Storage analyzers Matchbox runs, in `config.active_storage.analyzers` order:
//! `ImageAnalyzer::Vips`, `VideoAnalyzer` and `AudioAnalyzer` (ImageMagick never accepts,
//! because the variant processor is `:vips`), falling back to `NullAnalyzer`.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::json::Json;
use crate::process::output_within;
use crate::vips::Image;
use crate::{Error, Result, content_types};

/// How long ffprobe may take to read a file's streams before it's killed. Rails sets no limit.
pub const FFPROBE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Analyzer {
    Image,
    Video,
    Audio,
    Null,
}

impl Analyzer {
    pub fn for_content_type(content_type: &str) -> Analyzer {
        if content_type.starts_with("image") {
            Analyzer::Image
        } else if content_type.starts_with("video") {
            Analyzer::Video
        } else if content_type.starts_with("audio") {
            Analyzer::Audio
        } else {
            Analyzer::Null
        }
    }

    /// `analyze_later?`: only the null analyzer runs inline from `analyze_later`.
    pub fn analyze_later(self) -> bool {
        self != Analyzer::Null
    }

    /// `analyzer.metadata` for a local copy of the blob.
    pub fn metadata(self, path: &Path) -> Result<Json> {
        match self {
            Analyzer::Image => Ok(image_metadata(path)),
            Analyzer::Video => Ok(video_metadata(&probe(path)?)?),
            Analyzer::Audio => Ok(audio_metadata(&probe(path)?)?),
            Analyzer::Null => Ok(Json::object()),
        }
    }
}

/// `ImageAnalyzer::Vips#metadata`: dimensions, swapped for EXIF orientations that rotate by 90°.
/// Files libvips can't read yield `{}`.
fn image_metadata(path: &Path) -> Json {
    let Ok(image) = Image::open_sequential(path) else { return Json::object() };
    let rotated = image
        .get_string("exif-ifd0-Orientation")
        .is_some_and(|o| ["Right-top", "Left-bottom", "Top-right", "Bottom-left"].iter().any(|r| o.contains(r)));
    let (width, height) = if rotated { (image.height(), image.width()) } else { (image.width(), image.height()) };
    Json::Object(vec![("width".into(), Json::Int(width as i64)), ("height".into(), Json::Int(height as i64))])
}

/// `ffprobe -print_format json -show_streams -show_format -v error <path>`; `{}` without ffprobe.
fn probe(path: &Path) -> Result<Json> {
    let mut command = Command::new(content_types::ffprobe_path());
    command.args(["-print_format", "json", "-show_streams", "-show_format", "-v", "error"]).arg(path).stderr(Stdio::inherit());
    let output = match output_within(&mut command, FFPROBE_TIMEOUT) {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Json::object()),
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return Err(Error::Analyze(format!("ffprobe {e}"))),
        Err(e) => return Err(e.into()),
    };
    Json::parse(&String::from_utf8_lossy(&output.stdout)).map_err(|e| Error::Analyze(format!("ffprobe output: {e}")))
}

fn streams(probe: &Json) -> &[Json] {
    match probe.get("streams") {
        Some(Json::Array(streams)) => streams,
        _ => &[],
    }
}

fn stream<'a>(probe: &'a Json, codec_type: &str) -> Option<&'a Json> {
    streams(probe).iter().find(|s| s.get("codec_type").and_then(Json::as_str) == Some(codec_type))
}

fn present(stream: Option<&Json>) -> bool {
    matches!(stream, Some(Json::Object(entries)) if !entries.is_empty())
}

/// Ruby's `Float(value)` for the numbers and numeric strings ffprobe emits.
fn ruby_float(value: &Json) -> Result<f64> {
    match value {
        Json::Int(i) => Ok(*i as f64),
        Json::Float(f) => Ok(*f),
        Json::String(s) => s.trim().parse().map_err(|_| Error::Analyze(format!("invalid value for Float(): {s:?}"))),
        other => Err(Error::Analyze(format!("can't convert {other:?} into Float"))),
    }
}

/// Ruby's `Integer(value)`.
fn ruby_integer(value: &Json) -> Result<i64> {
    match value {
        Json::Int(i) => Ok(*i),
        Json::Float(f) if f.is_finite() => Ok(f.trunc() as i64),
        Json::String(s) => s.trim().parse().map_err(|_| Error::Analyze(format!("invalid value for Integer(): {s:?}"))),
        other => Err(Error::Analyze(format!("can't convert {other:?} into Integer"))),
    }
}

/// `VideoAnalyzer#metadata`: `{ width:, height:, duration:, angle:, display_aspect_ratio:,
/// audio:, video: }.compact`.
fn video_metadata(probe: &Json) -> Result<Json> {
    let video = stream(probe, "video");
    let audio = stream(probe, "audio");
    let field = |name: &str| video.and_then(|s| s.get(name)).filter(|v| **v != Json::Null);

    let angle = match video.and_then(|s| s.get("tags")).and_then(|t| t.get("rotate")) {
        Some(rotate) => Some(ruby_integer(rotate)?),
        None => {
            let display_matrix = match field("side_data_list") {
                Some(Json::Array(list)) => list.iter().find(|d| d.get("side_data_type").and_then(Json::as_str) == Some("Display Matrix")),
                _ => None,
            };
            match display_matrix.and_then(|d| d.get("rotation")).filter(|r| **r != Json::Null) {
                Some(rotation) => Some(ruby_integer(rotation)?),
                None => None,
            }
        }
    };

    let display_aspect_ratio = match field("display_aspect_ratio") {
        Some(descriptor) => {
            let descriptor = descriptor.as_str().ok_or_else(|| Error::Analyze("display_aspect_ratio".into()))?;
            let mut terms = descriptor.splitn(2, ':');
            let numerator = ruby_integer(&Json::from(terms.next().unwrap_or("")))?;
            let denominator = match terms.next() {
                Some(term) => ruby_integer(&Json::from(term))?,
                None => return Err(Error::Analyze("can't convert nil into Integer".into())),
            };
            (numerator != 0).then_some((numerator, denominator))
        }
        None => None,
    };

    let encoded_width = field("width").map(ruby_float).transpose()?;
    let encoded_height = field("height").map(ruby_float).transpose()?;
    let display_height_scale = display_aspect_ratio.map(|(n, d)| d as f64 / n as f64);
    let computed_height = match (encoded_width, display_height_scale) {
        (Some(width), Some(scale)) => Some(width * scale),
        _ => None,
    };
    let rotated = matches!(angle, Some(90 | 270 | -90 | -270));
    let (width, height) =
        if rotated { (computed_height.or(encoded_height), encoded_width) } else { (encoded_width, computed_height.or(encoded_height)) };

    let duration = match field("duration").or_else(|| probe.get("format").and_then(|f| f.get("duration"))) {
        Some(d) if *d != Json::Null => Some(ruby_float(d)?),
        _ => None,
    };

    let mut metadata = Vec::new();
    if let Some(width) = width {
        metadata.push(("width".to_string(), Json::Float(width)));
    }
    if let Some(height) = height {
        metadata.push(("height".to_string(), Json::Float(height)));
    }
    if let Some(duration) = duration {
        metadata.push(("duration".to_string(), Json::Float(duration)));
    }
    if let Some(angle) = angle {
        metadata.push(("angle".to_string(), Json::Int(angle)));
    }
    if let Some((n, d)) = display_aspect_ratio {
        metadata.push(("display_aspect_ratio".to_string(), Json::Array(vec![Json::Int(n), Json::Int(d)])));
    }
    metadata.push(("audio".to_string(), Json::Bool(present(audio))));
    metadata.push(("video".to_string(), Json::Bool(present(video))));
    Ok(Json::Object(metadata))
}

/// `AudioAnalyzer#metadata`: `{ duration:, bit_rate:, sample_rate:, tags: }.compact`.
fn audio_metadata(probe: &Json) -> Result<Json> {
    let audio = stream(probe, "audio");
    let field = |name: &str| audio.and_then(|s| s.get(name)).filter(|v| **v != Json::Null);
    let mut metadata = Vec::new();
    if let Some(duration) = field("duration") {
        metadata.push(("duration".to_string(), Json::Float(ruby_float(duration)?)));
    }
    if let Some(bit_rate) = field("bit_rate") {
        metadata.push(("bit_rate".to_string(), Json::Int(ruby_integer(bit_rate)?)));
    }
    if let Some(sample_rate) = field("sample_rate") {
        metadata.push(("sample_rate".to_string(), Json::Int(ruby_integer(sample_rate)?)));
    }
    if let Some(tags) = field("tags") {
        metadata.push(("tags".to_string(), tags.clone()));
    }
    Ok(Json::Object(metadata))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_metadata_matches_the_analyzer() {
        let probe = Json::parse(
            r#"{"streams":[{"codec_type":"video","width":320,"height":180,"display_aspect_ratio":"16:9","duration":"65.840000"}],"format":{"duration":"65.9"}}"#,
        )
        .unwrap();
        assert_eq!(
            video_metadata(&probe).unwrap().encode(),
            r#"{"width":320.0,"height":180.0,"duration":65.84,"display_aspect_ratio":[16,9],"audio":false,"video":true}"#
        );
    }

    #[test]
    fn rotated_video_swaps_dimensions() {
        let probe = Json::parse(
            r#"{"streams":[{"codec_type":"video","width":1920,"height":1080,"side_data_list":[{"side_data_type":"Display Matrix","rotation":-90}]},{"codec_type":"audio"}],"format":{"duration":"3.5"}}"#,
        )
        .unwrap();
        assert_eq!(
            video_metadata(&probe).unwrap().encode(),
            r#"{"width":1080.0,"height":1920.0,"duration":3.5,"angle":-90,"audio":true,"video":true}"#
        );
    }
}
