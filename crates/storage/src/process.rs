//! Producing variant and preview bytes: `ActiveStorage::Transformers::Vips` (image_processing
//! 1.14) and `ActiveStorage::Previewer::VideoPreviewer`.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tempfile::NamedTempFile;

use crate::content_types::{VIDEO_PREVIEW_ARGUMENTS, ffmpeg_path};
use crate::marshal::Value;
use crate::variation::Variation;
use crate::vips::Image;
use crate::{Error, Result};

/// How long ffmpeg may take to draw a preview frame before it's killed. Rails sets no limit, but
/// a crafted or hour-long video shouldn't hold a processing thread indefinitely.
pub const FFMPEG_TIMEOUT: Duration = Duration::from_secs(60);

/// `variation.transform(file)`: `ImageProcessing::Vips.source(file).loader(page: 0)
/// .convert(format).apply(operations).call`, saved to a tempfile named for the format.
pub fn transform(input: &Path, variation: &Variation) -> Result<NamedTempFile> {
    let format = variation.format()?;
    let operations = operations(variation)?;

    let mut image = Image::load_for_processing(input)?;
    for (width, height) in operations {
        image = image.resize_to_limit(width, height)?;
    }

    let output = tempfile::Builder::new().prefix("image_processing").suffix(&format!(".{format}")).tempfile()?;
    image.write_to_file(output.path())?;
    Ok(output)
}

/// `ImageProcessingTransformer#operations`: every transformation except `format`, skipping blank
/// arguments. Only `resize_to_limit` is implemented: it's the only one Matchbox defines.
fn operations(variation: &Variation) -> Result<Vec<(Option<i32>, Option<i32>)>> {
    let mut operations = Vec::new();
    for (name, argument) in variation.transformations() {
        match (name.as_str(), argument) {
            ("format", _) => {}
            ("combine_options", _) => {
                return Err(Error::InvalidVariation("combine_options is not supported".into()));
            }
            (_, argument) if blank(argument) => {}
            ("resize_to_limit", Value::Array(args)) if args.len() == 2 => {
                let dimension = |v: Option<&Value>| match v {
                    None | Some(Value::Nil) => Ok(None),
                    Some(Value::Int(n)) => {
                        i32::try_from(*n).map(Some).map_err(|_| Error::InvalidVariation(format!("resize_to_limit argument {n}")))
                    }
                    Some(other) => Err(Error::InvalidVariation(format!("resize_to_limit argument {other:?}"))),
                };
                let (width, height) = (dimension(args.first())?, dimension(args.get(1))?);
                if width.is_none() && height.is_none() {
                    return Err(Error::InvalidVariation("either width or height must be specified".into()));
                }
                operations.push((width, height));
            }
            (name, argument) => {
                return Err(Error::InvalidVariation(format!("unsupported transformation {name}: {argument:?}")));
            }
        }
    }
    Ok(operations)
}

/// `Object#present?` negated, for the values a transformation can hold.
fn blank(value: &Value) -> bool {
    match value {
        Value::Nil | Value::Bool(false) => true,
        Value::Str(s) => s.trim().is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Hash(entries) => entries.is_empty(),
        _ => false,
    }
}

/// `VideoPreviewer.accept?`: `system(ffmpeg, "-version")`, memoized.
pub fn ffmpeg_exists() -> bool {
    static EXISTS: OnceLock<bool> = OnceLock::new();
    *EXISTS.get_or_init(|| {
        Command::new(ffmpeg_path()).arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    })
}

/// `draw_relevant_frame_from`: `ffmpeg -i <input> <video_preview_arguments> -`, capturing stdout.
pub fn video_preview(input: &Path) -> Result<Vec<u8>> {
    let mut command = Command::new(ffmpeg_path());
    command.arg("-i").arg(input).args(VIDEO_PREVIEW_ARGUMENTS).arg("-").stderr(Stdio::piped());
    let output = output_within(&mut command, FFMPEG_TIMEOUT).map_err(|error| match error.kind() {
        std::io::ErrorKind::TimedOut => Error::Preview(format!("{} {error}", ffmpeg_path())),
        _ => error.into(),
    })?;
    if !output.status.success() {
        return Err(Error::Preview(format!(
            "{} failed (status {}): {}",
            ffmpeg_path(),
            output.status.code().map_or("nil".into(), |c| c.to_string()),
            String::from_utf8_lossy(&output.stderr).trim_end()
        )));
    }
    Ok(output.stdout)
}

/// `command.output()`, except that the child is killed (and reaped) once `timeout` passes, which
/// is an `ErrorKind::TimedOut` error. Stdin is closed and stdout captured; stderr is captured
/// only when the caller pipes it.
pub fn output_within(command: &mut Command, timeout: Duration) -> std::io::Result<Output> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).spawn()?;
    // Drain the pipes while waiting, so a chatty child can't stall on a full pipe.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let Some(status) = wait_until(&mut child, Instant::now() + timeout)? else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, format!("timed out after {:?}", timeout)));
    };
    Ok(Output { status, stdout: stdout.join().unwrap_or_default(), stderr: stderr.join().unwrap_or_default() })
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    })
}

/// The child's exit status, or `None` while it's still running at `deadline`.
fn wait_until(child: &mut Child, deadline: Instant) -> std::io::Result<Option<ExitStatus>> {
    let mut pause = Duration::from_millis(1);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        std::thread::sleep(pause.min(deadline - now));
        pause = next_pause(pause);
    }
}

/// The longest `wait_until` sleeps between checks. `try_wait` is one `waitpid(WNOHANG)`, so polling
/// often is cheap and keeps the return close to the exit: a child that exits between checks is
/// noticed up to this much later.
const MAX_PAUSE: Duration = Duration::from_millis(5);

fn next_pause(pause: Duration) -> Duration {
    (pause * 2).min(MAX_PAUSE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/test/fixtures/files").join(name)
    }

    fn thumbnail(input: &Path, size: i64, format: &str) -> Result<NamedTempFile> {
        transform(input, &Variation::resize_to_limit(size, size, Some(format)))
    }

    #[test]
    fn a_failing_variant_reports_its_own_error_after_many_variants() {
        // libvips' error buffer is process-wide and holds 10 KB. Probing JPEG and PNG loaders for
        // `page` used to append "no property named `page'" to it on every variant, until it was
        // full and the next real error was cut off.
        let jpeg = thumbnail(&fixture("moon.jpg"), 8, "jpg").unwrap();
        let png = thumbnail(jpeg.path(), 8, "png").unwrap();
        for _ in 0..200 {
            thumbnail(jpeg.path(), 4, "webp").unwrap();
            thumbnail(png.path(), 4, "webp").unwrap();
        }

        let corrupt = tempfile::Builder::new().suffix(".jpg").tempfile().unwrap();
        std::fs::write(corrupt.path(), b"\xFF\xD8\xFF\xE0 not really a JPEG").unwrap();
        let Err(Error::Vips(message)) = thumbnail(corrupt.path(), 4, "webp") else { panic!("a corrupt JPEG made a variant") };
        assert!(message.contains("JPEG datastream contains no image"), "{message}");
        assert!(!message.contains("no property named"), "{message}");
    }

    #[test]
    fn resize_arguments_must_fit_libvips() {
        let too_wide = Variation::resize_to_limit(i64::from(i32::MAX) + 1, 100, None);
        assert!(matches!(operations(&too_wide), Err(Error::InvalidVariation(_))));
        let widest = Variation::resize_to_limit(i64::from(i32::MAX), 100, None);
        assert_eq!(operations(&widest).unwrap(), [(Some(i32::MAX), Some(100))]);
    }

    #[test]
    fn output_within_captures_a_quick_child() {
        let mut command = Command::new("sh");
        command.args(["-c", "echo out; echo err >&2"]).stderr(Stdio::piped());
        let output = output_within(&mut command, Duration::from_secs(10)).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"out\n");
        assert_eq!(output.stderr, b"err\n");
    }

    #[test]
    fn the_pause_between_checks_doubles_up_to_five_milliseconds() {
        let pauses = std::iter::successors(Some(Duration::from_millis(1)), |&pause| Some(next_pause(pause)));
        assert_eq!(pauses.take(6).map(|pause| pause.as_millis()).collect::<Vec<_>>(), [1, 2, 4, 5, 5, 5]);
    }

    #[test]
    fn output_within_returns_soon_after_the_child_exits() {
        // With pauses growing to 50 ms, a child that exited at 35 ms was noticed at 63 ms.
        let run = |within: bool| {
            let mut command = Command::new("sleep");
            command.arg("0.035");
            let started = Instant::now();
            let output = if within { output_within(&mut command, Duration::from_secs(10)) } else { command.output() };
            assert!(output.unwrap().status.success());
            started.elapsed()
        };
        // The fastest of a few runs each, so a busy machine doesn't fail it.
        let (mut plain, mut within) = (Duration::MAX, Duration::MAX);
        for _ in 0..5 {
            plain = plain.min(run(false));
            within = within.min(run(true));
        }
        assert!(within < plain + Duration::from_millis(15), "output() {plain:?}, output_within {within:?}");
    }

    #[test]
    fn output_within_kills_a_child_that_overruns() {
        let started = Instant::now();
        let pid_file = tempfile::NamedTempFile::new().unwrap();
        let mut command = Command::new("sh");
        command.arg("-c").arg(format!("echo $$ > {}; exec sleep 30", pid_file.path().display()));
        let error = output_within(&mut command, Duration::from_millis(300)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
        let pid = std::fs::read_to_string(pid_file.path()).unwrap();
        assert!(!Path::new(&format!("/proc/{}", pid.trim())).exists(), "the child is still running");
    }
}
