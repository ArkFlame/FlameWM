//! Secure theme importer: valid fixtures and the hostile matrix. Scratch dirs live
//! under `std::env::temp_dir()` and are never inside the repository.

use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use flamewm_integrations_linux::themes::ThemeKind;
use flamewm_integrations_linux::themes::import::{ImportError, InstallOptions, install_theme};

const INDEX: &str = "[Icon Theme]\nName=Demo\nDirectories=16x16/apps\n[16x16/apps]\nSize=16\nContext=Apps\nType=Threshold\n";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "flamewm-theme-import-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// One tar member: name bytes, typeflag, body, linkname.
struct Member<'a> {
    name: &'a [u8],
    flag: u8,
    body: &'a [u8],
    link: &'a [u8],
}

fn m<'a>(name: &'a str, flag: u8, body: &'a [u8]) -> Member<'a> {
    Member {
        name: name.as_bytes(),
        flag,
        body,
        link: b"",
    }
}

fn link<'a>(name: &'a str, flag: u8, target: &'a str) -> Member<'a> {
    Member {
        name: name.as_bytes(),
        flag,
        body: b"",
        link: target.as_bytes(),
    }
}

fn header(member: &Member<'_>, size: u64) -> [u8; 512] {
    let mut h = [0u8; 512];
    h[..member.name.len().min(100)].copy_from_slice(&member.name[..member.name.len().min(100)]);
    h[100..108].copy_from_slice(b"0000755\0");
    h[108..116].copy_from_slice(b"0000000\0");
    h[116..124].copy_from_slice(b"0000000\0");
    h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
    h[136..148].copy_from_slice(b"00000000000\0");
    h[148..156].copy_from_slice(b"        ");
    h[156] = member.flag;
    h[157..157 + member.link.len()].copy_from_slice(member.link);
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    h
}

fn entry(member: &Member<'_>) -> Vec<u8> {
    let mut out = header(member, member.body.len() as u64).to_vec();
    out.extend_from_slice(member.body);
    out.extend(std::iter::repeat_n(
        0u8,
        (512 - member.body.len() % 512) % 512,
    ));
    out
}

fn tar(members: &[Member<'_>]) -> Vec<u8> {
    let mut out: Vec<u8> = members.iter().flat_map(entry).collect();
    out.extend(std::iter::repeat_n(0u8, 1024));
    out
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(bytes).unwrap();
    enc.finish().unwrap()
}

fn write_tgz(dir: &Path, name: &str, members: &[Member<'_>]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, gzip(&tar(members))).unwrap();
    path
}

fn demo_members<'a>(xcursor: &'a [u8]) -> Vec<Member<'a>> {
    vec![
        m("Demo/", b'5', b""),
        m("Demo/index.theme", b'0', INDEX.as_bytes()),
        m("Demo/16x16/", b'5', b""),
        m("Demo/16x16/apps/", b'5', b""),
        m("Demo/16x16/apps/app.png", b'0', b"png-bytes"),
        m("Demo/cursors/", b'5', b""),
        m("Demo/cursors/left_ptr", b'0', xcursor),
        link("Demo/cursors/default", b'2', "left_ptr"),
    ]
}

fn demo_with(base: &Path, name: &str, extra: Vec<Member<'_>>) -> PathBuf {
    let mut members = demo_members(b"c");
    members.extend(extra);
    write_tgz(base, name, &members)
}

fn run(
    source: &Path,
    icons: &Path,
    options: &InstallOptions,
) -> Result<flamewm_integrations_linux::themes::import::InstalledTheme, ImportError> {
    install_theme(source, icons, options, &AtomicBool::new(false))
}

fn assert_no_staging(icons: &Path) {
    let leftovers: Vec<_> = fs::read_dir(icons)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n.to_string_lossy().starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "staging left behind: {leftovers:?}");
}

#[test]
fn tgz_installs_theme_with_internal_symlink_and_no_staging_left() {
    let base = scratch("tgz-valid");
    let icons = base.join("icons");
    let src = write_tgz(&base, "demo.tar.gz", &demo_members(b"cursor"));
    let installed = run(&src, &icons, &InstallOptions::default()).expect("install");
    assert_eq!(installed.id, "Demo");
    assert_eq!(installed.kind, ThemeKind::Icon);
    assert_eq!(installed.root, icons.join("Demo"));
    assert_eq!(
        fs::read(icons.join("Demo/16x16/apps/app.png")).unwrap(),
        b"png-bytes"
    );
    let link = fs::read_link(icons.join("Demo/cursors/default")).unwrap();
    assert_eq!(link, Path::new("left_ptr"));
    assert_no_staging(&icons);
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn folder_source_installs_and_keeps_internal_links() {
    let base = scratch("folder-valid");
    let src = base.join("src/Demo");
    fs::create_dir_all(src.join("cursors")).unwrap();
    fs::write(
        src.join("index.theme"),
        "[Icon Theme]\nName=Demo\nDirectories=d\n[d]\nSize=8\n",
    )
    .unwrap();
    fs::write(src.join("cursors/left_ptr"), b"c").unwrap();
    symlink("left_ptr", src.join("cursors/default")).unwrap();
    let icons = base.join("icons");
    let installed = run(&src, &icons, &InstallOptions::default()).expect("install folder");
    assert_eq!(installed.id, "Demo");
    assert!(icons.join("Demo/cursors/default").is_symlink());
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn hardlink_inside_theme_is_accepted_and_copies_content() {
    let base = scratch("hardlink-ok");
    let icons = base.join("icons");
    let src = demo_with(
        &base,
        "h.tar.gz",
        vec![link(
            "Demo/16x16/apps/alias.png",
            b'1',
            "Demo/16x16/apps/app.png",
        )],
    );
    run(&src, &icons, &InstallOptions::default()).expect("install");
    assert_eq!(
        fs::read(icons.join("Demo/16x16/apps/alias.png")).unwrap(),
        b"png-bytes"
    );
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn xz_archive_with_bad_stream_header_is_rejected_without_writing() {
    let base = scratch("xz");
    let src = base.join("demo.tar.xz");
    fs::write(&src, b"\xFD7zXZ\0not really xz").unwrap();
    let err = run(&src, &base.join("icons"), &InstallOptions::default()).unwrap_err();
    assert!(matches!(err, ImportError::Corrupt(_)), "{err:?}");
    assert!(
        !base.join("icons").exists(),
        "nothing may be created for xz"
    );
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn xz_tar_installs_through_the_shared_tar_path() {
    let base = scratch("xz-valid");
    let icons = base.join("icons");
    let tar_bytes = tar(&demo_members(b"cursor"));
    let src = base.join("demo.tar.xz");
    let mut child = match std::process::Command::new("xz")
        .args(["-c", "-q"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            eprintln!("SKIP: xz CLI unavailable");
            return;
        }
    };
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&tar_bytes));
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap().unwrap();
    assert!(out.status.success());
    fs::write(&src, out.stdout).unwrap();
    let installed = run(&src, &icons, &InstallOptions::default()).expect("xz install");
    assert_eq!(installed.id, "Demo");
    assert_eq!(
        fs::read(icons.join("Demo/16x16/apps/app.png")).unwrap(),
        b"png-bytes"
    );
    assert_eq!(
        fs::read_link(icons.join("Demo/cursors/default")).unwrap(),
        Path::new("left_ptr")
    );
    assert_no_staging(&icons);
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn ambiguous_archive_requires_id_and_then_selects_that_theme() {
    let base = scratch("ambiguous");
    let icons = base.join("icons");
    let a = INDEX.as_bytes();
    let members = [m("A/index.theme", b'0', a), m("B/index.theme", b'0', a)];
    let src = write_tgz(&base, "two.tar.gz", &members);
    let err = run(&src, &icons, &InstallOptions::default()).unwrap_err();
    assert!(
        matches!(err, ImportError::AmbiguousThemes(ref v) if v.len() == 2),
        "{err:?}"
    );
    assert!(!icons.exists());

    let picked = run(
        &src,
        &icons,
        &InstallOptions {
            id: Some("B".into()),
            overwrite: false,
        },
    )
    .expect("select B");
    assert_eq!(picked.id, "B");
    assert!(icons.join("B/index.theme").is_file());
    assert!(
        !icons.join("A").exists(),
        "only the chosen theme is installed"
    );
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn id_collision_refuses_unless_overwrite_and_overwrite_replaces_content() {
    let base = scratch("collision");
    let icons = base.join("icons");
    let first = write_tgz(&base, "one.tar.gz", &demo_members(b"c"));
    run(&first, &icons, &InstallOptions::default()).expect("first install");

    let second_members = [
        m("Demo/index.theme", b'0', INDEX.as_bytes()),
        m("Demo/marker", b'0', b"v2"),
    ];
    let second = write_tgz(&base, "two.tar.gz", &second_members);
    let err = run(&second, &icons, &InstallOptions::default()).unwrap_err();
    assert!(
        matches!(err, ImportError::IdCollision(ref id) if id == "Demo"),
        "{err:?}"
    );
    assert!(
        icons.join("Demo/16x16/apps/app.png").is_file(),
        "existing theme untouched"
    );

    run(
        &second,
        &icons,
        &InstallOptions {
            id: None,
            overwrite: true,
        },
    )
    .expect("overwrite");
    assert_eq!(fs::read(icons.join("Demo/marker")).unwrap(), b"v2");
    assert!(
        !icons.join("Demo/16x16").exists(),
        "old tree fully replaced"
    );
    assert_no_staging(&icons);
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn missing_index_and_invalid_ids_are_rejected_before_writing() {
    let base = scratch("missing");
    let icons = base.join("icons");
    let src = write_tgz(&base, "none.tar.gz", &[m("Demo/readme", b'0', b"x")]);
    let err = run(&src, &icons, &InstallOptions::default()).unwrap_err();
    assert!(matches!(err, ImportError::MissingIndex), "{err:?}");

    let good = write_tgz(&base, "good.tar.gz", &demo_members(b"c"));
    for bad in ["../escape", ".hidden", "a/b", ""] {
        let err = run(
            &good,
            &icons,
            &InstallOptions {
                id: Some(bad.into()),
                overwrite: false,
            },
        )
        .unwrap_err();
        assert!(matches!(err, ImportError::InvalidId(_)), "{bad:?}: {err:?}");
    }
    assert!(!icons.exists());
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn hostile_paths_are_rejected() {
    let base = scratch("paths");
    let icons = base.join("icons");
    let cases: Vec<(&str, Member<'_>)> = vec![
        ("parent segment", m("Demo/../evil", b'0', b"x")),
        ("absolute", m("/etc/evil", b'0', b"x")),
        ("leading parent", m("../evil", b'0', b"x")),
    ];
    for (label, extra) in cases {
        let src = demo_with(&base, "p.tar.gz", vec![extra]);
        let err = run(&src, &icons, &InstallOptions::default()).unwrap_err();
        assert!(
            matches!(err, ImportError::UnsafePath(_)),
            "{label}: {err:?}"
        );
    }
    let raw = tar_raw_name(b"Demo/\xff\xfe", b"x");
    fs::write(
        base.join("u.tar.gz"),
        gzip(&[raw, tar(&demo_members(b"c"))].concat()),
    )
    .unwrap();
    let err = run(&base.join("u.tar.gz"), &icons, &InstallOptions::default()).unwrap_err();
    assert!(matches!(err, ImportError::InvalidUtf8), "{err:?}");
    assert!(!icons.exists());
    fs::remove_dir_all(&base).unwrap();
}

fn tar_raw_name(name: &[u8], body: &[u8]) -> Vec<u8> {
    entry(&Member {
        name,
        flag: b'0',
        body,
        link: b"",
    })
}

#[test]
fn escaping_and_absolute_links_are_rejected() {
    let base = scratch("links");
    let icons = base.join("icons");
    let cases: Vec<(&str, Member<'_>)> = vec![
        ("parent escape", link("Demo/up", b'2', "../../etc/passwd")),
        ("absolute target", link("Demo/abs", b'2', "/etc/passwd")),
        ("chained escape", link("Demo/hop", b'2', "left_ptr_alias")),
    ];
    for (label, extra) in cases {
        let mut members = demo_members(b"c");
        if label == "chained escape" {
            members.push(link("Demo/left_ptr_alias", b'2', "../../outside"));
        }
        members.push(extra);
        let src = write_tgz(&base, "l.tar.gz", &members);
        let err = run(&src, &icons, &InstallOptions::default()).unwrap_err();
        assert!(
            matches!(err, ImportError::EscapingLink(_)),
            "{label}: {err:?}"
        );
    }
    assert!(!icons.exists());
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn hardlink_to_outside_file_is_rejected() {
    let base = scratch("hardlink-escape");
    let icons = base.join("icons");
    let src = demo_with(
        &base,
        "hl.tar.gz",
        vec![
            m("Other/secret", b'0', b"secret"),
            link("Demo/steal", b'1', "Other/secret"),
        ],
    );
    let err = run(&src, &icons, &InstallOptions::default()).unwrap_err();
    assert!(matches!(err, ImportError::EscapingLink(_)), "{err:?}");
    assert!(!icons.exists());
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn device_and_fifo_entries_are_rejected() {
    let base = scratch("devices");
    let icons = base.join("icons");
    for flag in [b'3', b'4', b'6'] {
        let src = demo_with(&base, "d.tar.gz", vec![m("Demo/node", flag, b"")]);
        let err = run(&src, &icons, &InstallOptions::default()).unwrap_err();
        assert!(
            matches!(err, ImportError::UnsupportedEntry(_)),
            "flag {flag}: {err:?}"
        );
    }
    assert!(!icons.exists());
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn entry_count_and_depth_caps_are_enforced() {
    let base = scratch("caps");
    let icons = base.join("icons");

    let mut many = demo_members(b"c");
    let names: Vec<String> = (0..10_001).map(|i| format!("Demo/f{i}")).collect();
    for name in &names {
        many.push(m(name, b'0', b""));
    }
    let src = write_tgz(&base, "many.tar.gz", &many);
    let err = run(&src, &icons, &InstallOptions::default()).unwrap_err();
    assert!(matches!(err, ImportError::TooManyEntries), "{err:?}");

    let deep = format!("Demo/{}f", "d/".repeat(40));
    let src = demo_with(&base, "deep.tar.gz", vec![m(&deep, b'0', b"x")]);
    let err = run(&src, &icons, &InstallOptions::default()).unwrap_err();
    assert!(matches!(err, ImportError::TooDeep), "{err:?}");
    assert!(!icons.exists());
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn decompressed_size_cap_is_enforced_across_entries() {
    let base = scratch("bomb");
    let icons = base.join("icons");
    let chunk = vec![0u8; 130 * 1024 * 1024];
    let mut members = demo_members(b"c");
    members.push(m("Demo/big1", b'0', &chunk));
    members.push(m("Demo/big2", b'0', &chunk));
    let src = write_tgz(&base, "bomb.tar.gz", &members);
    let err = run(&src, &icons, &InstallOptions::default()).unwrap_err();
    assert!(matches!(err, ImportError::TooLarge), "{err:?}");
    assert!(!icons.exists());
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn cancellation_leaves_zero_entries() {
    let base = scratch("cancel");
    let icons = base.join("icons");
    let src = write_tgz(&base, "c.tar.gz", &demo_members(b"c"));
    let cancel = AtomicBool::new(true);
    let err = install_theme(&src, &icons, &InstallOptions::default(), &cancel).unwrap_err();
    assert!(matches!(err, ImportError::Cancelled), "{err:?}");
    let count = fs::read_dir(&icons).map_or(0, |d| d.count());
    assert_eq!(count, 0, "cancelled install must leave no entries");
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn file_below_symlinked_directory_is_rejected() {
    let base = scratch("ancestor");
    let icons = base.join("icons");
    let src = demo_with(
        &base,
        "a.tar.gz",
        vec![
            link("Demo/cursors/extra", b'2', "left_ptr"),
            m("Demo/cursors/extra/x", b'0', b"x"),
        ],
    );
    let err = run(&src, &icons, &InstallOptions::default()).unwrap_err();
    assert!(matches!(err, ImportError::Corrupt(_)), "{err:?}");
    assert!(!icons.exists());
    fs::remove_dir_all(&base).unwrap();
}
