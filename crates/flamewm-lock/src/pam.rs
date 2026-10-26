//! PAM authentication loaded with `dlopen`, so no build-time libpam dependency
//! exists. Every failure path resolves to a non-unlocked outcome.
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;

pub const SERVICE: &str = "flamewm-lock";
const LIBRARY_NAMES: [&str; 2] = ["libpam.so.0", "libpam.so"];
const RTLD_NOW: c_int = 2;

const PAM_SUCCESS: c_int = 0;
const PAM_SYSTEM_ERR: c_int = 4;
const PAM_BUF_ERR: c_int = 5;
const PAM_AUTH_ERR: c_int = 7;
const PAM_USER_UNKNOWN: c_int = 10;
const PAM_CONV_ERR: c_int = 19;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;
const PAM_MAX_NUM_MSG: c_int = 32;

#[repr(C)]
pub struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
pub struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

type ConvFn = unsafe extern "C" fn(
    c_int,
    *mut *const PamMessage,
    *mut *mut PamResponse,
    *mut c_void,
) -> c_int;

#[repr(C)]
pub struct PamConv {
    conv: Option<ConvFn>,
    appdata_ptr: *mut c_void,
}

type PamStartFn =
    unsafe extern "C" fn(*const c_char, *const c_char, *const PamConv, *mut *mut c_void) -> c_int;
type PamCallFn = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Unlocked,
    /// Wrong credentials or an account that may not log in: stay locked, say "failed".
    Denied,
    /// Any other failure, including a missing libpam: stay locked, say "unavailable".
    Unavailable,
}

/// Answers PAM prompts. `ECHO_OFF` gets the password, `ECHO_ON` the username,
/// informational messages get no reply, and unknown styles are rejected.
pub fn build_replies(
    styles: &[c_int],
    password: &str,
    username: &str,
) -> Result<Vec<Option<String>>, c_int> {
    styles
        .iter()
        .map(|&style| match style {
            PAM_PROMPT_ECHO_OFF => Ok(Some(password.to_string())),
            PAM_PROMPT_ECHO_ON => Ok(Some(username.to_string())),
            PAM_ERROR_MSG | PAM_TEXT_INFO => Ok(None),
            _ => Err(PAM_CONV_ERR),
        })
        .collect()
}

/// Operations a PAM transaction needs; the real libpam and test doubles both implement it.
pub trait PamSession {
    fn start(&mut self) -> Result<(), c_int>;
    fn authenticate(&mut self) -> c_int;
    fn acct_mgmt(&mut self) -> c_int;
    fn end(&mut self, status: c_int);
}

/// Runs the transaction and maps every outcome to a lock decision.
pub fn run_flow(session: &mut dyn PamSession) -> Outcome {
    if session.start().is_err() {
        return Outcome::Unavailable;
    }
    let auth = session.authenticate();
    let outcome = if auth == PAM_SUCCESS {
        if session.acct_mgmt() == PAM_SUCCESS {
            Outcome::Unlocked
        } else {
            Outcome::Denied
        }
    } else if auth == PAM_AUTH_ERR || auth == PAM_USER_UNKNOWN {
        Outcome::Denied
    } else {
        Outcome::Unavailable
    };
    session.end(if outcome == Outcome::Unlocked {
        PAM_SUCCESS
    } else {
        PAM_AUTH_ERR
    });
    outcome
}

/// Authenticates `username` with `password` through the `flamewm-lock` PAM service.
#[must_use]
pub fn authenticate(username: &str, password: &str) -> Outcome {
    let Ok(library) = DynamicPam::load_from(&LIBRARY_NAMES) else {
        return Outcome::Unavailable;
    };
    let Ok(mut session) = library.session(username, password) else {
        return Outcome::Unavailable;
    };
    run_flow(&mut session)
}

struct DynamicPam {
    handle: *mut c_void,
    start: PamStartFn,
    authenticate: PamCallFn,
    acct_mgmt: PamCallFn,
    end: PamCallFn,
}

impl DynamicPam {
    fn load_from(names: &[&str]) -> Result<Self, String> {
        let handle = open_any(names)?;
        // SAFETY: each symbol is a documented libpam entry point; `F` is a fn pointer
        // type matching its C signature.
        let resolved = (|| -> Result<Self, String> {
            unsafe {
                Ok(Self {
                    handle,
                    start: resolve(handle, b"pam_start\0")?,
                    authenticate: resolve(handle, b"pam_authenticate\0")?,
                    acct_mgmt: resolve(handle, b"pam_acct_mgmt\0")?,
                    end: resolve(handle, b"pam_end\0")?,
                })
            }
        })();
        if resolved.is_err() {
            // SAFETY: `handle` came from a successful dlopen and is not used after this.
            unsafe { dlclose(handle) };
        }
        resolved
    }

    fn session(&self, username: &str, password: &str) -> Result<LibPamSession<'_>, String> {
        let answers = Box::new(Answers {
            username: username.to_string(),
            password: password.to_string(),
        });
        let conv = Box::new(PamConv {
            conv: Some(conversation),
            appdata_ptr: &raw const *answers as *mut c_void,
        });
        Ok(LibPamSession {
            library: self,
            handle: ptr::null_mut(),
            user: CString::new(username).map_err(|error| error.to_string())?,
            answers,
            conv,
        })
    }
}

impl Drop for DynamicPam {
    fn drop(&mut self) {
        // SAFETY: `handle` is the dlopen handle owned by this value.
        unsafe { dlclose(self.handle) };
    }
}

struct Answers {
    username: String,
    password: String,
}

struct LibPamSession<'a> {
    library: &'a DynamicPam,
    handle: *mut c_void,
    user: CString,
    // Boxed so the addresses handed to libpam stay stable across moves.
    answers: Box<Answers>,
    conv: Box<PamConv>,
}

impl PamSession for LibPamSession<'_> {
    fn start(&mut self) -> Result<(), c_int> {
        let service = CString::new(SERVICE).map_err(|_| PAM_SYSTEM_ERR)?;
        // SAFETY: strings and conv struct outlive the call; `handle` is an out-pointer.
        let rc = unsafe {
            (self.library.start)(
                service.as_ptr(),
                self.user.as_ptr(),
                &raw const *self.conv,
                &raw mut self.handle,
            )
        };
        if rc == PAM_SUCCESS { Ok(()) } else { Err(rc) }
    }

    fn authenticate(&mut self) -> c_int {
        // SAFETY: `handle` was produced by a successful pam_start.
        unsafe { (self.library.authenticate)(self.handle, 0) }
    }

    fn acct_mgmt(&mut self) -> c_int {
        // SAFETY: `handle` was produced by a successful pam_start.
        unsafe { (self.library.acct_mgmt)(self.handle, 0) }
    }

    fn end(&mut self, status: c_int) {
        if !self.handle.is_null() {
            // SAFETY: `handle` is consumed here and nulled so it cannot be reused.
            unsafe { (self.library.end)(self.handle, status) };
            self.handle = ptr::null_mut();
        }
        let _ = &self.answers;
    }
}

unsafe extern "C" fn conversation(
    count: c_int,
    messages: *mut *const PamMessage,
    responses: *mut *mut PamResponse,
    appdata: *mut c_void,
) -> c_int {
    if count <= 0 || count > PAM_MAX_NUM_MSG || messages.is_null() || responses.is_null() {
        return PAM_CONV_ERR;
    }
    if appdata.is_null() {
        return PAM_CONV_ERR;
    }
    // SAFETY: appdata is the `Answers` box kept alive for the whole PAM call.
    let answers = unsafe { &*(appdata as *const Answers) };
    let count_usize = count as usize;
    let mut styles = Vec::with_capacity(count_usize);
    for index in 0..count_usize {
        // SAFETY: libpam passes `count` message pointers; each must be non-null.
        let message = unsafe { *messages.add(index) };
        if message.is_null() {
            return PAM_CONV_ERR;
        }
        // SAFETY: `message` was checked non-null above.
        styles.push(unsafe { (*message).msg_style });
    }
    let replies = match build_replies(&styles, &answers.password, &answers.username) {
        Ok(replies) => replies,
        Err(code) => return code,
    };
    // SAFETY: calloc returns zeroed memory; libpam releases it with free().
    let array =
        unsafe { calloc(count_usize, std::mem::size_of::<PamResponse>()) }.cast::<PamResponse>();
    if array.is_null() {
        return PAM_BUF_ERR;
    }
    for (index, reply) in replies.iter().enumerate() {
        let Some(text) = reply else { continue };
        let bytes = text.as_bytes();
        // SAFETY: allocates bytes.len() + 1 zeroed bytes for a NUL-terminated copy.
        let buffer = unsafe { calloc(bytes.len() + 1, 1) }.cast::<c_char>();
        if buffer.is_null() {
            // SAFETY: `array` and earlier buffers were allocated by calloc above.
            unsafe { free_responses(array, count_usize) };
            return PAM_BUF_ERR;
        }
        // SAFETY: `buffer` has room for bytes.len() bytes; `array[index]` is in range.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast::<u8>(), bytes.len());
            (*array.add(index)).resp = buffer;
        }
    }
    // SAFETY: `responses` was checked non-null; libpam takes ownership of `array`.
    unsafe { *responses = array };
    PAM_SUCCESS
}

/// # Safety
/// `array` must hold `count` `PamResponse` entries allocated with calloc.
unsafe fn free_responses(array: *mut PamResponse, count: usize) {
    for index in 0..count {
        // SAFETY: index is within the calloc'ed array.
        unsafe { free((*array.add(index)).resp.cast::<c_void>()) };
    }
    // SAFETY: `array` came from calloc.
    unsafe { free(array.cast::<c_void>()) };
}

/// # Safety
/// `handle` must be a live dlopen handle and `F` a fn pointer type matching the symbol.
unsafe fn resolve<F: Copy>(handle: *mut c_void, symbol: &'static [u8]) -> Result<F, String> {
    // SAFETY: `handle` is live and `symbol` is NUL-terminated.
    let raw = unsafe { dlsym(handle, symbol.as_ptr().cast::<c_char>()) };
    if raw.is_null() {
        return Err(format!(
            "missing symbol {}",
            String::from_utf8_lossy(&symbol[..symbol.len() - 1])
        ));
    }
    // SAFETY: `F` is a pointer-sized fn pointer type per the caller contract.
    Ok(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&raw) })
}

fn open_any(names: &[&str]) -> Result<*mut c_void, String> {
    for name in names {
        let c_name = CString::new(*name).map_err(|error| error.to_string())?;
        // SAFETY: `c_name` is NUL-terminated and alive for the call.
        let handle = unsafe { dlopen(c_name.as_ptr(), RTLD_NOW) };
        if !handle.is_null() {
            return Ok(handle);
        }
    }
    // SAFETY: dlerror only reads thread-local state.
    let last = unsafe { dlerror() };
    let detail = if last.is_null() {
        String::from("no detail")
    } else {
        // SAFETY: non-null dlerror result is a NUL-terminated message.
        unsafe { CStr::from_ptr(last) }
            .to_string_lossy()
            .into_owned()
    };
    Err(format!(
        "unable to load any of {}: {detail}",
        names.join(", ")
    ))
}

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}

unsafe extern "C" {
    fn calloc(count: usize, size: usize) -> *mut c_void;
    fn free(pointer: *mut c_void);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockSession {
        start: Result<(), c_int>,
        auth: c_int,
        acct: c_int,
        ended_with: Option<c_int>,
    }

    impl PamSession for MockSession {
        fn start(&mut self) -> Result<(), c_int> {
            self.start
        }
        fn authenticate(&mut self) -> c_int {
            self.auth
        }
        fn acct_mgmt(&mut self) -> c_int {
            self.acct
        }
        fn end(&mut self, status: c_int) {
            self.ended_with = Some(status);
        }
    }

    fn mock(start: Result<(), c_int>, auth: c_int, acct: c_int) -> MockSession {
        MockSession {
            start,
            auth,
            acct,
            ended_with: None,
        }
    }

    #[test]
    fn success_with_valid_account_unlocks() {
        let mut session = mock(Ok(()), PAM_SUCCESS, PAM_SUCCESS);
        assert_eq!(run_flow(&mut session), Outcome::Unlocked);
        assert_eq!(session.ended_with, Some(PAM_SUCCESS));
    }

    #[test]
    fn wrong_password_is_denied_and_stays_locked() {
        let mut session = mock(Ok(()), PAM_AUTH_ERR, PAM_SUCCESS);
        assert_eq!(run_flow(&mut session), Outcome::Denied);
        assert_eq!(session.ended_with, Some(PAM_AUTH_ERR));
    }

    #[test]
    fn pam_start_failure_is_unavailable() {
        let mut session = mock(Err(PAM_SYSTEM_ERR), PAM_SUCCESS, PAM_SUCCESS);
        assert_eq!(run_flow(&mut session), Outcome::Unavailable);
        assert_eq!(session.ended_with, None);
    }

    #[test]
    fn other_authenticate_errors_fail_closed() {
        let mut session = mock(Ok(()), PAM_CONV_ERR, PAM_SUCCESS);
        assert_eq!(run_flow(&mut session), Outcome::Unavailable);
    }

    #[test]
    fn conversation_replies_follow_prompt_style() {
        let replies = build_replies(
            &[
                PAM_PROMPT_ECHO_OFF,
                PAM_PROMPT_ECHO_ON,
                PAM_ERROR_MSG,
                PAM_TEXT_INFO,
            ],
            "secret",
            "alice",
        );
        assert_eq!(
            replies,
            Ok(vec![
                Some("secret".to_string()),
                Some("alice".to_string()),
                None,
                None
            ])
        );
    }

    #[test]
    fn unknown_prompt_style_is_a_conversation_error() {
        assert_eq!(build_replies(&[99], "secret", "alice"), Err(PAM_CONV_ERR));
    }

    #[test]
    fn expired_or_new_token_required_after_valid_password_stays_locked() {
        const PAM_NEW_AUTHTOK_REQD: c_int = 12;
        for acct in [13, PAM_NEW_AUTHTOK_REQD] {
            let mut session = mock(Ok(()), PAM_SUCCESS, acct);
            assert_eq!(run_flow(&mut session), Outcome::Denied);
            assert_eq!(session.ended_with, Some(PAM_AUTH_ERR));
        }
    }

    #[test]
    fn missing_libpam_stays_locked() {
        assert!(DynamicPam::load_from(&["libflamewm-no-such-pam.so.0"]).is_err());
    }
}
