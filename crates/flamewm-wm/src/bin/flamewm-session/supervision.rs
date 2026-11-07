use super::*;

pub(super) struct Running {
    pub(super) plan: ProcessPlan,
    pub(super) child: Child,
    pub(super) supervisor: ProcessSupervisorPolicy,
}

/// Starts a non-critical component (desktop or shell). When its binary is missing, cannot be
/// spawned, or exits during startup, the plan is replaced by the fallback process so the WM and
/// the other components keep running. Supervision errors and parent death still abort.
pub(super) fn start_optional(
    plan: &mut ProcessPlan,
    parent: Pid,
    stop_requested: &AtomicBool,
) -> Result<Running, String> {
    let label = role_label(plan.role);
    if session_diagnostics::is_runnable(&plan.program) {
        match spawn(plan.clone()) {
            Ok(mut running) => match startup_grace(&mut running, parent, stop_requested) {
                Ok(()) => return Ok(running),
                Err(error) if running.child.try_wait().ok().flatten().is_none() => {
                    let _ = running.child.kill();
                    let _ = running.child.wait();
                    return Err(error);
                }
                Err(error) => session_diagnostics::log(label, &error),
            },
            Err(error) => session_diagnostics::log(label, &error),
        }
    } else {
        session_diagnostics::log(
            label,
            &format!("{} is missing or not executable", plan.program.display()),
        );
    }
    session_diagnostics::log(
        label,
        "starting the fallback instead; the session continues",
    );
    *plan = fallback_plan(plan, &log_path_text());
    spawn(plan.clone())
}

pub(super) fn role_label(role: SessionRole) -> &'static str {
    match role {
        SessionRole::WindowManager => "window manager",
        SessionRole::Desktop => "desktop",
        SessionRole::Shell => "shell",
    }
}

pub(super) fn spawn(plan: ProcessPlan) -> Result<Running, String> {
    let mut command = Command::new(&plan.program);
    let (stdout, stderr) = session_diagnostics::child_stdio();
    command
        .args(&plan.arguments)
        .env(
            "FLAMEWM_SESSION_SUPERVISOR_PID",
            std::process::id().to_string(),
        )
        .env_remove("FLAMEWM_SESSION_LOGOUT_OWNER")
        .stdout(stdout)
        .stderr(stderr);
    apply_session_environment(&mut command, &plan.environment);
    if plan.role == SessionRole::WindowManager {
        command.env("FLAMEWM_SESSION_LOGOUT_OWNER", "1");
    }
    for (key, value) in allocator_tuning_defaults(|key| {
        env::var_os(key).is_some() || plan.environment.contains_key(key)
    }) {
        command.env(key, value);
    }
    let child = command.spawn().map_err(|error| {
        format!(
            "spawn {:?} at {}: {error}",
            plan.role,
            plan.program.display()
        )
    })?;
    Ok(Running {
        plan,
        child,
        supervisor: ProcessSupervisorPolicy::default(),
    })
}

pub(super) fn clean_wm_exit(index: usize, status: ExitStatus) -> bool {
    index == 0 && status.success()
}

/// Polls every child without early exit and returns exits in descending index order, so a
/// single pass sees all simultaneous exits before the supervisor decides to block.
pub(super) fn scan_exits<T>(
    count: usize,
    mut poll: impl FnMut(usize) -> Result<Option<T>, String>,
) -> Result<Vec<(usize, T)>, String> {
    let mut exits = Vec::new();
    for index in (0..count).rev() {
        if let Some(status) = poll(index)? {
            exits.push((index, status));
        }
    }
    Ok(exits)
}

pub(super) fn supervise(
    children: &mut Vec<Running>,
    parent: rustix::process::Pid,
    role_plans: &[ProcessPlan],
    stop_requested: &AtomicBool,
) -> Result<(), String> {
    loop {
        check_parent(parent, stop_requested)?;
        let exits = scan_exits(children.len(), |index| {
            children[index]
                .child
                .try_wait()
                .map_err(|error| child_error(&children[index], "supervise", error))
        })?;
        if let Some(&(index, status)) = exits.first() {
            let exit = status_to_exit(status);
            session_diagnostics::log(
                role_label(children[index].plan.role),
                &format!(
                    "{} {}",
                    children[index].plan.program.display(),
                    describe_exit(exit)
                ),
            );
            if clean_wm_exit(index, status) {
                children[0].supervisor.request_stop();
                return Ok(());
            }
            match children[index].supervisor.child_exited(exit) {
                SupervisorDecision::GiveUp if index != 0 => {
                    session_diagnostics::log(
                        role_label(children[index].plan.role),
                        "restart budget exhausted; starting the fallback instead",
                    );
                    children[index] = spawn(fallback_plan(&role_plans[index], &log_path_text()))?;
                }
                SupervisorDecision::RestartAfter(delay) => {
                    if index == 0 {
                        stop_dependents(children)?;
                    } else if index == 1 && children.len() > 2 {
                        stop_role(&mut children[2])?;
                        children.truncate(2);
                    }
                    wait_backoff(delay, parent, stop_requested)?;
                    let plan = role_plans[index].clone();
                    let supervisor = children[index].supervisor.clone();
                    children[index] = spawn(plan)?;
                    children[index].supervisor = supervisor;
                    if index == 0 {
                        wait_ready(&mut children[0], parent, stop_requested)?;
                        // Dependents reuse the original launch plans, strictly after WM readiness.
                        for plan in role_plans.iter().skip(1) {
                            children.push(spawn(plan.clone())?);
                            let child = children.last_mut().ok_or_else(|| {
                                "supervisor invariant violated: spawned dependent is missing after WM restart".to_owned()
                            })?;
                            startup_grace(child, parent, stop_requested)?;
                        }
                    } else {
                        startup_grace(&mut children[index], parent, stop_requested)?;
                        if index == 1 && children.len() == 2 {
                            children.push(spawn(role_plans[2].clone())?);
                            let child = children.last_mut().ok_or_else(|| {
                                "supervisor invariant violated: spawned shell is missing after desktop restart".to_owned()
                            })?;
                            startup_grace(child, parent, stop_requested)?;
                        }
                    }
                }
                SupervisorDecision::GiveUp => {
                    return Err(format!(
                        "{:?} at {} exhausted its restart budget",
                        children[index].plan.role,
                        children[index].plan.program.display()
                    ));
                }
                SupervisorDecision::StopCleanly => return Ok(()),
                SupervisorDecision::SpawnNow => {
                    return Err(format!(
                        "supervisor invariant violated: {:?} at {} requested immediate spawn after exit",
                        children[index].plan.role,
                        children[index].plan.program.display()
                    ));
                }
            }
        } else {
            // No exit pending: block until SIGCHLD/stop or the safety timeout. A handled exit
            // skips this block and re-scans, so no second simultaneous exit is left as a zombie.
            wait_for_wakeup(Some(SUPERVISOR_SAFETY_TIMEOUT))?;
        }
    }
}

pub(super) fn wait_backoff(
    delay_ms: u64,
    parent: rustix::process::Pid,
    stop_requested: &AtomicBool,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(delay_ms);
    while Instant::now() < deadline {
        check_parent(parent, stop_requested)?;
        wait_for_wakeup(Some(deadline.saturating_duration_since(Instant::now())))?;
    }
    check_parent(parent, stop_requested)
}

pub(super) fn stop_dependents(children: &mut Vec<Running>) -> Result<(), String> {
    let mut failures = Vec::new();
    while children.len() > 1 {
        if let Some(mut child) = children.pop() {
            if let Err(error) = stop_role(&mut child) {
                failures.push(error);
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

pub(super) fn stop_all(children: &mut Vec<Running>) -> Result<(), String> {
    let mut failures = Vec::new();
    while let Some(mut child) = children.pop() {
        child.supervisor.request_stop();
        if let Err(error) = stop_role(&mut child) {
            failures.push(error);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

pub(super) fn stop_role(child: &mut Running) -> Result<(), String> {
    child.supervisor.request_stop();
    let mut failures = Vec::new();
    let mut exited = match child.child.try_wait() {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(error) => {
            failures.push(child_error(child, "check before graceful stop", error));
            true
        }
    };
    if !exited {
        if let Some(pid) = Pid::from_raw(child.child.id() as i32) {
            if let Err(error) = kill_process(pid, Signal::TERM) {
                if error != rustix::io::Errno::SRCH {
                    failures.push(format!("terminate {:?}: {error}", child.plan.role));
                }
            }
        }
    }
    let deadline = Instant::now() + Duration::from_millis(child.supervisor.graceful_stop_ms());
    loop {
        if exited {
            break;
        }
        match child.child.try_wait() {
            Ok(Some(_)) => {
                exited = true;
                break;
            }
            Ok(None) if Instant::now() < deadline => {
                if let Err(error) =
                    wait_for_wakeup(Some(deadline.saturating_duration_since(Instant::now())))
                {
                    failures.push(error);
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                failures.push(child_error(child, "check during graceful stop", error));
                break;
            }
        }
    }
    if !exited {
        if let Err(error) = child.child.kill() {
            if error.kind() != std::io::ErrorKind::NotFound {
                failures.push(format!("force-kill {:?}: {error}", child.plan.role));
            }
        }
    }
    match child.child.wait() {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => failures.push(format!("reap {:?}: {error}", child.plan.role)),
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

pub(super) fn status_to_exit(status: ExitStatus) -> ChildExit {
    if status.success() {
        ChildExit::Success
    } else if let Some(code) = status.code() {
        ChildExit::ExitCode(code)
    } else if let Some(signal) = status.signal() {
        ChildExit::Signal(signal)
    } else {
        ChildExit::ExitCode(-1)
    }
}

pub(super) fn child_error(child: &Running, action: &str, error: std::io::Error) -> String {
    format!(
        "{action} {:?} at {}: {error}",
        child.plan.role,
        child.plan.program.display()
    )
}
