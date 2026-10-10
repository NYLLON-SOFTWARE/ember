// The Ember server: controllers, channels, jobs and integrations wired over the crates.

mod active_storage;
mod admin;
mod app;
mod channels;
mod concerns;
mod config;
mod controllers;
mod integrations;
mod jobs;
mod rich_text;
#[cfg(test)]
mod test_support;

/// jemalloc: the room page alone makes thousands of allocations per request, across the runtime
/// workers, the database's writer and reader threads, and the blocking pool.
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// jemalloc's options (`malloc_conf`, under tikv-jemallocator's `_rjem_` prefix), read when it
/// starts, before `main`: no transparent huge pages for its regions either (see
/// [`disable_transparent_huge_pages`]). jemalloc declares it `const char *`, so it's a thin pointer,
/// declared as tikv-jemalloc-sys does.
#[cfg(target_os = "linux")]
#[allow(unsafe_code, reason = "jemalloc reads its options from an exported symbol")]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static JEMALLOC_CONF: Option<&'static std::ffi::c_char> =
    // SAFETY: points at the first byte of a static, NUL-terminated string.
    Some(unsafe { &*c"thp:never".as_ptr() });

fn main() -> anyhow::Result<()> {
    disable_transparent_huge_pages();
    app::run()
}

/// On kernels with transparent huge pages set to `always` (Debian's and Arch's default), every
/// thread's 2 MB stack and each of jemalloc's regions get backed by whole 2 MB pages as soon as
/// they're touched: an idle server took 160 MB on 32 cores instead of 15 MB. Nothing here is big
/// enough to gain from huge pages, so the process (and ffmpeg, which inherits it) opts out.
#[allow(unsafe_code, reason = "prctl has no safe wrapper in std")]
fn disable_transparent_huge_pages() {
    #[cfg(target_os = "linux")]
    {
        let off: libc::c_ulong = 1;
        // SAFETY: PR_SET_THP_DISABLE takes machine-width integer arguments and only changes this
        // process's memory policy.
        if unsafe { libc::prctl(libc::PR_SET_THP_DISABLE, off, 0 as libc::c_ulong, 0 as libc::c_ulong, 0 as libc::c_ulong) } != 0 {
            eprintln!("couldn't disable transparent huge pages: {}", std::io::Error::last_os_error());
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
#[allow(unsafe_code, reason = "reads the settings back through prctl and mallctl")]
mod tests {
    #[test]
    fn transparent_huge_pages_are_disabled() {
        super::disable_transparent_huge_pages();
        let zero: libc::c_ulong = 0;
        // SAFETY: as in `disable_transparent_huge_pages`.
        assert_eq!(unsafe { libc::prctl(libc::PR_GET_THP_DISABLE, zero, zero, zero, zero) }, 1);
    }

    #[test]
    fn jemalloc_reads_its_options() {
        let mut thp: *const std::ffi::c_char = std::ptr::null();
        let mut len = std::mem::size_of_val(&thp);
        // SAFETY: `opt.thp` is a `const char *`, written into a variable of that type and size.
        let status = unsafe { tikv_jemalloc_sys::mallctl(c"opt.thp".as_ptr(), (&raw mut thp).cast(), &mut len, std::ptr::null_mut(), 0) };
        assert_eq!(status, 0);
        // SAFETY: jemalloc returned a pointer to one of its static option names.
        assert_eq!(unsafe { std::ffi::CStr::from_ptr(thp) }, c"never");
    }
}
