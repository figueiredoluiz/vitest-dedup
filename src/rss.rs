//! Operating-system memory samples, kept in the binary to contain FFI.
//! Peak reporting and current residency admission are independent measurements.

/// Peak RSS in bytes, or `None` where the platform has no reader wired up.
#[cfg(unix)]
pub fn peak_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: `getrusage` is handed `RUSAGE_SELF` and a correctly sized, correctly aligned
    // `rusage` allocation to write into; it fully initializes that struct on success and is
    // the documented POSIX interface for this. The return value is checked before the value
    // is assumed initialized, so a failing call never reads uninitialized memory.
    let queried = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if queried != 0 {
        return None;
    }
    // SAFETY: `getrusage` returned 0, which per POSIX means it wrote a complete `rusage`.
    let usage = unsafe { usage.assume_init() };
    let max_rss = u64::try_from(usage.ru_maxrss).ok()?;
    // The one portability trap in this function, and it is a factor of 1024: Darwin reports
    // `ru_maxrss` in **bytes**, while Linux and the BSDs report **kilobytes**. Reading the
    // Linux convention on macOS would over-report by 1024x, which on the corpus this was
    // built for is the difference between "well under budget" and "impossible".
    if cfg!(any(target_os = "macos", target_os = "ios")) {
        Some(max_rss)
    } else {
        Some(max_rss.saturating_mul(1024))
    }
}

/// Peak reporting remains unavailable outside Unix; the current-residency backstop is separate.
#[cfg(not(unix))]
pub fn peak_bytes() -> Option<u64> {
    None
}

/// Read the current resident pages; procfs reports pages rather than bytes.
#[cfg(target_os = "linux")]
pub fn current_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    // SAFETY: sysconf takes a constant selector and writes no caller-owned memory.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    resident_bytes(&statm, u64::try_from(page_size).ok()?)
}

/// Convert the resident field of statm using the runtime page size, rejecting bad readings.
#[cfg(any(target_os = "linux", test))]
fn resident_bytes(statm: &str, page_size: u64) -> Option<u64> {
    if page_size == 0 {
        return None;
    }
    let mut fields = statm.split_whitespace();
    fields.next()?.parse::<u64>().ok()?;
    fields.next()?.parse::<u64>().ok()?.checked_mul(page_size)
}

/// Read the current task's resident size in bytes rather than its lifetime high-water mark.
#[cfg(target_os = "macos")]
pub fn current_bytes() -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::mach_task_basic_info>::uninit();
    let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
    // libc recommends mach2 for this binding; the stable OS call uses our existing libc edge.
    #[allow(deprecated)]
    // SAFETY: mach_task_self returns this process's borrowed task port; no ownership transfers.
    let task = unsafe { libc::mach_task_self() };
    // SAFETY: the flavor, output allocation and integer-word count all describe
    // mach_task_basic_info. Only a successful, full-length result is read below.
    let result = unsafe {
        libc::task_info(
            task,
            libc::MACH_TASK_BASIC_INFO,
            info.as_mut_ptr().cast(),
            &mut count,
        )
    };
    if result != libc::KERN_SUCCESS || count < libc::MACH_TASK_BASIC_INFO_COUNT {
        return None;
    }
    // SAFETY: task_info succeeded and returned the full requested structure.
    Some(unsafe { info.assume_init() }.resident_size)
}

/// Read this process's current working set in bytes, leaving its pseudo-handle borrowed.
#[cfg(windows)]
pub fn current_bytes() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let size = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?;
    let mut info = PROCESS_MEMORY_COUNTERS {
        cb: size,
        ..Default::default()
    };
    // SAFETY: the borrowed pseudo-handle names this process; info is a valid initialized
    // PROCESS_MEMORY_COUNTERS allocation, and size describes that same output structure.
    let success = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut info, size) };
    if success == 0 {
        None
    } else {
        u64::try_from(info.WorkingSetSize).ok()
    }
}

/// Unsupported platforms retain the accounting roof when current residency is unavailable.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn current_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The second statm field is resident pages; virtual size and peak RSS are not substitutes.
    #[test]
    fn statm_requires_a_resident_count_and_checked_page_conversion() {
        assert_eq!(resident_bytes("9000 7 3 4 5", 4096), Some(28672));
        for text in ["", "9", "x 7", "9 x", "9 -1", "9 18446744073709551615"] {
            assert_eq!(resident_bytes(text, 4096), None, "{text}");
        }
        assert_eq!(resident_bytes("9 7", 0), None);
    }

    /// A supported OS reader returns a plausible nonzero current sample for this process.
    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    fn the_current_process_has_resident_memory() {
        assert!(current_bytes().is_some_and(|bytes| bytes > 0));
    }
}
