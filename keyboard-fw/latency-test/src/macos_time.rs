//! High-resolution host clock. `mach_absolute_time` is the same time base the
//! IOKit HID layer stamps input reports with, so converting both through the
//! one timebase keeps the HID timestamp and our serial-arrival timestamps on a
//! single monotonic nanosecond timeline.

use std::sync::LazyLock;

#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

extern "C" {
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> libc::c_int;
    fn mach_absolute_time() -> u64;
}

// (numer, denom): nanoseconds = mach_ticks * numer / denom. Fixed for the life
// of the process, so it lives with the static.
static TIMEBASE: LazyLock<(u64, u64)> = LazyLock::new(|| {
    let mut tb = MachTimebaseInfo { numer: 0, denom: 0 };
    unsafe { mach_timebase_info(&mut tb) };
    if tb.numer == 0 || tb.denom == 0 {
        (1, 1)
    } else {
        (tb.numer as u64, tb.denom as u64)
    }
});

/// Convert a mach absolute-time value (ticks) to nanoseconds.
pub fn mach_to_ns(ticks: u64) -> i64 {
    let (n, d) = *TIMEBASE;
    ((ticks as u128 * n as u128) / d as u128) as i64
}

/// Current host time in nanoseconds.
pub fn now_ns() -> i64 {
    mach_to_ns(unsafe { mach_absolute_time() })
}
