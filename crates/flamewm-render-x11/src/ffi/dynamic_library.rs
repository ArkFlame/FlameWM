use std::ffi::{CStr, CString};
use std::mem;
use std::os::raw::{c_char, c_int, c_void};

const RTLD_NOW: c_int = 2;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const RTLD_NODELETE: c_int = 0x1000;

pub(crate) struct DynamicLibrary {
    handle: *mut c_void,
    close_on_drop: bool,
}

impl DynamicLibrary {
    pub(crate) fn open(names: &[&str]) -> Result<Self, String> {
        let (flags, close_on_drop) = loader_options(false);
        Self::open_with_flags(names, flags, close_on_drop)
    }

    /// Keep a library mapped for process lifetime, including after its handle is dropped.
    pub(crate) fn open_nodelete(names: &[&str]) -> Result<Self, String> {
        let (flags, close_on_drop) = loader_options(true);
        Self::open_with_flags(names, flags, close_on_drop)
    }

    fn open_with_flags(names: &[&str], flags: c_int, close_on_drop: bool) -> Result<Self, String> {
        for name in names {
            let c_name = CString::new(*name).expect("library name is static and NUL-free");
            // `c_name` remains alive for the call; `flags` are valid dlopen flags.
            let handle = unsafe { dlopen(c_name.as_ptr(), flags) };
            if !handle.is_null() {
                return Ok(Self {
                    handle,
                    close_on_drop,
                });
            }
        }
        Err(format!("unable to load any of: {}", names.join(", ")))
    }

    pub(crate) unsafe fn symbol<T: Copy>(&self, name: &'static [u8]) -> Result<T, String> {
        debug_assert_eq!(name.last().copied(), Some(0));
        // SAFETY: `dlerror` takes no arguments; it only clears thread-local dl state.
        unsafe { dlerror() };
        // SAFETY: `self.handle` is a live `dlopen` handle owned until drop, and the
        // caller guarantees `name` is NUL-terminated per this function's contract.
        let raw = unsafe { dlsym(self.handle, name.as_ptr() as *const c_char) };
        // SAFETY: `dlerror` takes no arguments; it only reads thread-local dl state.
        let error = unsafe { dlerror() };
        if raw.is_null() || !error.is_null() {
            let message = if error.is_null() {
                "symbol resolved to NULL".to_string()
            } else {
                // SAFETY: `error` is non-NULL here; libdl guarantees a NUL-terminated
                // thread-local message valid until the next dl call on this thread.
                unsafe { CStr::from_ptr(error) }
                    .to_string_lossy()
                    .into_owned()
            };
            return Err(format!(
                "{}: {message}",
                String::from_utf8_lossy(&name[..name.len() - 1])
            ));
        }
        if mem::size_of::<T>() != mem::size_of::<*mut c_void>() {
            return Err("dynamic function pointer has unexpected size".to_string());
        }
        // SAFETY: `raw` is a non-NULL initialized data pointer and `T` was just
        // checked to have exactly pointer size, so a bitwise copy is sound.
        Ok(unsafe { mem::transmute_copy(&raw) })
    }
}

impl Drop for DynamicLibrary {
    fn drop(&mut self) {
        if self.close_on_drop && !self.handle.is_null() {
            // Handle was returned by `dlopen` and remains owned until this drop.
            unsafe {
                dlclose(self.handle);
            }
        }
    }
}

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

fn loader_options(nodelete: bool) -> (c_int, bool) {
    if !nodelete {
        return (RTLD_NOW, true);
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        (RTLD_NOW | RTLD_NODELETE, true)
    }
    // RTLD_NODELETE is a GNU/Linux extension. On other targets retain the handle instead.
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        (RTLD_NOW, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nodelete_load_flags_are_limited_to_nodelete_loads() {
        assert_eq!(RTLD_NOW, 2);
        assert_eq!(loader_options(false), (RTLD_NOW, true));
        let (flags, close_on_drop) = loader_options(true);

        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        {
            assert_eq!(RTLD_NODELETE, 0x1000);
            assert_eq!(flags, RTLD_NOW | RTLD_NODELETE);
            assert!(close_on_drop);
        }
        #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
        {
            // Without the GNU extension, open_nodelete retains the handle rather than dlclosing.
            assert_eq!(flags, RTLD_NOW);
            assert!(!close_on_drop);
        }
    }
}
