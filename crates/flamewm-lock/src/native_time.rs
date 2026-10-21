//! Local wall-clock time through the C library, since std has no local-time API.
use std::ffi::{c_char, c_int, c_long};
use std::mem::MaybeUninit;
use std::time::{SystemTime, UNIX_EPOCH};

/// Broken-down local time; the lock screen only needs these fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

/// glibc `struct tm` layout for 64-bit Linux.
#[repr(C)]
struct Tm {
    tm_sec: c_int,
    tm_min: c_int,
    tm_hour: c_int,
    tm_mday: c_int,
    tm_mon: c_int,
    tm_year: c_int,
    tm_wday: c_int,
    tm_yday: c_int,
    tm_isdst: c_int,
    tm_gmtoff: c_long,
    tm_zone: *const c_char,
}

unsafe extern "C" {
    /// # Safety
    /// `time` must point to a valid `time_t`; `out` must point to writable `struct tm`.
    fn localtime_r(time: *const c_long, out: *mut Tm) -> *mut Tm;
}

/// Current local time, or `None` if the clock or C library cannot convert it.
pub fn local_now() -> Option<LocalTime> {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let time = c_long::try_from(seconds).ok()?;
    let mut out = MaybeUninit::<Tm>::uninit();
    // SAFETY: `time` and `out` are valid for the duration of the call; on a
    // non-null return the C library has fully initialized `out`.
    let converted = unsafe { localtime_r(&time, out.as_mut_ptr()) };
    if converted.is_null() {
        return None;
    }
    // SAFETY: localtime_r returned non-null, so `out` was initialized.
    let tm = unsafe { out.assume_init() };
    Some(LocalTime {
        year: tm.tm_year.checked_add(1900)?,
        month: u8::try_from(tm.tm_mon.checked_add(1)?).ok()?,
        day: u8::try_from(tm.tm_mday).ok()?,
        hour: u8::try_from(tm.tm_hour).ok()?,
        minute: u8::try_from(tm.tm_min).ok()?,
    })
}
