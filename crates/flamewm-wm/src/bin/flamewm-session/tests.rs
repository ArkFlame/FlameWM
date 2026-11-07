use super::*;
use flamewm_session_core::{Keyring, ProcessSupervisorPolicy, SessionRole, SupervisorPolicy};
use std::collections::BTreeMap;
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Output};

#[test]
fn successful_wm_exit_stops_session_without_restarting_it() {
    assert!(clean_wm_exit(0, ExitStatus::from_raw(0)));
    assert!(!clean_wm_exit(0, ExitStatus::from_raw(1 << 8)));
    assert!(!clean_wm_exit(1, ExitStatus::from_raw(0)));
}

#[test]
fn logout_cleanup_reaps_only_managed_children_without_restart() {
    let (reader, _writer) = UnixStream::pair().unwrap();
    let _ = WAKEUP.set(reader);
    let make_plan = |role| ProcessPlan {
        role,
        program: if role == SessionRole::WindowManager {
            "/bin/true"
        } else {
            "/bin/sleep"
        }
        .into(),
        arguments: if role == SessionRole::WindowManager {
            Vec::new()
        } else {
            vec!["30".into()]
        },
        environment: BTreeMap::new(),
        required: true,
    };
    let wm_plan = make_plan(SessionRole::WindowManager);
    let mut wm_child = Command::new(&wm_plan.program).spawn().unwrap();
    let wm_pid = wm_child.id();
    let wm_status = wm_child.wait().unwrap();
    assert!(clean_wm_exit(0, wm_status));
    let wm = Running {
        plan: wm_plan,
        child: wm_child,
        supervisor: ProcessSupervisorPolicy::new(SupervisorPolicy {
            graceful_stop_ms: 5,
            ..SupervisorPolicy::default()
        }),
    };
    let desktop_plan = make_plan(SessionRole::Desktop);
    let desktop_child = Command::new(&desktop_plan.program)
        .args(&desktop_plan.arguments)
        .spawn()
        .unwrap();
    let desktop_pid = desktop_child.id();
    let desktop = Running {
        plan: desktop_plan,
        child: desktop_child,
        supervisor: ProcessSupervisorPolicy::new(SupervisorPolicy {
            graceful_stop_ms: 5,
            ..SupervisorPolicy::default()
        }),
    };
    let mut children = vec![wm, desktop];
    let managed_pids = [wm_pid, desktop_pid];
    let mut unrelated = Command::new("/bin/sleep").arg("30").spawn().unwrap();

    let cleanup = stop_all(&mut children);
    let unrelated_status = unrelated.try_wait().unwrap();
    let _ = unrelated.kill();
    let _ = unrelated.wait();

    assert!(cleanup.is_ok(), "managed child cleanup failed: {cleanup:?}");
    assert!(children.is_empty(), "supervisor retained managed children");
    assert!(
        managed_pids
            .iter()
            .all(|pid| !std::path::Path::new(&format!("/proc/{pid}")).exists()),
        "managed child remained alive"
    );
    assert!(
        unrelated_status.is_none(),
        "cleanup affected an unrelated process"
    );
}

#[test]
fn activation_clears_kde_identity_and_keeps_only_the_selected_wallet_compatibility_version() {
    let live = BTreeMap::from([
        ("DISPLAY".to_owned(), ":0".to_owned()),
        ("XDG_CURRENT_DESKTOP".to_owned(), "FlameWM".to_owned()),
        ("KDE_SESSION_VERSION".to_owned(), "6".to_owned()),
    ]);
    let inherited = BTreeMap::from([
        ("XDG_CURRENT_DESKTOP".to_owned(), "KDE".to_owned()),
        ("KDE_FULL_SESSION".to_owned(), "true".to_owned()),
        ("KDE_SESSION_VERSION".to_owned(), "5".to_owned()),
        ("KDE_SESSION_UID".to_owned(), "1000".to_owned()),
        (
            "PAM_KWALLET5_LOGIN".to_owned(),
            "/private/socket".to_owned(),
        ),
    ]);
    let args = activation_assignments(&live, &inherited);
    assert!(args.contains(&"XDG_CURRENT_DESKTOP=FlameWM".to_owned()));
    assert!(args.contains(&"KDE_FULL_SESSION=".to_owned()));
    assert!(args.contains(&"KDE_SESSION_VERSION=".to_owned()));
    assert!(!args.contains(&"KDE_SESSION_VERSION=6".to_owned()));
    assert!(args.contains(&"KDE_SESSION_UID=".to_owned()));
    assert!(!args.iter().any(|arg| arg.contains("PAM_KWALLET5_LOGIN")));
    let generic = BTreeMap::from([("XDG_CURRENT_DESKTOP".to_owned(), "FlameWM".to_owned())]);
    let selected = activation_assignments(&generic, &inherited);
    assert!(selected.contains(&"KDE_SESSION_VERSION=".to_owned()));
}

#[test]
fn keyring_start_stdout_reaches_children_and_activation_assignments() {
    let mut start_called = false;
    let keyring_environment = start_session_keyring_with(Keyring::GnomeKeyring, || {
        start_called = true;
        Ok(Output {
            status: ExitStatus::from_raw(0),
            stdout: b"GNOME_KEYRING_CONTROL=/run/user/1000/keyring\nGPG_AGENT_INFO=/run/user/1000/keyring/gpg:0:1\nSSH_AUTH_SOCK=/run/user/1000/keyring/ssh\nSSH_AGENT_PID=1234\n".to_vec(),
            stderr: Vec::new(),
        })
    });
    assert!(start_called);
    let make_plan = |role| ProcessPlan {
        role,
        program: "/bin/true".into(),
        arguments: Vec::new(),
        environment: BTreeMap::new(),
        required: true,
    };
    let mut processes = [
        make_plan(SessionRole::WindowManager),
        make_plan(SessionRole::Desktop),
        make_plan(SessionRole::Shell),
    ];

    add_keyring_environment(&mut processes, &keyring_environment);

    for process in &processes {
        assert_eq!(process.environment, keyring_environment);
    }
    let activation = activation_assignments(&processes[0].environment, &BTreeMap::new());
    assert!(activation.contains(&"GNOME_KEYRING_CONTROL=/run/user/1000/keyring".to_owned()));
    assert!(activation.contains(&"GPG_AGENT_INFO=/run/user/1000/keyring/gpg:0:1".to_owned()));
    assert!(activation.contains(&"SSH_AUTH_SOCK=/run/user/1000/keyring/ssh".to_owned()));
    assert!(activation.contains(&"SSH_AGENT_PID=1234".to_owned()));
}

#[test]
fn session_child_environment_removes_inherited_kde_markers_but_keeps_explicit_app_env() {
    let inherited = BTreeMap::from([
        ("KDE_FULL_SESSION".to_owned(), "true".to_owned()),
        ("KDE_SESSION_VERSION".to_owned(), "5".to_owned()),
        ("KDE_SESSION_UID".to_owned(), "1000".to_owned()),
    ]);
    let planned = BTreeMap::from([
        ("KDE_SESSION_VERSION".to_owned(), "6".to_owned()),
        ("FLAMEWM_TEST_ENV".to_owned(), "session".to_owned()),
    ]);
    let mut command = Command::new("/bin/sh");
    command.args([
        "-c",
        "printf '%s|%s|%s|%s' \"${KDE_FULL_SESSION:-<unset>}\" \"${KDE_SESSION_VERSION:-<unset>}\" \"${KDE_SESSION_UID:-<unset>}\" \"$FLAMEWM_TEST_ENV\"",
    ]);
    command.envs(&inherited);
    apply_session_environment(&mut command, &planned);
    let output = command.output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "<unset>|<unset>|<unset>|session"
    );

    let mut compatibility = planned;
    clear_kde_session_identity(&mut compatibility);
    assert!(!compatibility.contains_key("KDE_FULL_SESSION"));
    assert!(!compatibility.contains_key("KDE_SESSION_VERSION"));
    assert!(!compatibility.contains_key("KDE_SESSION_UID"));
    assert_eq!(
        compatibility.get("FLAMEWM_TEST_ENV").map(String::as_str),
        Some("session")
    );
}

#[test]
fn explicit_brave_wallet_selection_survives_until_compatibility_setup() {
    let existing = BTreeMap::from([(
        "FLAMEWM_BRAVE_PASSWORD_STORE".to_owned(),
        "kwallet5".to_owned(),
    )]);
    let mut processes = [ProcessPlan {
        role: SessionRole::WindowManager,
        program: "/bin/true".into(),
        arguments: Vec::new(),
        environment: BTreeMap::new(),
        required: true,
    }];

    add_app_compatibility_environment(&mut processes, &existing);
    assert_eq!(
        processes[0]
            .environment
            .get("KDE_SESSION_VERSION")
            .map(String::as_str),
        Some("5")
    );
    clear_kde_session_identity(&mut processes[0].environment);
    assert!(!processes[0].environment.contains_key("KDE_SESSION_VERSION"));
}

#[test]
fn activation_snapshot_restores_only_touched_systemd_keys() {
    let keys = vec![
        "DISPLAY".to_owned(),
        "KDE_FULL_SESSION".to_owned(),
        "KDE_SESSION_VERSION".to_owned(),
        "KDE_SESSION_UID".to_owned(),
        "GNOME_KEYRING_CONTROL".to_owned(),
        "GPG_AGENT_INFO".to_owned(),
        "SSH_AUTH_SOCK".to_owned(),
        "SSH_AGENT_PID".to_owned(),
    ];
    let mut snapshot = ActivationEnvironmentSnapshot::from_show_environment(
        keys.clone(),
        "DISPLAY=:8\nKDE_FULL_SESSION=true\nKDE_SESSION_VERSION=5\nGNOME_KEYRING_CONTROL=/run/user/1000/old-keyring\nGPG_AGENT_INFO=/run/user/1000/old-keyring/gpg:321:1\nSSH_AUTH_SOCK=/run/user/1000/old-keyring/ssh\nSSH_AGENT_PID=9876\nUNRELATED=value\n",
    );
    snapshot.restored = true;
    assert_eq!(snapshot.keys, keys);
    assert_eq!(
        snapshot.previous,
        vec![
            "DISPLAY=:8".to_owned(),
            "KDE_FULL_SESSION=true".to_owned(),
            "KDE_SESSION_VERSION=5".to_owned(),
            "GNOME_KEYRING_CONTROL=/run/user/1000/old-keyring".to_owned(),
            "GPG_AGENT_INFO=/run/user/1000/old-keyring/gpg:321:1".to_owned(),
            "SSH_AUTH_SOCK=/run/user/1000/old-keyring/ssh".to_owned(),
            "SSH_AGENT_PID=9876".to_owned()
        ]
    );
    assert_eq!(
        snapshot.dbus_restore_assignments(),
        vec![
            "DISPLAY=:8".to_owned(),
            "KDE_FULL_SESSION=true".to_owned(),
            "KDE_SESSION_VERSION=5".to_owned(),
            "KDE_SESSION_UID=".to_owned(),
            "GNOME_KEYRING_CONTROL=/run/user/1000/old-keyring".to_owned(),
            "GPG_AGENT_INFO=/run/user/1000/old-keyring/gpg:321:1".to_owned(),
            "SSH_AUTH_SOCK=/run/user/1000/old-keyring/ssh".to_owned(),
            "SSH_AGENT_PID=9876".to_owned()
        ]
    );
    let args = snapshot.busctl_arguments();
    assert_eq!(args[5], "UnsetAndSetEnvironment");
    assert_eq!(args[6], "as");
    assert_eq!(args[7], "8");
    assert_eq!(args[16], "as");
    assert_eq!(args[17], "7");
    assert!(args.contains(&"KDE_SESSION_VERSION=5".to_owned()));
    assert!(!args.contains(&"UNRELATED=value".to_owned()));
}

#[test]
fn activation_snapshot_returns_mocked_restore_failure_and_attempts_both_backends() {
    let mut snapshot = ActivationEnvironmentSnapshot::from_show_environment(
        vec!["DISPLAY".to_owned()],
        "DISPLAY=:8\n",
    );
    let mut restored_programs = Vec::new();

    let result = snapshot.restore_with(|program, _arguments| {
        restored_programs.push(program.to_owned());
        if program == "dbus-update-activation-environment" {
            Err("fixture failure".to_owned())
        } else {
            Ok(())
        }
    });

    assert_eq!(
        result,
        Err("D-Bus restore failed: fixture failure".to_owned())
    );
    assert_eq!(
        restored_programs,
        vec![
            "dbus-update-activation-environment".to_owned(),
            "busctl".to_owned()
        ]
    );
}

#[test]
fn activation_restore_failure_fails_success_and_preserves_session_failure() {
    let restore_failure = Err("D-Bus restore failed".to_owned());
    assert_eq!(
        combine_activation_restore_result(Ok(()), restore_failure.clone()),
        Err("activation environment restore failed: D-Bus restore failed".to_owned())
    );
    assert_eq!(
        combine_activation_restore_result(Err("session failed".to_owned()), restore_failure),
        Err(
            "session failed; activation environment restore failed: D-Bus restore failed"
                .to_owned()
        )
    );
}

#[test]
fn activation_snapshot_keys_come_only_from_published_assignments() {
    let assignments = vec![
        "DISPLAY=:0".to_owned(),
        "KDE_SESSION_VERSION=".to_owned(),
        "DISPLAY=:1".to_owned(),
    ];
    assert_eq!(
        activation_assignment_keys(&assignments),
        vec!["DISPLAY".to_owned(), "KDE_SESSION_VERSION".to_owned()]
    );
}

#[test]
fn scan_exits_reports_every_exited_child_before_blocking() {
    let mixed = scan_exits(3, |index| {
        Ok(if index == 1 { None } else { Some(index as i32) })
    })
    .unwrap();
    assert_eq!(mixed, vec![(2, 2), (0, 0)]);
    let both = scan_exits(2, |index| Ok(Some(index as i32))).unwrap();
    assert_eq!(both, vec![(1, 1), (0, 0)]);
    assert!(
        scan_exits(2, |_| Ok::<Option<i32>, String>(None))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        scan_exits(2, |_| Err::<Option<i32>, _>("poll".to_owned())),
        Err("poll".to_owned())
    );
}

#[test]
fn allocator_tuning_fills_only_unset_keys() {
    let all = allocator_tuning_defaults(|_| false);
    assert_eq!(
        all,
        vec![
            ("MALLOC_ARENA_MAX", "2"),
            ("MALLOC_TRIM_THRESHOLD_", "131072"),
            ("MALLOC_MMAP_THRESHOLD_", "131072"),
        ]
    );
    let user_arena = allocator_tuning_defaults(|key| key == "MALLOC_ARENA_MAX");
    assert_eq!(user_arena.len(), 2);
    assert!(user_arena.iter().all(|(key, _)| *key != "MALLOC_ARENA_MAX"));
    assert!(allocator_tuning_defaults(|_| true).is_empty());
}

#[test]
fn supporting_wm_check_requires_matching_nonzero_self_reference() {
    assert!(supporting_wm_check_is_valid(Some(42), Some(42)));
    assert!(!supporting_wm_check_is_valid(None, Some(42)));
    assert!(!supporting_wm_check_is_valid(Some(42), None));
    assert!(!supporting_wm_check_is_valid(Some(42), Some(43)));
    assert!(!supporting_wm_check_is_valid(Some(0), Some(0)));
}
