//! Theme catalog discovery, inheritance cycles, and Xcursor preview decoding.

use std::fs;
use std::path::{Path, PathBuf};

use flamewm_integrations_linux::themes::cursor::{CursorError, parse_xcursor_preview};
use flamewm_integrations_linux::themes::{ThemeCatalog, ThemeKind};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "flamewm-theme-catalog-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn write_index(dir: &Path, body: &str) {
    fs::create_dir_all(dir).expect("theme dir");
    fs::write(dir.join("index.theme"), body).expect("index.theme");
}

fn xcursor(frames: &[(u32, u32, u32, u32, u32, u32)]) -> Vec<u8> {
    // frames: (nominal, width, height, xhot, yhot, argb pixel used for every pixel)
    let mut out = Vec::new();
    let push = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    out.extend_from_slice(b"Xcur");
    push(&mut out, 16);
    push(&mut out, 0x0001_0000);
    push(&mut out, frames.len() as u32);
    let mut pos = 16 + 12 * frames.len() as u32;
    let mut chunks = Vec::new();
    for &(nominal, w, h, xhot, yhot, argb) in frames {
        push(&mut out, 0xfffd_0002);
        push(&mut out, nominal);
        push(&mut out, pos);
        let mut chunk = Vec::new();
        push(&mut chunk, 36);
        push(&mut chunk, 0xfffd_0002);
        push(&mut chunk, nominal);
        push(&mut chunk, 1);
        push(&mut chunk, w);
        push(&mut chunk, h);
        push(&mut chunk, xhot);
        push(&mut chunk, yhot);
        push(&mut chunk, 0);
        for _ in 0..(w * h) {
            push(&mut chunk, argb);
        }
        pos += chunk.len() as u32;
        chunks.push(chunk);
    }
    for chunk in chunks {
        out.extend_from_slice(&chunk);
    }
    out
}

#[test]
fn discover_finds_icon_and_cursor_themes_and_prefers_user_root() {
    let base = scratch("discover");
    let xdg = base.join("xdg");
    let prefix = base.join("prefix");
    write_index(
        &xdg.join("icons/Alpha"),
        "[Icon Theme]\nName=Alpha User\nDirectories=16x16/apps\n[16x16/apps]\nSize=16\n",
    );
    fs::create_dir_all(xdg.join("icons/Alpha/16x16/apps")).unwrap();
    write_index(
        &xdg.join("icons/Beta"),
        "[Icon Theme]\nName=Beta\n",
    );
    fs::create_dir_all(xdg.join("icons/Beta/cursors")).unwrap();
    fs::write(xdg.join("icons/Beta/cursors/left_ptr"), b"x").unwrap();
    fs::create_dir_all(xdg.join("icons/NoIndex")).unwrap();
    write_index(
        &prefix.join("icons/Alpha"),
        "[Icon Theme]\nName=Alpha Bundled\nDirectories=16x16/apps\n",
    );

    let catalog = ThemeCatalog::discover(&[xdg.clone(), prefix.clone()]);
    let ours: Vec<_> = catalog
        .entries()
        .iter()
        .filter(|e| e.root.starts_with(&base))
        .collect();

    let alpha = ours
        .iter()
        .find(|e| e.id == "Alpha" && e.kind == ThemeKind::Icon)
        .expect("alpha icon");
    assert_eq!(alpha.name, "Alpha User", "user root must win over bundled");
    assert_eq!(alpha.root, xdg.join("icons/Alpha"));
    assert!(alpha.valid);

    let beta_cursor = ours
        .iter()
        .find(|e| e.id == "Beta" && e.kind == ThemeKind::Cursor)
        .expect("beta cursor");
    assert!(beta_cursor.valid);
    assert!(
        !ours.iter().any(|e| e.id == "Beta" && e.kind == ThemeKind::Icon),
        "Beta has no Directories= so it is not an icon theme"
    );
    assert!(!ours.iter().any(|e| e.id == "NoIndex"), "no index.theme, no entry");
    assert_eq!(
        ours.iter().filter(|e| e.id == "Alpha").count(),
        1,
        "bundled duplicate must not be listed"
    );
    assert!(catalog.find(ThemeKind::Icon, "Alpha").is_some());
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn icon_chain_terminates_on_inherits_cycle() {
    let base = scratch("cycle");
    let xdg = base.join("xdg");
    write_index(
        &xdg.join("icons/A"),
        "[Icon Theme]\nName=A\nDirectories=d\nInherits=B\n[d]\nSize=16\n",
    );
    write_index(
        &xdg.join("icons/B"),
        "[Icon Theme]\nName=B\nDirectories=d\nInherits=A,C\n[d]\nSize=16\n",
    );
    let catalog = ThemeCatalog::discover(std::slice::from_ref(&xdg));
    let chain = catalog.icon_chain("A");
    assert_eq!(chain.first().map(String::as_str), Some("A"));
    assert_eq!(chain.iter().filter(|n| *n == "A").count(), 1);
    assert_eq!(chain.iter().filter(|n| *n == "B").count(), 1);
    assert_eq!(chain.last().map(String::as_str), Some("hicolor"));
    fs::remove_dir_all(&base).unwrap();
}

#[test]
fn xcursor_picks_nearest_nominal_size_and_converts_argb_to_rgba() {
    // Two frames: 16 and 30. 30 is nearer to 24. Pixel word 0xAA112233 is ARGB.
    let bytes = xcursor(&[(16, 1, 1, 0, 0, 0xff00_0000), (30, 3, 2, 1, 1, 0xAA11_2233)]);
    let preview = parse_xcursor_preview(&bytes).expect("valid cursor");
    assert_eq!(preview.nominal_size, 30);
    assert_eq!((preview.width, preview.height), (3, 2));
    assert_eq!(preview.hotspot, (1, 1));
    assert_eq!(preview.rgba.len(), 3 * 2 * 4);
    assert_eq!(&preview.rgba[0..4], &[0x11, 0x22, 0x33, 0xAA]);
}

#[test]
fn xcursor_rejects_malformed_and_truncated_input_without_panicking() {
    let good = xcursor(&[(24, 2, 2, 0, 0, 0xffff_ffff)]);
    assert!(parse_xcursor_preview(&good).is_ok());

    let mut bad_magic = good.clone();
    bad_magic[0] = b'Y';
    assert!(matches!(
        parse_xcursor_preview(&bad_magic),
        Err(CursorError::BadMagic)
    ));

    for cut in 0..good.len() {
        assert!(
            parse_xcursor_preview(&good[..cut]).is_err(),
            "prefix of length {cut} must not decode"
        );
    }

    let zero_width = xcursor(&[(24, 0, 2, 0, 0, 0)]);
    assert!(parse_xcursor_preview(&zero_width).is_err());

    let hotspot_outside = xcursor(&[(24, 2, 2, 2, 0, 0)]);
    assert!(parse_xcursor_preview(&hotspot_outside).is_err());

    let mut huge_toc = good.clone();
    huge_toc[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(parse_xcursor_preview(&huge_toc).is_err());

    let mut position_past_end = good.clone();
    position_past_end[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(parse_xcursor_preview(&position_past_end).is_err());
}
