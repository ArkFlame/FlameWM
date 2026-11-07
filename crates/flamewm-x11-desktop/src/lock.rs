use super::*;

pub(super) const LOCK_SCREEN_BINARY: &str = "flamewm-lock";

/// What the Lock session action runs.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum LockProgram {
    Screen(PathBuf),
    Loginctl,
}

/// Prefers `flamewm-lock` next to the running executable, then on `PATH`,
/// and falls back to `loginctl lock-session` when no executable is found.
pub(super) fn resolve_lock_program(
    exe_dir: Option<&Path>,
    path_var: Option<&OsStr>,
) -> LockProgram {
    let on_path = path_var
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(LOCK_SCREEN_BINARY));
    exe_dir
        .map(|dir| dir.join(LOCK_SCREEN_BINARY))
        .into_iter()
        .chain(on_path)
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
        .map_or(LockProgram::Loginctl, LockProgram::Screen)
}

/// Delay before a crashed lock screen is respawned.
pub(super) const LOCK_RESPAWN_DELAY: Duration = Duration::from_millis(500);
/// Respawns allowed inside [`LOCK_RESPAWN_WINDOW`] before supervision gives up.
pub(super) const LOCK_RESPAWN_CAP: usize = 5;
pub(super) const LOCK_RESPAWN_WINDOW: Duration = Duration::from_secs(30);

/// What the lock supervisor does after the lock process exits.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum LockSupervision {
    Respawn { delay: Duration },
    Stop,
}

/// A successful exit means the user unlocked, so supervision ends. Any other
/// exit (non-zero status or a signal such as SIGKILL, which drops the grabs)
/// respawns the lock until the crash-loop cap is reached.
pub(super) fn decide(exit_status: ExitStatus, recent_restarts: usize) -> LockSupervision {
    if exit_status.success() || recent_restarts >= LOCK_RESPAWN_CAP {
        LockSupervision::Stop
    } else {
        LockSupervision::Respawn {
            delay: LOCK_RESPAWN_DELAY,
        }
    }
}

pub(super) fn spawn_lock_process(program: &Path, args: &[&str]) -> std::io::Result<Child> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
}

/// Owns the lock child for the lifetime of the lock screen: respawns it per
/// [`decide`] and returns once supervision stops.
pub(super) fn supervise_lock(program: PathBuf, mut child: Child) {
    let mut restarts: VecDeque<Instant> = VecDeque::new();
    loop {
        let status = match child.wait() {
            Ok(status) => status,
            Err(error) => {
                eprintln!("flamewm: lock screen wait failed: {error}");
                return;
            }
        };
        let now = Instant::now();
        while restarts
            .front()
            .is_some_and(|at| now.duration_since(*at) > LOCK_RESPAWN_WINDOW)
        {
            restarts.pop_front();
        }
        match decide(status, restarts.len()) {
            LockSupervision::Stop => {
                if !status.success() {
                    eprintln!(
                        "flamewm: lock screen exited ({status}) and crash-loop cap reached; not respawning"
                    );
                }
                return;
            }
            LockSupervision::Respawn { delay } => {
                thread::sleep(delay);
                restarts.push_back(Instant::now());
                match spawn_lock_process(&program, &[]) {
                    Ok(next) => child = next,
                    Err(error) => {
                        eprintln!("flamewm: lock screen respawn failed: {error}");
                        return;
                    }
                }
            }
        }
    }
}

/// Starts the lock screen. The lock binary is spawned detached; a supervisor
/// thread respawns it if it crashes so the screen cannot silently unlock.
pub fn start_lock_screen() -> FlameResult<()> {
    match current_lock_program() {
        LockProgram::Screen(program) => {
            let child = spawn_lock_process(&program, &[]).map_err(FlameError::io)?;
            thread::Builder::new()
                .name("flamewm-lock-supervisor".to_owned())
                .spawn(move || supervise_lock(program, child))
                .map(|_| ())
                .map_err(FlameError::io)
        }
        LockProgram::Loginctl => Command::new("loginctl")
            .arg("lock-session")
            .status()
            .map_err(FlameError::io)?
            .success()
            .then_some(())
            .ok_or_else(|| {
                FlameError::new(ErrorCode::PermissionDenied, "session action command failed")
            }),
    }
}

pub(super) fn current_lock_program() -> LockProgram {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let path_var = std::env::var_os("PATH");
    resolve_lock_program(exe_dir.as_deref(), path_var.as_deref())
}

/// Argument vector for the preview lock: it takes no grabs and exits on any input.
pub(super) const LOCK_PREVIEW_ARGS: &[&str] = &["--preview"];

/// Argument vector for the screensaver preview: ribbons only, no grabs, no PAM, exits on input.
pub(super) const SCREENSAVER_PREVIEW_ARGS: &[&str] = &["--preview-screensaver"];

/// The single live lock preview child. Reaped with `try_wait` before each spawn decision.
pub(super) static LOCK_PREVIEW: Mutex<Option<Child>> = Mutex::new(None);

/// The single live screensaver preview child, tracked separately from the lock preview.
pub(super) static SCREENSAVER_PREVIEW: Mutex<Option<Child>> = Mutex::new(None);

/// Spawns a preview unless `slot` still holds a running child. An exited child is reaped first.
pub(super) fn spawn_preview_once(
    slot: &mut Option<Child>,
    program: &Path,
    args: &[&str],
) -> std::io::Result<()> {
    if let Some(child) = slot.as_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            return Ok(());
        }
    }
    *slot = Some(spawn_lock_process(program, args)?);
    Ok(())
}

/// Starts one preview of `slot`'s kind. At most one preview of each kind runs at a time.
pub(super) fn start_preview(slot: &Mutex<Option<Child>>, args: &[&str]) -> FlameResult<()> {
    match current_lock_program() {
        LockProgram::Screen(program) => {
            let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
            spawn_preview_once(&mut slot, &program, args).map_err(FlameError::io)
        }
        LockProgram::Loginctl => Err(FlameError::new(
            ErrorCode::NotFound,
            "flamewm-lock binary not found; lock preview unavailable",
        )),
    }
}

/// Starts the lock screen in preview mode. At most one preview runs at a time.
pub fn start_lock_preview() -> FlameResult<()> {
    start_preview(&LOCK_PREVIEW, LOCK_PREVIEW_ARGS)
}

/// Starts the screensaver preview. At most one preview runs at a time.
pub fn start_screensaver_preview() -> FlameResult<()> {
    start_preview(&SCREENSAVER_PREVIEW, SCREENSAVER_PREVIEW_ARGS)
}

#[cfg(test)]
pub(super) fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("flamewm-lock-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[cfg(test)]
mod lock_preview_tests {
    use super::*;

    /// Fake lock binary: logs its argv once per launch, then runs until killed.
    fn fake_lock(dir: &Path) -> PathBuf {
        let program = dir.join(LOCK_SCREEN_BINARY);
        let body = format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nexec sleep 30\n",
            dir.join("launches.log").display()
        );
        std::fs::write(&program, body).expect("write fake lock");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake lock");
        program
    }

    fn launches(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("launches.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn wait_for_launches(dir: &Path, count: usize) {
        // 10 s: the fake lock is a shell script and the workspace suite runs under heavy load.
        for _ in 0..1000 {
            if launches(dir).len() >= count {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("expected {count} launches, saw {:?}", launches(dir));
    }

    /// Spawns the preview, retrying `ETXTBSY`: exec of a just-written script races with a fork
    /// in a parallel test thread that still holds the write fd.
    fn spawn_retrying(slot: &mut Option<Child>, program: &Path) -> std::io::Result<()> {
        spawn_retrying_with(slot, program, LOCK_PREVIEW_ARGS)
    }

    fn spawn_retrying_with(
        slot: &mut Option<Child>,
        program: &Path,
        args: &[&str],
    ) -> std::io::Result<()> {
        let mut last = None;
        for _ in 0..100 {
            match spawn_preview_once(slot, program, args) {
                Err(error) if error.raw_os_error() == Some(26) => {
                    last = Some(error);
                    std::thread::sleep(Duration::from_millis(20));
                }
                other => return other,
            }
        }
        Err(last.expect("retried at least once"))
    }

    #[test]
    fn preview_argument_vector_is_exactly_preview_flag() {
        assert_eq!(LOCK_PREVIEW_ARGS, ["--preview"]);
    }

    #[test]
    fn screensaver_preview_argument_vector_is_exactly_its_flag() {
        assert_eq!(SCREENSAVER_PREVIEW_ARGS, ["--preview-screensaver"]);
    }

    #[test]
    fn screensaver_preview_spawns_once_with_only_its_flag() {
        let dir = temp_dir("screensaver-argv");
        let program = fake_lock(&dir);
        let mut slot = None;
        spawn_retrying_with(&mut slot, &program, SCREENSAVER_PREVIEW_ARGS).expect("spawn");
        spawn_preview_once(&mut slot, &program, SCREENSAVER_PREVIEW_ARGS).expect("second spawn");
        wait_for_launches(&dir, 1);
        assert_eq!(launches(&dir), ["--preview-screensaver"]);
        let _ = slot.expect("child tracked").kill();
    }

    #[test]
    fn preview_spawns_fake_lock_with_only_the_preview_flag() {
        let dir = temp_dir("argv");
        let program = fake_lock(&dir);
        let mut slot = None;
        spawn_retrying(&mut slot, &program).expect("spawn preview");
        wait_for_launches(&dir, 1);
        assert_eq!(launches(&dir), ["--preview"]);
        let _ = slot.expect("child tracked").kill();
    }

    #[test]
    fn second_preview_is_refused_while_first_is_running() {
        let dir = temp_dir("single");
        let program = fake_lock(&dir);
        let mut slot = None;
        spawn_retrying(&mut slot, &program).expect("first preview");
        spawn_retrying(&mut slot, &program).expect("second call is a no-op");
        wait_for_launches(&dir, 1);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            launches(&dir).len(),
            1,
            "a second preview spawned while one runs"
        );
        let _ = slot.expect("child tracked").kill();
    }

    #[test]
    fn exited_preview_is_reaped_and_a_new_one_starts() {
        let dir = temp_dir("reap");
        let program = fake_lock(&dir);
        let mut slot = None;
        spawn_retrying(&mut slot, &program).expect("first preview");
        wait_for_launches(&dir, 1);
        let child = slot.as_mut().expect("child tracked");
        child.kill().expect("kill preview");
        child.wait().expect("reap preview");
        spawn_retrying(&mut slot, &program).expect("preview after exit");
        wait_for_launches(&dir, 2);
        let _ = slot.expect("child tracked").kill();
    }
}

#[cfg(test)]
mod lock_resolution_tests {
    use super::*;

    fn write_binary(dir: &Path, mode: u32) {
        let path = dir.join(LOCK_SCREEN_BINARY);
        std::fs::write(&path, b"#!/bin/sh\n").expect("write binary");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("chmod binary");
    }

    #[test]
    fn falls_back_to_loginctl_when_binary_absent() {
        let exe = temp_dir("absent-exe");
        let path = temp_dir("absent-path");
        let path_var = std::env::join_paths([&path]).expect("join");
        assert_eq!(
            resolve_lock_program(Some(&exe), Some(&path_var)),
            LockProgram::Loginctl
        );
        assert_eq!(resolve_lock_program(None, None), LockProgram::Loginctl);
    }

    #[test]
    fn prefers_sibling_of_current_executable() {
        let exe = temp_dir("sibling-exe");
        let path = temp_dir("sibling-path");
        write_binary(&exe, 0o755);
        write_binary(&path, 0o755);
        let path_var = std::env::join_paths([&path]).expect("join");
        assert_eq!(
            resolve_lock_program(Some(&exe), Some(&path_var)),
            LockProgram::Screen(exe.join(LOCK_SCREEN_BINARY))
        );
    }

    #[test]
    fn finds_binary_on_path_when_not_beside_executable() {
        let exe = temp_dir("path-exe");
        let path = temp_dir("path-dir");
        write_binary(&path, 0o755);
        let path_var = std::env::join_paths([&exe, &path]).expect("join");
        assert_eq!(
            resolve_lock_program(Some(&exe), Some(&path_var)),
            LockProgram::Screen(path.join(LOCK_SCREEN_BINARY))
        );
    }

    #[test]
    fn ignores_non_executable_binary() {
        let exe = temp_dir("noexec-exe");
        write_binary(&exe, 0o644);
        assert_eq!(
            resolve_lock_program(Some(&exe), None),
            LockProgram::Loginctl
        );
    }
}
#[cfg(test)]
mod lock_supervision_tests {
    use std::os::unix::process::ExitStatusExt;

    use super::*;

    /// Raw wait status: exit code lives in bits 8..16, a signal in bits 0..7.
    fn exited(code: i32) -> ExitStatus {
        ExitStatus::from_raw(code << 8)
    }

    fn killed(signal: i32) -> ExitStatus {
        ExitStatus::from_raw(signal)
    }

    #[test]
    fn successful_exit_unlocks_and_ends_supervision() {
        assert_eq!(decide(exited(0), 0), LockSupervision::Stop);
        assert_eq!(
            decide(exited(0), LOCK_RESPAWN_CAP - 1),
            LockSupervision::Stop
        );
    }

    #[test]
    fn nonzero_exit_and_sigkill_respawn_after_backoff() {
        let respawn = LockSupervision::Respawn {
            delay: Duration::from_millis(500),
        };
        assert_eq!(decide(exited(1), 0), respawn);
        assert_eq!(decide(killed(9), 0), respawn);
        assert_eq!(decide(killed(9), LOCK_RESPAWN_CAP - 1), respawn);
    }

    #[test]
    fn crash_loop_cap_stops_supervision() {
        assert_eq!(decide(killed(9), LOCK_RESPAWN_CAP), LockSupervision::Stop);
        assert_eq!(
            decide(exited(1), LOCK_RESPAWN_CAP + 3),
            LockSupervision::Stop
        );
    }
}
