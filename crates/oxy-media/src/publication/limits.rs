use std::time::Duration;

const GIB: u64 = 1024 * 1024 * 1024;

/// Registry payload memory only; protocol response copies have a separate cap.
pub fn resource_memory_budget(total_memory: Option<u64>) -> usize {
    usize::try_from(total_memory.map_or(GIB, |total| (total / 8).max(GIB))).unwrap_or(usize::MAX)
}

#[derive(Clone, Copy, Debug)]
pub struct ResourceRegistryLimits {
    pub max_entries: usize,
    pub max_encoded_bytes: usize,
    pub max_materialized_responses: usize,
    pub max_materialized_bytes: usize,
    pub publish_grace: Duration,
    pub ui_lease: Duration,
}

impl ResourceRegistryLimits {
    /// Explicit payload limits, also used by tests without probing system RAM.
    pub fn new(max_entries: usize, max_encoded_bytes: usize) -> Self {
        Self {
            max_entries,
            max_encoded_bytes,
            max_materialized_responses: 4,
            max_materialized_bytes: 128 * 1024 * 1024,
            publish_grace: Duration::from_secs(5),
            ui_lease: Duration::from_secs(30),
        }
    }

    pub(super) fn production() -> Self {
        Self::new(512, resource_memory_budget(system_total_memory()))
    }
}

#[cfg(target_os = "macos")]
fn system_total_memory() -> Option<u64> {
    let mut bytes = 0_u64;
    let mut len = std::mem::size_of::<u64>();
    // SAFETY: the name is NUL terminated, and output and size point to valid
    // writable storage of the declared size. No sysctl value is changed.
    let result = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&mut bytes as *mut u64).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (result == 0 && len == std::mem::size_of::<u64>()).then_some(bytes)
}

#[cfg(target_os = "linux")]
fn system_total_memory() -> Option<u64> {
    // SAFETY: sysconf has no pointer arguments and only queries system values.
    let (pages, page_size) = unsafe {
        (
            libc::sysconf(libc::_SC_PHYS_PAGES),
            libc::sysconf(libc::_SC_PAGESIZE),
        )
    };
    u64::try_from(pages)
        .ok()?
        .checked_mul(u64::try_from(page_size).ok()?)
}

#[cfg(target_os = "windows")]
fn system_total_memory() -> Option<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: u32::try_from(std::mem::size_of::<MEMORYSTATUSEX>()).ok()?,
        ..Default::default()
    };
    // SAFETY: status is initialized with the API-required size and remains valid.
    unsafe { GlobalMemoryStatusEx(&mut status).ok()? };
    Some(status.ullTotalPhys)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn system_total_memory() -> Option<u64> {
    None
}
