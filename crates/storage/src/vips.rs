//! Minimal libvips FFI covering exactly the calls ruby-vips makes for Active Storage:
//! the Vips image analyzer, and `ImageProcessing::Vips` `resize_to_limit` + format conversion.
//!
//! libvips must be the same version as the reference image's for variants to be byte-identical.

#![allow(unsafe_code, reason = "the libvips FFI: every call into libvips is unsafe")]

use std::ffi::{CStr, CString, c_char, c_double, c_int, c_void};
use std::path::Path;
use std::sync::OnceLock;

use crate::{Error, Result};

#[repr(C)]
struct VipsImage {
    _private: [u8; 0],
}

const VIPS_ARGUMENT_REQUIRED: c_int = 1;
const VIPS_ARGUMENT_CONSTRUCT: c_int = 2;
const VIPS_ARGUMENT_INPUT: c_int = 16;
const VIPS_ARGUMENT_DEPRECATED: c_int = 64;
const VIPS_ACCESS_SEQUENTIAL: c_int = 1;
const VIPS_SIZE_DOWN: c_int = 2;
const VIPS_PRECISION_INTEGER: c_int = 0;
const G_TYPE_INVALID: usize = 0;

#[link(name = "vips")]
#[link(name = "gobject-2.0")]
#[link(name = "glib-2.0")]
unsafe extern "C" {
    fn vips_init(argv0: *const c_char) -> c_int;
    fn vips_version_string() -> *const c_char;
    fn vips_block_untrusted_set(state: c_int);
    fn vips_operation_block_set(name: *const c_char, state: c_int);
    fn vips_error_buffer_copy() -> *mut c_char;
    fn vips_error_clear();
    fn vips_foreign_find_load(filename: *const c_char) -> *const c_char;
    fn vips_operation_new(name: *const c_char) -> *mut c_void;
    fn vips_object_get_args(object: *mut c_void, names: *mut *const *const c_char, flags: *mut *const c_int, n_args: *mut c_int) -> c_int;
    fn vips_image_new_from_file(name: *const c_char, ...) -> *mut VipsImage;
    fn vips_image_new_matrix_from_array(width: c_int, height: c_int, array: *const c_double, size: c_int) -> *mut VipsImage;
    fn vips_image_set_double(image: *mut VipsImage, name: *const c_char, d: c_double);
    fn vips_image_get_width(image: *const VipsImage) -> c_int;
    fn vips_image_get_height(image: *const VipsImage) -> c_int;
    fn vips_image_get_typeof(image: *const VipsImage, name: *const c_char) -> usize;
    fn vips_image_get_as_string(image: *const VipsImage, name: *const c_char, out: *mut *mut c_char) -> c_int;
    fn vips_autorot(input: *mut VipsImage, out: *mut *mut VipsImage, ...) -> c_int;
    fn vips_thumbnail_image(input: *mut VipsImage, out: *mut *mut VipsImage, width: c_int, ...) -> c_int;
    fn vips_conv(input: *mut VipsImage, out: *mut *mut VipsImage, mask: *mut VipsImage, ...) -> c_int;
    fn vips_image_write_to_file(image: *mut VipsImage, name: *const c_char, ...) -> c_int;
    fn g_object_unref(object: *mut c_void);
    fn g_free(mem: *mut c_void);
}

static INIT: OnceLock<std::result::Result<(), String>> = OnceLock::new();

/// `vips_init`, then the loader restrictions of `config/initializers/vips.rb`:
/// `Vips.block_untrusted(true)` and `Vips.block("VipsForeignLoadOpenslide", true)`. Runs on first
/// use rather than at boot, so a server that never touches an image doesn't pay for libvips. A
/// failure is kept and returned every time: a second `vips_init` would report success.
pub fn init() -> Result<()> {
    // SAFETY: the OnceLock runs this once and other callers wait for it, so vips_init is never
    // entered twice; argv0 and the operation name are static NUL-terminated strings.
    INIT.get_or_init(|| unsafe {
        if vips_init(c"ember".as_ptr()) != 0 {
            return Err(format!("vips_init failed: {}", take_error()));
        }
        vips_block_untrusted_set(1);
        vips_operation_block_set(c"VipsForeignLoadOpenslide".as_ptr(), 1);
        Ok(())
    })
    .clone()
    .map_err(Error::Vips)
}

pub fn version() -> Result<String> {
    init()?;
    // SAFETY: libvips returns its version as a static NUL-terminated string.
    Ok(unsafe { CStr::from_ptr(vips_version_string()).to_string_lossy().into_owned() })
}

/// One reference to a libvips image, never null (`wrap` checks), released on drop.
pub struct Image(*mut VipsImage);

// SAFETY: GObject reference counts are atomic and libvips images are immutable once built, so an
// Image may be read or dropped on another thread.
unsafe impl Send for Image {}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: the Image owns this reference, and this is the only place it's released.
        unsafe { g_object_unref(self.0.cast()) }
    }
}

impl Image {
    fn wrap(ptr: *mut VipsImage) -> Result<Image> {
        if ptr.is_null() { Err(Error::Vips(take_error())) } else { Ok(Image(ptr)) }
    }

    fn wrap_out(status: c_int, out: *mut VipsImage) -> Result<Image> {
        if status != 0 { Err(Error::Vips(take_error())) } else { Image::wrap(out) }
    }

    /// `Vips::Image.new_from_file(path, access: :sequential)`, as the image analyzer opens files.
    pub fn open_sequential(path: &Path) -> Result<Image> {
        init()?;
        let path = cstring(path)?;
        // SAFETY: the path is NUL-terminated; the options are name/value pairs, each value of the
        // C type libvips reads for it (an enum is a c_int), ending in a NULL name.
        Image::wrap(unsafe {
            vips_image_new_from_file(path.as_ptr(), c"access".as_ptr(), VIPS_ACCESS_SEQUENTIAL, std::ptr::null::<c_char>())
        })
    }

    /// `ImageProcessing::Vips::Processor.load_image(path, page: 0)`: `page: 0` only reaches loaders
    /// that accept it (`Utils.select_valid_loader_options`), then `autorot`.
    pub fn load_for_processing(path: &Path) -> Result<Image> {
        init()?;
        let path = cstring(path)?;
        let image = if loader_accepts_page(&path) {
            // SAFETY: as in `open_sequential`; `page` is an int.
            unsafe { vips_image_new_from_file(path.as_ptr(), c"page".as_ptr(), 0 as c_int, std::ptr::null::<c_char>()) }
        } else {
            // SAFETY: the path is NUL-terminated, and the option list is only its NULL terminator.
            unsafe { vips_image_new_from_file(path.as_ptr(), std::ptr::null::<c_char>()) }
        };
        Image::wrap(image)?.autorot()
    }

    pub fn width(&self) -> i32 {
        // SAFETY: `self.0` is a live image.
        unsafe { vips_image_get_width(self.0) }
    }

    pub fn height(&self) -> i32 {
        // SAFETY: `self.0` is a live image.
        unsafe { vips_image_get_height(self.0) }
    }

    /// `image.get(name)` for string fields such as `exif-ifd0-Orientation`; `None` when absent.
    pub fn get_string(&self, name: &str) -> Option<String> {
        let name = CString::new(name).ok()?;
        // SAFETY: `self.0` is a live image and `name` is NUL-terminated. On success libvips hands
        // back a NUL-terminated string of our own, freed with g_free once copied.
        unsafe {
            if vips_image_get_typeof(self.0, name.as_ptr()) == G_TYPE_INVALID {
                return None;
            }
            let mut out: *mut c_char = std::ptr::null_mut();
            if vips_image_get_as_string(self.0, name.as_ptr(), &mut out) != 0 {
                vips_error_clear();
                return None;
            }
            let value = CStr::from_ptr(out).to_string_lossy().into_owned();
            g_free(out.cast());
            Some(value)
        }
    }

    pub fn autorot(&self) -> Result<Image> {
        let mut out = std::ptr::null_mut();
        // SAFETY: `self.0` is a live image, `out` receives a new reference on success, and the
        // option list is only its NULL terminator.
        let status = unsafe { vips_autorot(self.0, &mut out, std::ptr::null::<c_char>()) };
        Image::wrap_out(status, out)
    }

    /// `resize_to_limit(width, height)`: `thumbnail_image(width, height:, size: :down,
    /// no_rotate: true)` followed by `conv(SHARPEN_MASK, precision: :integer)`.
    pub fn resize_to_limit(&self, width: Option<i32>, height: Option<i32>) -> Result<Image> {
        const MAX_COORD: i32 = 10_000_000;
        let (width, height) = (width.unwrap_or(MAX_COORD), height.unwrap_or(MAX_COORD));
        let mut thumbnail = std::ptr::null_mut();
        // SAFETY: `self.0` is a live image and `thumbnail` receives a new reference on success. The
        // options are name/value pairs with c_int values (libvips reads int, enum and gboolean
        // arguments as int), ending in a NULL name.
        let status = unsafe {
            vips_thumbnail_image(
                self.0,
                &mut thumbnail,
                width as c_int,
                c"height".as_ptr(),
                height as c_int,
                c"size".as_ptr(),
                VIPS_SIZE_DOWN,
                c"no_rotate".as_ptr(),
                1 as c_int,
                std::ptr::null::<c_char>(),
            )
        };
        let thumbnail = Image::wrap_out(status, thumbnail)?;
        let mask = sharpen_mask()?;
        let mut sharpened = std::ptr::null_mut();
        // SAFETY: both images are live and `sharpened` receives a new reference on success;
        // `precision` is an enum passed as c_int, and the options end in a NULL name.
        let status = unsafe {
            vips_conv(thumbnail.0, &mut sharpened, mask.0, c"precision".as_ptr(), VIPS_PRECISION_INTEGER, std::ptr::null::<c_char>())
        };
        Image::wrap_out(status, sharpened)
    }

    /// `write_to_file(path)`: the saver and its defaults are picked from the extension.
    pub fn write_to_file(&self, path: &Path) -> Result<()> {
        let path = cstring(path)?;
        // SAFETY: `self.0` is a live image, the path is NUL-terminated, and the option list is only
        // its NULL terminator.
        let status = unsafe { vips_image_write_to_file(self.0, path.as_ptr(), std::ptr::null::<c_char>()) };
        if status != 0 { Err(Error::Vips(take_error())) } else { Ok(()) }
    }
}

/// `ImageProcessing::Vips::Processor::SHARPEN_MASK`: `new_from_array([[-1,-1,-1],[-1,32,-1],[-1,-1,-1]], 24)`.
fn sharpen_mask() -> Result<Image> {
    let values: [c_double; 9] = [-1.0, -1.0, -1.0, -1.0, 32.0, -1.0, -1.0, -1.0, -1.0];
    // SAFETY: `values` holds the 3 × 3 doubles libvips copies into the new image.
    let mask = Image::wrap(unsafe { vips_image_new_matrix_from_array(3, 3, values.as_ptr(), 9) })?;
    // SAFETY: `mask` is a live image and the field names are NUL-terminated.
    unsafe {
        vips_image_set_double(mask.0, c"scale".as_ptr(), 24.0);
        vips_image_set_double(mask.0, c"offset".as_ptr(), 0.0);
    }
    Ok(mask)
}

/// Whether the loader libvips picks for `path` has an optional `page` input, as
/// `Utils.select_valid_loader_options` asks ruby-vips' `Introspect`. The whole argument table is
/// read, like `Introspect`'s `vips_argument_map`: asking `vips_object_get_argument_flags` for an
/// argument the loader doesn't have (jpegload, pngload) appends "no property named `page'" to
/// libvips' error buffer.
fn loader_accepts_page(path: &CStr) -> bool {
    // SAFETY: `path` is NUL-terminated and the loader name libvips finds is static. The operation
    // is only introspected, never built, and released once. `vips_object_get_args` fills `n`
    // NUL-terminated names and their flags in arrays that belong to the operation, so they're read
    // before it's released.
    unsafe {
        let loader = vips_foreign_find_load(path.as_ptr());
        if loader.is_null() {
            vips_error_clear();
            return false;
        }
        let operation = vips_operation_new(loader);
        if operation.is_null() {
            vips_error_clear();
            return false;
        }
        let (mut names, mut flags, mut n) = (std::ptr::null(), std::ptr::null(), 0);
        let accepts = vips_object_get_args(operation, &mut names, &mut flags, &mut n) == 0
            && (0..n as usize).any(|i| CStr::from_ptr(*names.add(i)) == c"page" && optional_input(*flags.add(i)));
        g_object_unref(operation);
        accepts
    }
}

/// `Introspect#optional_input` (ruby-vips vips/operation.rb): construct-time inputs that aren't
/// required, counting deprecated required ones as optional.
fn optional_input(flags: c_int) -> bool {
    let required = flags & VIPS_ARGUMENT_REQUIRED != 0 && flags & VIPS_ARGUMENT_DEPRECATED == 0;
    flags & VIPS_ARGUMENT_CONSTRUCT != 0 && flags & VIPS_ARGUMENT_INPUT != 0 && !required
}

fn cstring(path: &Path) -> Result<CString> {
    use std::os::unix::ffi::OsStrExt;
    CString::new(path.as_os_str().as_bytes()).map_err(|_| Error::Vips("path contains a NUL byte".into()))
}

/// libvips' error buffer, taken and cleared in one step by `vips_error_buffer_copy`. The buffer is
/// shared by every thread, so the message can include another thread's error that was pending at
/// the same time (and a `vips_error_clear` on another thread can drop this one's).
fn take_error() -> String {
    // SAFETY: libvips returns a NUL-terminated copy of the buffer (or NULL), ours to g_free.
    unsafe {
        let copy = vips_error_buffer_copy();
        if copy.is_null() {
            return String::new();
        }
        let message = CStr::from_ptr(copy).to_string_lossy().trim_end().to_string();
        g_free(copy.cast());
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_goes_only_to_loaders_that_take_it() {
        let moon = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/test/fixtures/files/moon.jpg");
        let image = Image::load_for_processing(&moon).unwrap().resize_to_limit(Some(8), Some(8)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        for (format, accepts) in [("jpg", false), ("png", false), ("gif", true), ("webp", true)] {
            let path = dir.path().join(format!("moon.{format}"));
            image.write_to_file(&path).unwrap();
            assert_eq!(loader_accepts_page(&cstring(&path).unwrap()), accepts, "{format}");
        }
    }
}
