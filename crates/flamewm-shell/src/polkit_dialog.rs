//! Polkit authentication dialog projection. Pure state: `PolkitEvent` in, dialog model out.
//! No bus, no drawing, no polkit reply happens here. The shell wiring turns a `Submission` into a
//! helper or authority response, and a `cancel` cookie into `AgentSessions::finish`.

use std::collections::VecDeque;
use std::fmt;

use flamewm_integrations_linux::polkit_agent::{
    BeginRequest, Identity, PolkitEvent, MAX_QUEUED_EVENTS,
};

/// Upper bound on password bytes the field accepts.
pub const MAX_PASSWORD_BYTES: usize = 1024;
/// Shown when submit is pressed with an empty field.
pub const ERROR_EMPTY_PASSWORD: &str = "Enter your password.";
/// Shown when polkit offered no identity the user can authenticate as.
pub const ERROR_NO_IDENTITY: &str = "No account can authenticate this request.";

/// One identity row in the dialog. Polkit sends uids only, so names are not resolved here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityChoice {
    pub label: String,
    pub uid: Option<u32>,
}

/// Typed password buffer. Debug shows the length only. Bytes are overwritten on clear, on
/// take, and on drop. Best effort: the allocator may keep older copies, and text the input
/// method already handed to the toolkit is outside this type's reach.
#[derive(Default)]
pub struct PasswordField {
    bytes: Vec<u8>,
}

impl PasswordField {
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Appends typed text up to `MAX_PASSWORD_BYTES`, never splitting a character.
    /// Returns the number of bytes accepted.
    pub fn push_str(&mut self, text: &str) -> usize {
        let room = MAX_PASSWORD_BYTES - self.bytes.len();
        let mut take = text.len().min(room);
        while !text.is_char_boundary(take) {
            take -= 1;
        }
        self.bytes.extend_from_slice(&text.as_bytes()[..take]);
        take
    }

    /// Removes the last character (UTF-8 aware).
    pub fn backspace(&mut self) {
        let Some(mut popped) = self.bytes.pop() else {
            return;
        };
        while popped & 0xC0 == 0x80 {
            match self.bytes.pop() {
                Some(byte) => popped = byte,
                None => break,
            }
        }
    }

    pub fn clear(&mut self) {
        wipe(&mut self.bytes);
    }

    /// Moves the bytes out for one submit. The field is empty afterwards.
    fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.bytes)
    }
}

impl Drop for PasswordField {
    fn drop(&mut self) {
        wipe(&mut self.bytes);
    }
}

impl fmt::Debug for PasswordField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PasswordField {{ len: {} }}", self.bytes.len())
    }
}

/// Overwrites the bytes with zeros, then empties the buffer. `black_box` keeps the optimiser from
/// dropping the stores as dead. No `zeroize` crate is a dependency of this crate, so this is the
/// minimal overwrite.
fn wipe(bytes: &mut Vec<u8>) {
    for byte in bytes.iter_mut() {
        *byte = 0;
    }
    std::hint::black_box(&*bytes);
    bytes.clear();
}

/// The password leaving the dialog for one submit. The buffer is wiped on drop, so the
/// responder should drop it right after writing it out. Debug never prints the bytes.
pub struct Submission {
    pub cookie: String,
    pub uid: Option<u32>,
    password: Vec<u8>,
}

impl Submission {
    #[must_use]
    pub fn password_bytes(&self) -> &[u8] {
        &self.password
    }
}

impl Drop for Submission {
    fn drop(&mut self) {
        wipe(&mut self.password);
    }
}

impl fmt::Debug for Submission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Submission")
            .field("cookie", &self.cookie)
            .field("uid", &self.uid)
            .field("password", &format_args!("<{} bytes>", self.password.len()))
            .finish()
    }
}

/// One authentication prompt as the dialog shows it.
#[derive(Debug)]
pub struct DialogModel {
    pub cookie: String,
    pub message: String,
    pub action_id: String,
    pub icon_name: String,
    pub details: Vec<(String, String)>,
    pub identities: Vec<IdentityChoice>,
    pub selected: usize,
    pub password: PasswordField,
    pub error: Option<String>,
    pub busy: bool,
}

impl DialogModel {
    pub fn from_request(request: BeginRequest) -> Self {
        let identities: Vec<IdentityChoice> =
            request.identities.iter().map(identity_choice).collect();
        let error = identities.is_empty().then(|| ERROR_NO_IDENTITY.to_owned());
        Self {
            cookie: request.cookie,
            message: request.message,
            action_id: request.action_id,
            icon_name: request.icon_name,
            details: request.details,
            identities,
            selected: 0,
            password: PasswordField::default(),
            error,
            busy: false,
        }
    }

    /// Submit is enabled only with an identity, a non-empty password, and no request in flight.
    #[must_use]
    pub fn can_submit(&self) -> bool {
        !self.busy && !self.identities.is_empty() && !self.password.is_empty()
    }

    /// Picks an identity row. Ignored while busy or out of range.
    pub fn select(&mut self, index: usize) -> bool {
        if self.busy || index >= self.identities.len() {
            return false;
        }
        self.selected = index;
        true
    }

    pub fn input(&mut self, text: &str) -> usize {
        if self.busy {
            return 0;
        }
        self.password.push_str(text)
    }

    pub fn backspace(&mut self) {
        if !self.busy {
            self.password.backspace();
        }
    }

    /// Takes the password for one answer. Sets `busy` until `reject` or the shell's dismiss.
    /// Returns `None` and sets `error` when the prompt cannot be answered yet.
    pub fn submit(&mut self) -> Option<Submission> {
        if self.busy {
            return None;
        }
        let Some(choice) = self.identities.get(self.selected) else {
            self.error = Some(ERROR_NO_IDENTITY.to_owned());
            return None;
        };
        if self.password.is_empty() {
            self.error = Some(ERROR_EMPTY_PASSWORD.to_owned());
            return None;
        }
        self.busy = true;
        self.error = None;
        Some(Submission {
            cookie: self.cookie.clone(),
            uid: choice.uid,
            password: self.password.take(),
        })
    }

    /// Records a failed answer. Clears the field and allows another try. `message` is shown as
    /// plain text and must never contain the password.
    pub fn reject(&mut self, message: &str) {
        self.busy = false;
        self.password.clear();
        self.error = Some(message.to_owned());
    }
}

fn identity_choice(identity: &Identity) -> IdentityChoice {
    let label = match identity.uid {
        Some(uid) => format!("User {uid}"),
        None => identity.kind.clone(),
    };
    IdentityChoice {
        label,
        uid: identity.uid,
    }
}

/// Dialog state for the shell. One prompt is active; later prompts wait in FIFO order, bounded by
/// the same `MAX_QUEUED_EVENTS` the event queue uses.
#[derive(Debug, Default)]
pub struct PolkitDialog {
    active: Option<DialogModel>,
    pending: VecDeque<DialogModel>,
}

impl PolkitDialog {
    /// Applies one drained event. A `Show` beyond the bound is dropped; polkit then times out
    /// that request. Returns whether the event was accepted.
    pub fn apply(&mut self, event: PolkitEvent) -> bool {
        match event {
            PolkitEvent::Show(request) => {
                if self.active.is_none() {
                    self.active = Some(DialogModel::from_request(request));
                    true
                } else if self.pending.len() < MAX_QUEUED_EVENTS {
                    self.pending.push_back(DialogModel::from_request(request));
                    true
                } else {
                    false
                }
            }
            PolkitEvent::Dismiss(cookie) => {
                self.remove(&cookie);
                true
            }
        }
    }

    #[must_use]
    pub fn active(&self) -> Option<&DialogModel> {
        self.active.as_ref()
    }

    pub fn active_mut(&mut self) -> Option<&mut DialogModel> {
        self.active.as_mut()
    }

    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// User pressed cancel on the active prompt. Returns its cookie so the caller can finish it
    /// on the agent side. The password is wiped when the model drops.
    pub fn cancel(&mut self) -> Option<String> {
        let model = self.active.take()?;
        self.active = self.pending.pop_front();
        Some(model.cookie.clone())
    }

    /// Submits the active prompt's answer.
    pub fn submit(&mut self) -> Option<Submission> {
        self.active.as_mut()?.submit()
    }

    fn remove(&mut self, cookie: &str) {
        if self
            .active
            .as_ref()
            .is_some_and(|model| model.cookie == cookie)
        {
            self.active = self.pending.pop_front();
        } else {
            self.pending.retain(|model| model.cookie != cookie);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(cookie: &str, uids: &[Option<u32>]) -> BeginRequest {
        BeginRequest {
            action_id: "org.example.action".into(),
            message: "Authentication is required".into(),
            icon_name: "dialog-password".into(),
            details: vec![("polkit.message".into(), "Install".into())],
            cookie: cookie.into(),
            identities: uids
                .iter()
                .map(|uid| Identity {
                    kind: "unix-user".into(),
                    uid: *uid,
                })
                .collect(),
        }
    }

    #[test]
    fn show_opens_first_and_queues_second() {
        let mut dialog = PolkitDialog::default();
        assert!(dialog.apply(PolkitEvent::Show(request("a", &[Some(1000)]))));
        assert!(dialog.apply(PolkitEvent::Show(request("b", &[Some(1000)]))));
        assert_eq!(dialog.active().unwrap().cookie, "a");
        assert_eq!(dialog.pending_len(), 1);
    }

    #[test]
    fn queue_is_bounded() {
        let mut dialog = PolkitDialog::default();
        let accepted = (0..=MAX_QUEUED_EVENTS)
            .filter(|i| dialog.apply(PolkitEvent::Show(request(&format!("c{i}"), &[Some(1)]))))
            .count();
        assert_eq!(accepted, MAX_QUEUED_EVENTS + 1);
        assert_eq!(dialog.pending_len(), MAX_QUEUED_EVENTS);
    }

    #[test]
    fn dismiss_active_promotes_next_and_dismiss_pending_only_removes() {
        let mut dialog = PolkitDialog::default();
        dialog.apply(PolkitEvent::Show(request("a", &[Some(1)])));
        dialog.apply(PolkitEvent::Show(request("b", &[Some(1)])));
        dialog.apply(PolkitEvent::Show(request("c", &[Some(1)])));
        dialog.apply(PolkitEvent::Dismiss("b".into()));
        assert_eq!(dialog.active().unwrap().cookie, "a");
        assert_eq!(dialog.pending_len(), 1);
        dialog.apply(PolkitEvent::Dismiss("a".into()));
        assert_eq!(dialog.active().unwrap().cookie, "c");
        assert_eq!(dialog.pending_len(), 0);
    }

    #[test]
    fn password_caps_at_bound_and_never_splits_characters() {
        let mut field = PasswordField::default();
        let cap = "é".repeat(MAX_PASSWORD_BYTES);
        assert_eq!(field.push_str(&cap), MAX_PASSWORD_BYTES);
        assert_eq!(field.len(), MAX_PASSWORD_BYTES);
        assert_eq!(field.push_str("x"), 0);

        let mut odd = PasswordField::default();
        odd.push_str(&"a".repeat(MAX_PASSWORD_BYTES - 1));
        assert_eq!(odd.push_str("é"), 0, "two-byte char must not be split");
    }

    #[test]
    fn backspace_removes_whole_character() {
        let mut field = PasswordField::default();
        field.push_str("aé");
        field.backspace();
        assert_eq!(field.len(), 1);
        field.backspace();
        assert!(field.is_empty());
        field.backspace();
        assert!(field.is_empty());
    }

    #[test]
    fn submit_requires_password_and_hands_over_selected_uid() {
        let mut model = DialogModel::from_request(request("a", &[Some(1000), Some(1001)]));
        assert!(!model.can_submit());
        assert!(model.submit().is_none());
        assert_eq!(model.error.as_deref(), Some(ERROR_EMPTY_PASSWORD));

        assert!(model.select(1));
        model.input("hunter2");
        assert!(model.can_submit());
        let submission = model.submit().expect("submission");
        assert_eq!(submission.cookie, "a");
        assert_eq!(submission.uid, Some(1001));
        assert_eq!(submission.password_bytes(), b"hunter2");
        assert!(model.password.is_empty());
        assert!(model.busy);
        assert!(model.submit().is_none(), "no second answer while busy");
        assert_eq!(model.input("x"), 0, "no typing while busy");
    }

    #[test]
    fn reject_clears_field_and_allows_retry() {
        let mut model = DialogModel::from_request(request("a", &[Some(1000)]));
        model.input("wrong");
        let _ = model.submit();
        model.reject("Authentication failed.");
        assert!(!model.busy);
        assert!(model.password.is_empty());
        assert_eq!(model.error.as_deref(), Some("Authentication failed."));
        model.input("right");
        assert!(model.submit().is_some());
    }

    #[test]
    fn no_identity_blocks_submit_with_error() {
        let mut model = DialogModel::from_request(request("a", &[]));
        assert_eq!(model.error.as_deref(), Some(ERROR_NO_IDENTITY));
        model.input("pw");
        assert!(!model.can_submit());
        assert!(model.submit().is_none());
    }

    #[test]
    fn cancel_returns_cookie_and_promotes_next() {
        let mut dialog = PolkitDialog::default();
        dialog.apply(PolkitEvent::Show(request("a", &[Some(1)])));
        dialog.apply(PolkitEvent::Show(request("b", &[Some(1)])));
        assert_eq!(dialog.cancel().as_deref(), Some("a"));
        assert_eq!(dialog.active().unwrap().cookie, "b");
        assert_eq!(dialog.cancel().as_deref(), Some("b"));
        assert!(dialog.active().is_none());
        assert!(dialog.cancel().is_none());
    }

    #[test]
    fn debug_output_never_shows_password() {
        let mut dialog = PolkitDialog::default();
        dialog.apply(PolkitEvent::Show(request("a", &[Some(1)])));
        dialog.active_mut().unwrap().input("s3cret-value");
        let submission = dialog.submit().unwrap();
        let rendered = format!("{dialog:?} {submission:?}");
        assert!(!rendered.contains("s3cret-value"), "{rendered}");
        assert!(rendered.contains("12 bytes"), "{rendered}");
    }
}
