//! Answers one polkit prompt. A dialog `Submission` starts `polkit-agent-helper-1`, its pipe is
//! registered on the reactor, the helper's single password prompt is answered from the submission,
//! and the helper's result is reported through `settle`. The shell's `settle` finishes the pending
//! `BeginAuthentication` call and dismisses the dialog entry.

use std::cell::RefCell;
use std::io;
use std::process::Command;
use std::rc::Rc;

use flamewm_integrations_linux::polkit_agent::{AuthOutcome, PolkitAgent, PolkitEvent};
use flamewm_integrations_linux::polkit_helper::{
    drive, helper_path, spawn_helper, user_name_for_uid, HelperConversation, HelperStep,
};
use flamewm_reactor::{ChildPipe, FdAction, ReactorHandle};

use crate::polkit_dialog::{PolkitDialog, Submission};

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
        move |cookie, outcome| {
            if let Err(error) = agent.borrow_mut().finish(cookie, outcome) {
                eprintln!("flamewm-shell: polkit reply failed: {error}");
            }
            dialog
                .borrow_mut()
                .apply(PolkitEvent::Dismiss(cookie.to_owned()));
        },
    )
}

/// Runs `command` (the helper, or a test double) for `user` and answers its prompt with the
/// submitted password. `settle` gets the cookie and the outcome exactly once.
///
/// # Errors
/// Returns the spawn or registration error. Errors after registration are reported as `Failed`.
pub fn start_with<F>(
    reactor: &ReactorHandle,
    command: Command,
    user: &str,
    submission: Submission,
    settle: F,
) -> io::Result<()>
where
    F: FnMut(&str, AuthOutcome) + 'static,
{
    let cookie = submission.cookie.clone();
    let pipe = spawn_helper(command, user, &cookie)?;
    let mut settle = settle;
    let mut pending = Some(submission);
    let mut conversation = HelperConversation::default();
    reactor
        .register_child_pipe(pipe, move |_fd, pipe: &mut ChildPipe, _| {
            let step = match drive(pipe, &mut conversation) {
                Ok(step) => step,
                Err(error) => {
                    eprintln!("flamewm-shell: polkit helper read failed: {error}");
                    let _ = pipe.kill();
                    settle(&cookie, AuthOutcome::Failed);
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
                    if answered {
                        conversation.answered();
                        FdAction::Continue
                    } else {
                        let _ = pipe.kill();
                        settle(&cookie, AuthOutcome::Failed);
                        FdAction::Remove
                    }
                }
                HelperStep::Done(outcome) => {
                    let _ = pipe.wait();
                    settle(&cookie, outcome);
                    FdAction::Remove
                }
            }
        })
        .map(|_| ())
        .map_err(|error| io::Error::other(format!("register polkit helper: {error}")))
}

/// Cancels the active prompt: the pending call gets `Error.Cancelled`, and the next prompt, if
/// any, becomes active.
pub fn cancel_active(agent: &Rc<RefCell<PolkitAgent>>, dialog: &Rc<RefCell<PolkitDialog>>) {
    let Some(cookie) = dialog.borrow_mut().cancel() else {
        return;
    };
    if let Err(error) = agent.borrow_mut().finish(&cookie, AuthOutcome::Cancelled) {
        eprintln!("flamewm-shell: polkit cancel reply failed: {error}");
    }
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

    fn submission(typed: &str) -> Submission {
        let mut dialog = PolkitDialog::default();
        dialog.apply(PolkitEvent::Show(BeginRequest {
            action_id: "org.example.action".into(),
            message: "Authentication is required".into(),
            icon_name: "dialog-password".into(),
            details: Vec::new(),
            cookie: "cookie-x".into(),
            identities: vec![Identity {
                kind: "unix-user".into(),
                uid: Some(1000),
            }],
        }));
        dialog.active_mut().expect("active").input(typed);
        dialog.submit().expect("submission")
    }

    fn run(typed: &str) -> Vec<(String, AuthOutcome)> {
        let settled = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&settled);
        let mut command = Command::new("sh");
        command.arg("-c").arg(FAKE).arg("sh");
        let mut reactor = Reactor::new().expect("reactor");
        start_with(
            &reactor.handle(),
            command,
            "alice",
            submission(typed),
            move |cookie, outcome| sink.borrow_mut().push((cookie.to_owned(), outcome)),
        )
        .expect("start");
        let deadline = Instant::now() + Duration::from_secs(5);
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
            run("hunter2"),
            vec![("cookie-x".to_owned(), AuthOutcome::Success)]
        );
    }

    #[test]
    fn wrong_password_settles_failed_once() {
        assert_eq!(
            run("nope"),
            vec![("cookie-x".to_owned(), AuthOutcome::Failed)]
        );
    }
}
