//! Answers one polkit prompt. A dialog `Submission` starts `polkit-agent-helper-1`, its pipe is
//! registered on the reactor, the helper's single password prompt is answered from the submission,
//! and the helper's result is reported through `settle`. The shell's `settle` finishes the pending
//! `BeginAuthentication` call and dismisses the dialog entry.
//!
//! After the password is written, a timer kills the helper if it has not finished within
//! `HELPER_ANSWER_TIMEOUT`. Its output then closes and the conversation settles as `Failed`.

use std::cell::{Cell, RefCell};
use std::io;
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

use flamewm_integrations_linux::polkit_agent::{AuthOutcome, PolkitAgent, PolkitEvent};
use flamewm_integrations_linux::polkit_helper::{
    drive, helper_path, spawn_helper, user_name_for_uid, HelperConversation, HelperStep,
};
use flamewm_reactor::{ChildPipe, FdAction, ReactorHandle};

use crate::polkit_dialog::{PolkitDialog, Submission};

/// Time the helper gets after the password is written before it is killed and the answer fails.
pub const HELPER_ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// Starts the real helper for `submission` and answers it on the reactor.
///
/// # Errors
/// Returns `InvalidInput` when the identity has no uid, `NotFound` when the uid has no account
/// or the helper is not installed, or the spawn and registration error.
pub fn start(
    reactor: &ReactorHandle,
    agent: Rc<RefCell<PolkitAgent>>,
    dialog: Rc<RefCell<PolkitDialog>>,
    submission: Submission,
) -> io::Result<()> {
    let uid = submission
        .uid
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "identity has no uid"))?;
    let user = user_name_for_uid(uid)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no account for identity uid"))?;
    let program = helper_path()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "polkit-agent-helper-1 missing"))?;
    start_with(
        reactor,
        Command::new(program),
        &user,
        submission,
        HELPER_ANSWER_TIMEOUT,
        move |cookie, outcome| finish_and_dismiss(&agent, &dialog, cookie, outcome),
    )
}

pub fn finish_and_dismiss(
    agent: &Rc<RefCell<PolkitAgent>>,
    dialog: &Rc<RefCell<PolkitDialog>>,
    cookie: &str,
    outcome: AuthOutcome,
) {
    if let Err(error) = agent.borrow_mut().finish(cookie, outcome) {
        eprintln!("flamewm-shell: polkit reply failed: {error}");
    }
    dialog
        .borrow_mut()
        .apply(PolkitEvent::Dismiss(cookie.to_owned()));
}

/// Runs `command` (the helper, or a test double) for `user` and answers its prompt with the
/// submitted password. `settle` gets the cookie and the outcome exactly once. If the helper is
/// still running `answer_timeout` after the password is written, it is killed and settles `Failed`.
///
/// # Errors
/// Returns the spawn or registration error. Errors after registration are reported as `Failed`.
pub fn start_with<F>(
    reactor: &ReactorHandle,
    command: Command,
    user: &str,
    submission: Submission,
    answer_timeout: Duration,
    settle: F,
) -> io::Result<()>
where
    F: FnMut(&str, AuthOutcome) + 'static,
{
    let cookie = submission.cookie.clone();
    let pipe = spawn_helper(command, user, &cookie)?;
    let finished = Rc::new(Cell::new(false));
    let mut settle = settle;
    let mut settle_once = {
        let finished = Rc::clone(&finished);
        move |cookie: &str, outcome: AuthOutcome| {
            finished.set(true);
            settle(cookie, outcome);
        }
    };
    let mut pending = Some(submission);
    let mut conversation = HelperConversation::default();
    let handle = reactor.clone();
    reactor
        .register_child_pipe(pipe, move |_fd, pipe: &mut ChildPipe, _| {
            let step = match drive(pipe, &mut conversation) {
                Ok(step) => step,
                Err(error) => {
                    eprintln!("flamewm-shell: polkit helper read failed: {error}");
                    let _ = pipe.kill();
                    settle_once(&cookie, AuthOutcome::Failed);
                    return FdAction::Remove;
                }
            };
            match step {
                HelperStep::Idle => FdAction::Continue,
                HelperStep::NeedsPassword => {
                    // The submission is dropped after the write, which wipes its buffer.
                    let answered = pending.take().is_some_and(|submission| {
                        std::str::from_utf8(submission.password_bytes())
                            .is_ok_and(|text| pipe.write_line(text).is_ok())
                    });
                    if !answered {
                        let _ = pipe.kill();
                        settle_once(&cookie, AuthOutcome::Failed);
                        return FdAction::Remove;
                    }
                    conversation.answered();
                    let pid = pipe.pid();
                    let guard = Rc::clone(&finished);
                    let armed = handle.register_timer(answer_timeout, move || {
                        // Only a helper that is still pending is killed; a finished one may have
                        // been reaped, and its pid could then belong to another process.
                        if !guard.get() {
                            let _ = ChildPipe::kill_pid(pid);
                        }
                    });
                    if armed.is_err() {
                        let _ = pipe.kill();
                        settle_once(&cookie, AuthOutcome::Failed);
                        return FdAction::Remove;
                    }
                    FdAction::Continue
                }
                HelperStep::Done(outcome) => {
                    let _ = pipe.wait();
                    settle_once(&cookie, outcome);
                    FdAction::Remove
                }
            }
        })
        .map(|_| ())
        .map_err(|error| io::Error::other(format!("register polkit helper: {error}")))
}

/// Cancels the active prompt: the pending call gets `Error.Cancelled`, and the next prompt, if
/// any, becomes active. Returns whether a prompt was active.
pub fn cancel_active(agent: &Rc<RefCell<PolkitAgent>>, dialog: &Rc<RefCell<PolkitDialog>>) -> bool {
    cancel_with(dialog, |cookie, outcome| {
        if let Err(error) = agent.borrow_mut().finish(cookie, outcome) {
            eprintln!("flamewm-shell: polkit cancel reply failed: {error}");
        }
    })
}

/// Cancel without the bus: removes the active prompt and reports `Cancelled` through `finish`.
fn cancel_with(
    dialog: &Rc<RefCell<PolkitDialog>>,
    mut finish: impl FnMut(&str, AuthOutcome),
) -> bool {
    let Some(cookie) = dialog.borrow_mut().cancel() else {
        return false;
    };
    finish(&cookie, AuthOutcome::Cancelled);
    true
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use flamewm_integrations_linux::polkit_agent::{BeginRequest, Identity};
    use flamewm_reactor::Reactor;

    use super::*;

    const FAKE: &str = r#"
read cookie
printf 'PAM_PROMPT_ECHO_OFF Password: \n'
read answer
if [ "$cookie" = "cookie-x" ] && [ "$answer" = "hunter2" ] && [ "$1" = "alice" ]; then
  printf 'PAM_TEXT_INFO ok\nSUCCESS\n'
else
  printf 'PAM_ERROR_MSG wrong\nFAILURE\n'
fi
"#;

    /// Prompts, reads the answer, then hangs. `exec` makes the shell itself the sleeper, so killing
    /// the pid closes the output pipe at once.
    const HANG: &str = r"
read cookie
printf 'PAM_PROMPT_ECHO_OFF Password: \n'
read answer
exec sleep 30
";

    fn dialog_with(cookies: &[&str]) -> Rc<RefCell<PolkitDialog>> {
        let mut dialog = PolkitDialog::default();
        for cookie in cookies {
            dialog.apply(PolkitEvent::Show(request(cookie)));
        }
        Rc::new(RefCell::new(dialog))
    }

    fn request(cookie: &str) -> BeginRequest {
        BeginRequest {
            action_id: "org.example.action".into(),
            message: "Authentication is required".into(),
            icon_name: "dialog-password".into(),
            details: Vec::new(),
            cookie: cookie.into(),
            identities: vec![Identity {
                kind: "unix-user".into(),
                uid: Some(1000),
            }],
        }
    }

    fn submission(typed: &str) -> Submission {
        let dialog = dialog_with(&["cookie-x"]);
        let mut dialog = dialog.borrow_mut();
        dialog.active_mut().expect("active").input(typed);
        dialog.submit().expect("submission")
    }

    /// Runs `script` as the helper with `typed` as the password. Returns what `settle` saw.
    fn run_script(
        script: &str,
        typed: &str,
        answer_timeout: Duration,
    ) -> Vec<(String, AuthOutcome)> {
        let settled = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&settled);
        let mut command = Command::new("sh");
        command.arg("-c").arg(script).arg("sh");
        let mut reactor = Reactor::new().expect("reactor");
        start_with(
            &reactor.handle(),
            command,
            "alice",
            submission(typed),
            answer_timeout,
            move |cookie, outcome| sink.borrow_mut().push((cookie.to_owned(), outcome)),
        )
        .expect("start");
        let deadline = Instant::now() + Duration::from_secs(10);
        while settled.borrow().is_empty() {
            assert!(Instant::now() < deadline, "helper answer timed out");
            reactor
                .dispatch(Some(Duration::from_millis(20)))
                .expect("dispatch");
        }
        settled.take()
    }

    #[test]
    fn right_password_settles_success_once() {
        assert_eq!(
            run_script(FAKE, "hunter2", HELPER_ANSWER_TIMEOUT),
            vec![("cookie-x".to_owned(), AuthOutcome::Success)]
        );
    }

    #[test]
    fn wrong_password_settles_failed_once() {
        assert_eq!(
            run_script(FAKE, "nope", HELPER_ANSWER_TIMEOUT),
            vec![("cookie-x".to_owned(), AuthOutcome::Failed)]
        );
    }

    #[test]
    fn hung_helper_is_killed_and_settles_failed() {
        assert_eq!(
            run_script(HANG, "hunter2", Duration::from_millis(200)),
            vec![("cookie-x".to_owned(), AuthOutcome::Failed)]
        );
    }

    #[test]
    fn cancel_reaches_finish_with_cancelled_and_promotes_next() {
        let dialog = dialog_with(&["a", "b"]);
        let mut seen = Vec::new();
        assert!(cancel_with(&dialog, |cookie, outcome| seen
            .push((cookie.to_owned(), outcome))));
        assert_eq!(seen, vec![("a".to_owned(), AuthOutcome::Cancelled)]);
        assert_eq!(dialog.borrow().active().expect("next").cookie, "b");
    }

    #[test]
    fn cancel_without_prompt_finishes_nothing() {
        let dialog = dialog_with(&[]);
        let mut seen = Vec::new();
        assert!(!cancel_with(&dialog, |cookie, outcome| seen
            .push((cookie.to_owned(), outcome))));
        assert!(seen.is_empty());
    }
}
