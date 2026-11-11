//! Raw conversation with `polkit-agent-helper-1`: spawn, cookie line, prompt/response lines and
//! the final `SUCCESS`/`FAILURE`. Pure parsing lives here; the child and its fds are owned by
//! `flamewm_reactor::ChildPipe` (registered by the shell), so this module adds no event source.
//!
//! Protocol: argv[1] is the user name; the first stdin line is the polkit cookie. Helper stdout
//! lines are `PAM_PROMPT_ECHO_OFF <text>`, `PAM_PROMPT_ECHO_ON <text>`, `PAM_ERROR_MSG <text>`,
//! `PAM_TEXT_INFO <text>`, then `SUCCESS` or `FAILURE`. Each prompt is answered with one stdin line.
//! Token confirmation: the four `PAM_*` tokens and `SUCCESS`/`FAILURE` appear as literal strings in
//! the installed `/usr/lib/polkit-1/polkit-agent-helper-1` binary. The framing (one line per token,
//! cookie first) is spec-recalled and checked against a fake helper only.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use flamewm_reactor::ChildPipe;

/// Installed locations, in lookup order. `/usr/libexec` is normally a symlink to `/usr/lib`.
pub const HELPER_PATHS: [&str; 2] = [
    "/usr/lib/polkit-1/polkit-agent-helper-1",
    "/usr/libexec/polkit-agent-helper-1",
];

/// One parsed helper output line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperEvent {
    /// The helper wants a response; `echo` is false for passwords.
    Prompt {
        echo: bool,
        text: String,
    },
    ErrorMsg(String),
    TextInfo(String),
    Success,
    Failure,
}

/// Parses one helper stdout line (without its newline). Unknown lines return `None` and are ignored.
#[must_use]
pub fn parse_helper_line(line: &str) -> Option<HelperEvent> {
    let line = line.trim_end_matches('\r');
    match line {
        "SUCCESS" => return Some(HelperEvent::Success),
        "FAILURE" => return Some(HelperEvent::Failure),
        _ => {}
    }
    let (token, text) = line.split_once(' ').unwrap_or((line, ""));
    let text = text.to_owned();
    match token {
        "PAM_PROMPT_ECHO_OFF" => Some(HelperEvent::Prompt { echo: false, text }),
        "PAM_PROMPT_ECHO_ON" => Some(HelperEvent::Prompt { echo: true, text }),
        "PAM_ERROR_MSG" => Some(HelperEvent::ErrorMsg(text)),
        "PAM_TEXT_INFO" => Some(HelperEvent::TextInfo(text)),
        _ => None,
    }
}

/// Splits raw bytes from the helper into complete lines, keeping any partial tail for the next read.
#[derive(Debug, Default)]
pub struct LineBuffer {
    partial: Vec<u8>,
}

impl LineBuffer {
    /// Appends `bytes` and returns every line completed by them, without newlines.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.partial.extend_from_slice(bytes);
        let mut lines = Vec::new();
        while let Some(end) = self.partial.iter().position(|&b| b == b'\n') {
            let rest = self.partial.split_off(end + 1);
            let mut line = std::mem::replace(&mut self.partial, rest);
            line.pop();
            lines.push(String::from_utf8_lossy(&line).into_owned());
        }
        lines
    }
}

/// First helper binary that exists on this machine.
#[must_use]
pub fn helper_path() -> Option<PathBuf> {
    helper_path_in(&HELPER_PATHS, Path::exists)
}

fn helper_path_in(candidates: &[&str], exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| exists(path))
}

/// Spawns `command` as the helper for `user` and writes the cookie as its first stdin line.
///
/// `command` is normally `Command::new(helper_path()?)`; tests pass a fake helper. The caller
/// registers the returned pipe with the reactor.
///
/// # Errors
/// Returns `InvalidInput` for an empty user or cookie, or the spawn/write error.
pub fn spawn_helper(mut command: Command, user: &str, cookie: &str) -> io::Result<ChildPipe> {
    if user.is_empty() || cookie.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "helper needs a user and a cookie",
        ));
    }
    let mut pipe = ChildPipe::spawn(command.arg(user))?;
    pipe.write_line(cookie)?;
    Ok(pipe)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use flamewm_reactor::{FdAction, Reactor};

    use super::*;

    #[test]
    fn parses_pam_tokens_and_terminal_lines() {
        assert_eq!(
            parse_helper_line("PAM_PROMPT_ECHO_OFF Password:"),
            Some(HelperEvent::Prompt {
                echo: false,
                text: "Password:".into()
            })
        );
        assert_eq!(
            parse_helper_line("PAM_PROMPT_ECHO_ON user:\r"),
            Some(HelperEvent::Prompt {
                echo: true,
                text: "user:".into()
            })
        );
        assert_eq!(
            parse_helper_line("PAM_ERROR_MSG bad"),
            Some(HelperEvent::ErrorMsg("bad".into()))
        );
        assert_eq!(
            parse_helper_line("PAM_TEXT_INFO hi"),
            Some(HelperEvent::TextInfo("hi".into()))
        );
        assert_eq!(parse_helper_line("SUCCESS"), Some(HelperEvent::Success));
        assert_eq!(parse_helper_line("FAILURE"), Some(HelperEvent::Failure));
        assert_eq!(parse_helper_line("PAM_UNKNOWN x"), None);
        assert_eq!(parse_helper_line(""), None);
    }

    #[test]
    fn line_buffer_keeps_partial_tail() {
        let mut buf = LineBuffer::default();
        assert!(buf.push(b"PAM_PROMPT_ECHO_").is_empty());
        assert_eq!(
            buf.push(b"OFF x\nSUC"),
            vec!["PAM_PROMPT_ECHO_OFF x".to_owned()]
        );
        assert_eq!(buf.push(b"CESS\n"), vec!["SUCCESS".to_owned()]);
    }

    #[test]
    fn helper_path_takes_first_existing_candidate() {
        let found = helper_path_in(&["/a", "/b"], |p| p == Path::new("/b"));
        assert_eq!(found, Some(PathBuf::from("/b")));
        assert_eq!(helper_path_in(&["/a"], |_| false), None);
    }

    #[test]
    fn spawn_rejects_empty_user_or_cookie() {
        assert!(spawn_helper(Command::new("true"), "", "c").is_err());
        assert!(spawn_helper(Command::new("true"), "u", "").is_err());
    }

    /// Fake helper: prompts for a password, checks the answer against the cookie, then reports.
    /// Argv: $1 = user. Stdin: cookie line, then the response line.
    const FAKE: &str = r#"
read cookie
printf 'PAM_PROMPT_ECHO_OFF Password: \n'
read answer
if [ "$cookie" = "cookie-1" ] && [ "$answer" = "hunter2" ] && [ "$1" = "alice" ]; then
  printf 'PAM_TEXT_INFO ok\nSUCCESS\n'
else
  printf 'PAM_ERROR_MSG wrong\nFAILURE\n'
fi
"#;

    #[test]
    fn fake_helper_conversation_reaches_success_through_reactor() {
        let mut command = Command::new("sh");
        command.arg("-c").arg(FAKE).arg("sh");
        let pipe = spawn_helper(command, "alice", "cookie-1").expect("spawn");
        let mut reactor = Reactor::new().expect("reactor");
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&events);
        let mut lines = LineBuffer::default();
        reactor
            .register_child_pipe(pipe, move |_fd, pipe, _| {
                let mut buf = [0u8; 256];
                match pipe.read_some(&mut buf) {
                    Ok(0) => FdAction::Remove,
                    Ok(n) => {
                        for line in lines.push(&buf[..n]) {
                            if let Some(event) = parse_helper_line(&line) {
                                if matches!(event, HelperEvent::Prompt { .. }) {
                                    pipe.write_line("hunter2").expect("answer");
                                }
                                sink.borrow_mut().push(event);
                            }
                        }
                        FdAction::Continue
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => FdAction::Continue,
                    Err(_) => FdAction::Remove,
                }
            })
            .expect("register");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !events.borrow().contains(&HelperEvent::Success) && events.borrow().len() < 4 {
            assert!(Instant::now() < deadline, "helper conversation timed out");
            reactor
                .dispatch(Some(Duration::from_millis(20)))
                .expect("dispatch");
        }
        assert_eq!(
            events.borrow().as_slice(),
            &[
                HelperEvent::Prompt {
                    echo: false,
                    text: "Password: ".into()
                },
                HelperEvent::TextInfo("ok".into()),
                HelperEvent::Success,
            ]
        );
    }
}
