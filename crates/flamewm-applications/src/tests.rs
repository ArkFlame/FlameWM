use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn spawn_reaped_leaves_no_zombie_after_the_child_exits() {
    let pid = spawn_reaped(&mut Command::new("true")).expect("spawn true");
    let proc_entry = PathBuf::from(format!("/proc/{pid}"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    // The waiter thread reaps the child, so its /proc entry disappears; a zombie would stay.
    while proc_entry.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!proc_entry.exists(), "child {pid} was not reaped");
}

fn scratch_apps_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "flamewm-rescan-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn desktop(name: &str) -> String {
    format!("[Desktop Entry]\nType=Application\nName={name}\nExec=run-{name}\n")
}

#[test]
fn rescan_reports_added_changed_and_removed_entries() {
    let dir = scratch_apps_dir("delta");
    fs::write(dir.join("keep.desktop"), desktop("Keep")).unwrap();
    fs::write(dir.join("gone.desktop"), desktop("Gone")).unwrap();
    let shared = SharedApplicationCatalog::new(
        ApplicationCatalog::from_dirs(std::slice::from_ref(&dir)).unwrap(),
    );

    fs::write(dir.join("keep.desktop"), desktop("Renamed")).unwrap();
    fs::remove_file(dir.join("gone.desktop")).unwrap();
    fs::write(dir.join("new.desktop"), desktop("New")).unwrap();
    let delta = shared.rescan_from_dirs(std::slice::from_ref(&dir)).unwrap();

    assert_eq!(delta.added, vec![DesktopAppId::new("new.desktop")]);
    assert_eq!(delta.removed, vec![DesktopAppId::new("gone.desktop")]);
    assert_eq!(delta.changed, vec![DesktopAppId::new("keep.desktop")]);
    let current = shared.current();
    assert_eq!(
        current
            .find(&DesktopAppId::new("keep.desktop"))
            .unwrap()
            .name(),
        "Renamed"
    );
    assert!(current.find(&DesktopAppId::new("gone.desktop")).is_err());
    let again = shared.rescan_from_dirs(std::slice::from_ref(&dir)).unwrap();
    assert!(again.is_empty(), "unchanged dirs must yield an empty delta");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn snapshots_taken_before_rescan_stay_valid() {
    let dir = scratch_apps_dir("snapshot");
    fs::write(dir.join("old.desktop"), desktop("Old")).unwrap();
    let shared = SharedApplicationCatalog::new(
        ApplicationCatalog::from_dirs(std::slice::from_ref(&dir)).unwrap(),
    );
    let before = shared.current();
    fs::remove_file(dir.join("old.desktop")).unwrap();
    shared.rescan_from_dirs(std::slice::from_ref(&dir)).unwrap();
    assert_eq!(before.entries().len(), 1);
    assert!(shared.current().entries().is_empty());
    let _ = fs::remove_dir_all(&dir);
}

fn fixture() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "flamewm-applications-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(path.join("applications/nested")).unwrap();
    fs::write(path.join("applications/nested/example.desktop"), "[Desktop Entry]\nType=Application\nName=Example Tool\nGenericName=Editor\nComment=Fixture\nKeywords=write;fixture;\nCategories=Utility;\nIcon=example\nExec=fixture %U --literal=DISPLAY=:9\n").unwrap();
    fs::write(
        path.join("applications/nested/hidden.desktop"),
        "[Desktop Entry]\nType=Application\nName=Hidden\nHidden=true\nExec=hidden\n",
    )
    .unwrap();
    path
}

#[test]
fn mime_handler_catalog_includes_nodisplay_and_ignores_one_bad_launcher() {
    let root = fixture();
    let dir = root.join("handlers");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("nodisplay.desktop"),
        "[Desktop Entry]\nType=Application\nName=Viewer\nNoDisplay=true\nExec=viewer %F\n",
    )
    .unwrap();
    // Invalid UTF-8 makes read_to_string fail; discovery must skip it, not abort.
    fs::write(dir.join("broken.desktop"), [0xff_u8, 0xfe, 0x00]).unwrap();
    let handlers =
        ApplicationCatalog::from_dirs_with_visibility(std::slice::from_ref(&dir), true).unwrap();
    assert!(
        handlers
            .entries()
            .iter()
            .any(|e| e.application.name == "Viewer")
    );
    let start_menu = ApplicationCatalog::from_dirs(std::slice::from_ref(&dir)).unwrap();
    assert!(
        start_menu
            .entries()
            .iter()
            .all(|e| e.application.name != "Viewer")
    );
}

#[test]
fn catalog_precedence_search_and_desktop_id() {
    let root = fixture();
    let catalog = ApplicationCatalog::from_dirs(&[root.join("applications")]).unwrap();
    assert_eq!(catalog.entries().len(), 1);
    assert_eq!(catalog.entries()[0].id.as_str(), "nested-example.desktop");
    assert_eq!(catalog.search("fixture")[0].name, "Example Tool");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn strict_exec_argv_keeps_nested_display_literal() {
    let app = DesktopApplication {
        id: DesktopAppId::new("example.desktop"),
        name: "Example".into(),
        generic_name: String::new(),
        comment: String::new(),
        startup_wm_class: String::new(),
        argv: Vec::new(),
        keywords: Vec::new(),
        icon_name: "icon".into(),
        categories: Vec::new(),
    };
    let options = ApplicationLaunchOptions {
        extra_args: vec!["--extra".into()],
        uris: vec!["file:///tmp/a b".into()],
    };
    assert_eq!(
        expand_exec("program %U --literal=DISPLAY=:9", &app, &options).unwrap(),
        vec![
            "program",
            "file:///tmp/a b",
            "--literal=DISPLAY=:9",
            "--extra"
        ]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn spawned_application_is_reaped_after_exit() {
    let pid = spawn_reaped_with_id(&mut Command::new("true"), None).unwrap();
    let proc_entry = PathBuf::from(format!("/proc/{pid}"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    // A zombie keeps /proc/<pid> alive; reaping removes it.
    while proc_entry.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "child {pid} was not reaped"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn app_with_icon(icon: &str) -> DesktopApplication {
    DesktopApplication {
        id: DesktopAppId::new("example.desktop"),
        name: "Example".into(),
        generic_name: String::new(),
        comment: String::new(),
        startup_wm_class: String::new(),
        argv: Vec::new(),
        keywords: Vec::new(),
        icon_name: icon.into(),
        categories: Vec::new(),
    }
}

#[test]
fn expand_exec_ignores_deprecated_field_codes() {
    let options = ApplicationLaunchOptions {
        extra_args: Vec::new(),
        uris: vec!["file:///tmp/a".into()],
    };
    assert_eq!(
        expand_exec("prog %d %U", &app_with_icon(""), &options).unwrap(),
        vec!["prog", "file:///tmp/a"]
    );
    assert_eq!(
        expand_exec(
            "prog %d %D %n %N %v %m",
            &app_with_icon(""),
            &ApplicationLaunchOptions::default()
        )
        .unwrap(),
        vec!["prog"]
    );
}

#[test]
fn expand_exec_standalone_codes_without_uris_and_percent_literal() {
    let options = ApplicationLaunchOptions::default();
    // Empty icon: %i contributes nothing; %F and %u contribute nothing without uris.
    assert_eq!(
        expand_exec("prog %F %u %i %%", &app_with_icon(""), &options).unwrap(),
        vec!["prog", "%"]
    );
    // Non-empty icon: %i pushes the --icon pair before the %% literal.
    assert_eq!(
        expand_exec("prog %F %u %i %%", &app_with_icon("example"), &options).unwrap(),
        vec!["prog", "--icon", "example", "%"]
    );
}

#[test]
fn terminal_true_entries_are_marked_for_terminal_wrapping() {
    let root = std::env::temp_dir().join(format!("flamewm-term-entry-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let term = write_entry(
        &root,
        "htop.desktop",
        "[Desktop Entry]\nType=Application\nName=htop\nExec=htop %U\nTerminal=true\n",
    );
    let gui = write_entry(
        &root,
        "gui.desktop",
        "[Desktop Entry]\nType=Application\nName=gui\nExec=gui\nTerminal=false\n",
    );
    assert!(DesktopEntry::from_file(&term).unwrap().unwrap().terminal);
    assert!(!DesktopEntry::from_file(&gui).unwrap().unwrap().terminal);
    let gui_entry = DesktopEntry::from_file(&gui).unwrap().unwrap();
    assert_eq!(
        gui_entry
            .argv(&ApplicationLaunchOptions::default())
            .unwrap(),
        vec!["gui"]
    );
    fs::remove_dir_all(&root).unwrap();
}

fn write_entry(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    fs::write(&path, body).unwrap();
    path
}

#[test]
fn from_file_exposes_name_icon_exec() {
    let root = fixture();
    let path = write_entry(
        &root,
        "entry.desktop",
        "[Desktop Entry]\nType=Application\nName=Entry Name\nIcon=entry-icon\nExec=myapp --flag\n",
    );
    let entry = DesktopEntry::from_file(&path).unwrap().unwrap();
    assert_eq!(entry.name(), "Entry Name");
    assert_eq!(entry.icon(), "entry-icon");
    assert_eq!(entry.exec(), "myapp --flag");
    assert_eq!(entry.application().name, "Entry Name");
    assert_eq!(entry.path(), path.as_path());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn from_file_absolute_icon_path() {
    let root = fixture();
    let path = write_entry(
        &root,
        "absolute.desktop",
        "[Desktop Entry]\nType=Application\nName=Absolute\nIcon=/usr/share/icons/absolute.png\nExec=myapp\n",
    );
    let entry = DesktopEntry::from_file(&path).unwrap().unwrap();
    assert_eq!(entry.icon(), "/usr/share/icons/absolute.png");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn from_file_icon_name() {
    let root = fixture();
    let path = write_entry(
        &root,
        "named.desktop",
        "[Desktop Entry]\nType=Application\nName=Named\nIcon=text-editor\nExec=myapp\n",
    );
    let entry = DesktopEntry::from_file(&path).unwrap().unwrap();
    assert_eq!(entry.icon(), "text-editor");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn from_file_expands_field_codes() {
    let root = fixture();
    let path = write_entry(
        &root,
        "codes.desktop",
        "[Desktop Entry]\nType=Application\nName=Field Codes\nIcon=icon\nExec=myapp %c %k %f\n",
    );
    let entry = DesktopEntry::from_file(&path).unwrap().unwrap();
    let options = ApplicationLaunchOptions {
        extra_args: Vec::new(),
        uris: vec!["file:///tmp/input".into()],
    };
    let argv = entry.argv(&options).unwrap();
    let desktop = path.to_string_lossy().into_owned();
    assert_eq!(
        argv,
        vec![
            "myapp".to_owned(),
            "Field Codes".to_owned(),
            desktop,
            "file:///tmp/input".to_owned(),
        ]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn from_file_missing_exec_is_none() {
    let root = fixture();
    let path = write_entry(
        &root,
        "noexec.desktop",
        "[Desktop Entry]\nType=Application\nName=No Exec\nIcon=icon\n",
    );
    assert!(DesktopEntry::from_file(&path).unwrap().is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn from_file_hidden_is_none() {
    let root = fixture();
    let path = write_entry(
        &root,
        "hidden-entry.desktop",
        "[Desktop Entry]\nType=Application\nName=Hidden Entry\nHidden=true\nExec=myapp\n",
    );
    assert!(DesktopEntry::from_file(&path).unwrap().is_none());
    fs::remove_dir_all(root).unwrap();
}

fn identity_catalog() -> ApplicationCatalog {
    fn entry(id: &str, name: &str, wm_class: &str, exec: &str) -> DesktopEntry {
        DesktopEntry {
            id: DesktopAppId::new(id),
            application: DesktopApplication {
                id: DesktopAppId::new(id),
                name: name.into(),
                generic_name: String::new(),
                comment: String::new(),
                startup_wm_class: wm_class.into(),
                argv: Vec::new(),
                keywords: Vec::new(),
                icon_name: String::new(),
                categories: Vec::new(),
            },
            path: PathBuf::from(format!("/tmp/{id}")),
            exec: exec.into(),
            startup_notify: None,
            terminal: false,
        }
    }
    let mut catalog = ApplicationCatalog {
        applications: vec![
            entry(
                "org.kde.dolphin.desktop",
                "Dolphin",
                "dolphin",
                "dolphin %U",
            ),
            entry(
                "firefox.desktop",
                "Firefox",
                "firefox",
                "/usr/lib/firefox/firefox %u",
            ),
            entry(
                "org.prismlauncher.PrismLauncher.desktop",
                "Prism Launcher",
                "PrismLauncher",
                "prismlauncher",
            ),
            entry("myeditor.desktop", "MyEditor", "", "myeditor"),
            entry("myeditor-pro.desktop", "MyEditor Pro", "", "myeditor-pro"),
        ],
        by_id: std::collections::BTreeMap::new(),
        by_wm_class: std::collections::BTreeMap::new(),
        by_stem: std::collections::BTreeMap::new(),
        by_exec: std::collections::BTreeMap::new(),
    };
    catalog.rebuild_indexes();
    catalog
}

#[test]
fn dolphin_launch_sets_the_kde_platform_theme_and_other_apps_stay_untouched() {
    let mut dolphin = Command::new("dolphin");
    configure_kde_theme(&mut dolphin, "dolphin", None);
    let envs: std::collections::BTreeMap<_, _> = dolphin
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert_eq!(
        envs.get("XDG_CURRENT_DESKTOP").and_then(Option::as_deref),
        Some("KDE")
    );
    assert_eq!(
        envs.get("KDE_SESSION_VERSION").and_then(Option::as_deref),
        Some("6")
    );
    assert_eq!(
        envs.get("QT_QPA_PLATFORMTHEME").and_then(Option::as_deref),
        Some("KDE")
    );
    assert!(envs.contains_key("KDE_FULL_SESSION"));

    let mut konsole = Command::new("konsole");
    configure_kde_theme(
        &mut konsole,
        "/usr/bin/konsole",
        Some(&DesktopAppId::new("konsole.desktop")),
    );
    assert_eq!(konsole.get_envs().count(), 0);
}

#[test]
fn identity_matches_dolphin_by_startup_wm_class() {
    let catalog = identity_catalog();
    let found = catalog
        .find_by_window_identity(&WindowApplicationIdentity::new(
            "dolphin", "Dolphin", "dolphin",
        ))
        .unwrap();
    assert_eq!(found.id.as_str(), "org.kde.dolphin.desktop");
    assert!(is_dolphin_application("/usr/bin/dolphin", None));
    assert!(is_dolphin_application(
        "/usr/libexec/other",
        Some(&found.id)
    ));
    assert!(!is_dolphin_application(
        "/usr/bin/konsole",
        Some(&DesktopAppId::new("konsole.desktop"))
    ));
    assert_eq!(found.application.id, found.id);
    assert_eq!(
        catalog
            .all()
            .into_iter()
            .find(|application| application.name == "Dolphin")
            .unwrap()
            .id,
        found.id
    );
}

#[test]
fn identity_matches_firefox_by_exec_basename() {
    let catalog = identity_catalog();
    let found = catalog
        .find_by_window_identity(&WindowApplicationIdentity::new("", "", "firefox"))
        .unwrap();
    assert_eq!(found.id.as_str(), "firefox.desktop");
}

#[test]
fn identity_matches_prism_launcher_startup_wm_class() {
    let catalog = identity_catalog();
    let found = catalog
        .find_by_window_identity(&WindowApplicationIdentity::new(
            "PrismLauncher",
            "prismlauncher",
            "prismlauncher",
        ))
        .unwrap();
    assert_eq!(found.id.as_str(), "org.prismlauncher.PrismLauncher.desktop");
}

#[test]
fn identity_matches_class_and_desktop_file_stem() {
    let catalog = identity_catalog();
    let by_class = catalog
        .find_by_window_identity(&WindowApplicationIdentity::new("", "DOLPHIN", ""))
        .unwrap();
    let by_stem = catalog
        .find_by_window_identity(&WindowApplicationIdentity::new("", "", "org.kde.dolphin"))
        .unwrap();
    assert_eq!(by_class.id.as_str(), "org.kde.dolphin.desktop");
    assert_eq!(by_stem.id.as_str(), "org.kde.dolphin.desktop");
    assert_eq!(by_stem.application.id, by_stem.id);
}

#[test]
fn identity_matches_wm_class_instance() {
    let catalog = identity_catalog();
    let found = catalog
        .find_by_window_identity(&WindowApplicationIdentity::new("PrismLauncher", "", ""))
        .unwrap();
    assert_eq!(found.id.as_str(), "org.prismlauncher.PrismLauncher.desktop");
    assert_eq!(found.application.id, found.id);
}

#[test]
fn identity_prefers_startup_wm_class_over_id_stem() {
    let catalog = identity_catalog();
    // wm_class "dolphin" must not fuzzy-match another nearby id stem.
    let found = catalog
        .find_by_window_identity(&WindowApplicationIdentity::new("", "DOLPHIN", ""))
        .unwrap();
    assert_eq!(found.id.as_str(), "org.kde.dolphin.desktop");
}

#[test]
fn identity_near_collision_names_stay_exact() {
    let catalog = identity_catalog();
    let plain = catalog
        .find_by_window_identity(&WindowApplicationIdentity::new("", "", "myeditor"))
        .unwrap();
    let pro = catalog
        .find_by_window_identity(&WindowApplicationIdentity::new("", "", "myeditor-pro"))
        .unwrap();
    assert_eq!(plain.id.as_str(), "myeditor.desktop");
    assert_eq!(pro.id.as_str(), "myeditor-pro.desktop");
    // Substring must not match: "myedit" is not an exact stem/basename.
    assert!(
        catalog
            .find_by_window_identity(&WindowApplicationIdentity::new("", "", "myedit"))
            .is_none()
    );
    assert!(
        catalog
            .find_by_window_identity(&WindowApplicationIdentity::new("", "", ""))
            .is_none()
    );
}
