use super::*;

fn fixture_root(tag: &str, line: u32) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("flamewm-{tag}-{}-{line}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    root
}

#[test]
fn unique_paths_drops_empty_and_repeats_keeping_first_order() {
    let input = vec![
        PathBuf::from("/b"),
        PathBuf::new(),
        PathBuf::from("/a"),
        PathBuf::from("/b"),
    ];
    assert_eq!(
        unique_paths(input),
        vec![PathBuf::from("/b"), PathBuf::from("/a")]
    );
}

#[test]
fn inherits_only_parse_matches_full_parse() {
    let texts = [
        "[Icon Theme]\nName=Test\nInherits=Breeze,hicolor\n\n[16x16/apps]\nInherits=Other\n",
        "# c\nInherits=Early\n[ Icon Theme ]\n; c\nInherits = A , B \nInherits=C\n[x]\n[Icon Theme]\nName=T\n",
        "[Icon Theme]\nName=NoInherits\n",
        "",
    ];
    for text in texts {
        let full = IconThemeIndex::parse("T", text);
        let light =
            IconThemeIndex::parse_inherits("T", text.as_bytes()).expect("in-memory read");
        assert_eq!(light.inherits, full.inherits, "{text:?}");
        assert_eq!(light.name, full.name);
    }
}

#[test]
fn parses_directories_and_inherits() {
    let text = "[Icon Theme]\nName=Test\nDirectories=16x16/apps,scalable/apps\nInherits=Breeze,hicolor\n\n[16x16/apps]\nSize=16\nType=Fixed\n\n[scalable/apps]\nSize=48\nType=Scalable\nMinSize=16\nMaxSize=512\n";
    let index = IconThemeIndex::parse("Test", text);
    assert_eq!(index.inherits, vec!["Breeze", "hicolor"]);
    assert_eq!(index.directories.len(), 2);
    assert_eq!(index.dirs["16x16/apps"].dir_type, IconDirType::Fixed);
    assert_eq!(index.dirs["scalable/apps"].min_size, 16);
}

#[test]
fn cycle_terminates() {
    let loader = |name: &str| {
        let inherits = match name {
            "A" => vec!["B".to_owned()],
            "B" => vec!["A".to_owned()],
            _ => Vec::new(),
        };
        Some(IconThemeIndex {
            name: name.to_owned(),
            inherits,
            ..IconThemeIndex::default()
        })
    };
    let chain = inheritance_chain("A", &loader);
    assert_eq!(chain.first().map(String::as_str), Some("A"));
    assert!(chain.contains(&"B".to_owned()));
    assert_eq!(chain.last().map(String::as_str), Some("hicolor"));
    assert!(chain.iter().filter(|n| *n == "A").count() == 1);
}

#[test]
fn configured_theme_parses_kdeglobals_style_text() {
    let text = "[General]\nColorScheme=BreezeDark\n\n[Icons]\nTheme=Papirus-Dark\n\n[KDE]\nLookAndFeelPackage=org.kde.breeze.desktop\n";
    assert_eq!(
        parse_ini_value(text, "Icons", "Theme").as_deref(),
        Some("Papirus-Dark")
    );
    assert_eq!(parse_ini_value(text, "General", "Theme"), None);
    assert_eq!(parse_ini_value("[Icons]\nTheme=\n", "Icons", "Theme"), None);
    let gtk = "[Settings]\ngtk-icon-theme-name = Adwaita\n";
    assert_eq!(
        parse_ini_value(gtk, "Settings", "gtk-icon-theme-name").as_deref(),
        Some("Adwaita")
    );
}

#[test]
fn exists_cache_is_generation_scoped_and_bounded() {
    let mut caches = IconThemeCaches::default();
    let path = Path::new("/icons/a.png");
    assert_eq!(caches.cached_exists(path, 1), None);
    caches.store_exists(path, 1, true);
    assert_eq!(caches.cached_exists(path, 1), Some(true));
    assert_eq!(caches.cached_exists(path, 2), None);
    caches.store_exists(path, 2, false);
    assert_eq!(caches.cached_exists(path, 2), Some(false));
    assert_eq!(caches.len(), 2);
    for i in 0..(EXISTS_CAP + 10) {
        let p = PathBuf::from(format!("/icons/{i}.png"));
        caches.store_exists(&p, 3, true);
    }
    assert!(caches.len() <= EXISTS_CAP);
    assert_eq!(caches.len(), caches.exists.values().map(HashMap::len).sum());
}

#[test]
fn name_memo_is_bounded_by_pair_count() {
    let mut memo = NameMemo::default();
    for i in 0..5u32 {
        memo.insert("apps", i, vec![PathBuf::from("x")], 3);
    }
    assert_eq!(memo.len, 3);
    assert!(memo.get("apps", 4).is_some());
    memo.insert("apps", 4, vec![PathBuf::from("y")], 3);
    assert_eq!(memo.len, 3);
    assert_eq!(memo.get("apps", 4), Some(&vec![PathBuf::from("y")]));
}

#[test]
fn shallow_watch_detects_nested_icon_directory_change() {
    let root = fixture_root("watch", line!());
    let nested = root.join("Theme/16x16/apps");
    fs::create_dir_all(&nested).expect("nested theme dirs");
    let before = current_watch_mtime(std::slice::from_ref(&root), &[], None);
    // Force a distinct mtime on the nested directory so the check does
    // not depend on filesystem timestamp granularity.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    fs::write(nested.join("new-app.png"), b"x").expect("add icon");
    fs::File::open(&nested)
        .and_then(|dir| dir.set_modified(later))
        .expect("set nested mtime");
    let after = current_watch_mtime(std::slice::from_ref(&root), &[], None);
    let _ = fs::remove_dir_all(&root);
    assert_ne!(before, after);
    assert_eq!(after, Some(later));
}

#[test]
fn watch_dirs_covers_depth_three_under_last_theme_past_cap() {
    let root = fixture_root("watch-cap", line!());
    // First theme alone holds more than WATCH_DIR_CAP directories.
    for i in 0..2100 {
        fs::create_dir_all(root.join("A").join(format!("s{i}"))).expect("wide theme dirs");
    }
    // Last theme: a depth-3 category directory (theme/size/category).
    let category = root.join("B").join("16x16").join("apps");
    fs::create_dir_all(&category).expect("last theme category dir");
    let watched = watch_dirs(std::slice::from_ref(&root), &[], None);
    let _ = fs::remove_dir_all(&root);
    assert!(
        watched.contains(&category),
        "depth-3 category under last theme must be watched past the cap"
    );
}

#[test]
fn warm_candidates_hit_memo_without_new_miss() {
    let index = IconLookupIndex::new(
        Vec::new(),
        vec!["hicolor".to_owned()],
        HashMap::new(),
        HashMap::new(),
        7,
    );
    let first = index.candidates("firefox", 48);
    let (_, cold_misses) = index.stats();
    let second = index.candidates("firefox", 48);
    let (hits, misses) = index.stats();
    assert_eq!(first, second);
    assert_eq!(misses, cold_misses, "warm call must not miss again");
    assert_eq!(hits, 1, "warm call must be served from the memo");
    assert_eq!(index.memo_len(), 1);
}

#[test]
fn needs_rebuild_bumps_generation_when_watch_mtime_is_stale() {
    let root = fixture_root("rebuild-gen", line!());
    fs::create_dir_all(&root).expect("watch root");
    let mut index = IconLookupIndex::new(
        vec![root.clone()],
        Vec::new(),
        HashMap::new(),
        HashMap::new(),
        3,
    );
    index.watch_mtime = Some(std::time::UNIX_EPOCH);
    let next = index.needs_rebuild();
    let throttled = index.needs_rebuild();
    let _ = fs::remove_dir_all(&root);
    let next = next.expect("stale watch mtime must produce a rebuilt snapshot");
    assert_eq!(next.generation, 4);
    assert!(
        throttled.is_none(),
        "second check inside 5s must be throttled"
    );
}

fn build_index(data: &Path, names: &[&str]) -> IconLookupIndex {
    let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
    let index = IconLookupIndex::build(&names, &[data.to_path_buf()], 1);
    let _ = index.files();
    index
}

fn theme_fixture(data: &Path, name: &str, index: &str, icon: Option<&str>) {
    let base = data.join("icons").join(name);
    fs::create_dir_all(base.join("16x16/apps")).expect("theme fixture dirs");
    fs::write(base.join("index.theme"), index).expect("index.theme fixture");
    if let Some(icon) = icon {
        fs::write(base.join("16x16/apps").join(icon), b"").expect("icon fixture");
    }
}

const FIXED_16: &str =
    "[Icon Theme]\nDirectories=16x16/apps\n\n[16x16/apps]\nSize=16\nType=Fixed\n";

#[cfg(unix)]
#[test]
fn unindexed_theme_symlink_loop_terminates() {
    let data = fixture_root("loop", line!());
    // No index.theme: forces the build-time walk for this theme.
    let apps = data.join("icons/loopy/16x16/apps");
    fs::create_dir_all(&apps).expect("loop fixture dirs");
    fs::write(apps.join("loop-app.png"), b"").expect("icon fixture");
    std::os::unix::fs::symlink(data.join("icons/loopy"), apps.join("up")).expect("loop link");
    std::os::unix::fs::symlink(&apps, apps.join("self")).expect("self link");
    let index = build_index(&data, &["loopy"]);
    let _ = fs::remove_dir_all(&data);
    let (paths, hit) = index.lookup("loop-app", 16);
    assert!(hit);
    assert!(paths[0].ends_with("loopy/16x16/apps/loop-app.png"));
}

#[cfg(unix)]
#[test]
fn unindexed_theme_valid_symlinked_subdir_is_indexed() {
    let data = fixture_root("symlink", line!());
    // Target lives outside the theme; only the in-theme symlink reaches it.
    let real = data.join("shared/real");
    fs::create_dir_all(&real).expect("symlink fixture dirs");
    fs::create_dir_all(data.join("icons/linked")).expect("theme dir");
    fs::write(real.join("linked-app.png"), b"").expect("icon fixture");
    std::os::unix::fs::symlink(&real, data.join("icons/linked/alias")).expect("dir link");
    let index = build_index(&data, &["linked"]);
    let _ = fs::remove_dir_all(&data);
    let (paths, hit) = index.lookup("linked-app", 16);
    assert!(hit, "valid symlinked subdir must stay indexed");
    assert!(
        paths
            .iter()
            .any(|p| p.ends_with("linked/alias/linked-app.png"))
    );
}

#[test]
fn selected_theme_inherits_before_breeze_then_hicolor() {
    let data = fixture_root("chain", line!());
    let mcmojave = "[Icon Theme]\nName=McMojave\nDirectories=16x16/apps\nInherits=Numix-Circle-Light,hicolor,breeze\n\n[16x16/apps]\nSize=16\nType=Fixed\n";
    theme_fixture(&data, "McMojave-circle", mcmojave, None);
    theme_fixture(&data, "Numix-Circle-Light", FIXED_16, Some("chain-app.png"));
    theme_fixture(&data, "Breeze", FIXED_16, Some("chain-app.png"));
    theme_fixture(&data, "hicolor", FIXED_16, Some("chain-app.png"));
    let index = build_index(&data, &["McMojave-circle", "Breeze"]);
    let _ = fs::remove_dir_all(&data);
    let (paths, hit) = index.lookup("chain-app", 16);
    assert!(hit);
    assert_eq!(
        index.ordered_themes.first().map(String::as_str),
        Some("McMojave-circle")
    );
    // Inherited Numix must outrank Breeze, which the old ordering put first.
    assert!(paths[0].ends_with("Numix-Circle-Light/16x16/apps/chain-app.png"));
}

#[test]
fn theme_dir_lookup_is_case_insensitive_for_breeze() {
    let data = fixture_root("case", line!());
    // Host ships lowercase `breeze`; the chain names `Breeze`.
    theme_fixture(&data, "breeze", FIXED_16, Some("case-app.png"));
    let index = build_index(&data, &["Breeze"]);
    let _ = fs::remove_dir_all(&data);
    let (paths, hit) = index.lookup("case-app", 16);
    assert!(hit, "Breeze must resolve to the installed breeze directory");
    assert!(paths[0].ends_with("breeze/16x16/apps/case-app.png"));
}

#[test]
fn missing_selected_theme_falls_back_to_breeze_not_blank() {
    let data = fixture_root("missing", line!());
    theme_fixture(&data, "Breeze", FIXED_16, Some("fallback-app.png"));
    let index = build_index(&data, &["McMojave-circle", "Breeze"]);
    let (paths, hit) = index.lookup("fallback-app", 16);
    let (missing, missing_hit) = index.lookup("never-installed-icon", 16);
    let _ = fs::remove_dir_all(&data);
    assert!(hit, "Breeze must serve when the selected theme is absent");
    assert!(paths[0].ends_with("Breeze/16x16/apps/fallback-app.png"));
    assert!(
        missing.is_empty() && !missing_hit,
        "intentionally missing icon stays empty"
    );
}

#[test]
fn build_parses_each_chained_index_theme_once() {
    let data = fixture_root("parse-once", line!());
    theme_fixture(&data, "Breeze", FIXED_16, Some("once-app.png"));
    let mine = data.join("icons").join("Mine");
    fs::create_dir_all(&mine).expect("selected theme dir");
    fs::write(
        mine.join("index.theme"),
        "[Icon Theme]\nName=Mine\nInherits=Breeze\nDirectories=16x16/apps\n\n[16x16/apps]\nSize=16\nType=Fixed\n",
    )
    .expect("selected index.theme");
    INDEX_THEME_READS.with(|c| c.set(0));
    let index = IconLookupIndex::build(&["Mine".to_owned()], &[data.clone()], 1);
    let reads = INDEX_THEME_READS.with(std::cell::Cell::get);
    let (paths, hit) = index.lookup("once-app", 16);
    let _ = fs::remove_dir_all(&data);
    assert_eq!(
        reads, 2,
        "Mine and Breeze each parse index.theme once per build"
    );
    assert!(hit && paths[0].ends_with("Breeze/16x16/apps/once-app.png"));
}

#[test]
fn indexed_file_record_stays_compact() {
    assert!(std::mem::size_of::<IndexedIconFile>() <= 8);
}

#[test]
fn compact_index_selects_same_paths_as_before() {
    let data = fixture_root("compact", line!());
    let base = data.join("icons/FlameFixture");
    fs::create_dir_all(base.join("16x16/apps")).expect("16 dir");
    fs::create_dir_all(base.join("32x32/apps")).expect("32 dir");
    fs::write(
        base.join("index.theme"),
        "[Icon Theme]\nName=FlameFixture\nDirectories=16x16/apps,32x32/apps\n\n[16x16/apps]\nSize=16\nType=Fixed\n\n[32x32/apps]\nSize=32\nType=Fixed\n",
    )
    .expect("index.theme fixture");
    fs::write(base.join("16x16/apps/flame-fixture-app.png"), b"").expect("16 png");
    fs::write(base.join("32x32/apps/flame-fixture-app.svg"), b"").expect("32 svg");
    fs::write(base.join("32x32/apps/flame-fixture-app.png"), b"").expect("32 png");
    let index = build_index(&data, &["FlameFixture"]);
    let at_32 = index.lookup("flame-fixture-app", 32).0;
    let at_16 = index.lookup("flame-fixture-app", 16).0;
    let svg = index.lookup("flame-fixture-app.svg", 16).0;
    let _ = fs::remove_dir_all(&data);
    assert_eq!(at_32, vec![base.join("32x32/apps/flame-fixture-app.png")]);
    assert_eq!(at_16, vec![base.join("16x16/apps/flame-fixture-app.png")]);
    assert_eq!(svg, vec![base.join("32x32/apps/flame-fixture-app.svg")]);
}

#[test]
fn duplicate_search_roots_index_each_file_once() {
    let first = fixture_root("dup-a", line!());
    let second = fixture_root("dup-b", line!());
    for data in [&first, &second] {
        let base = data.join("icons/FlameDup");
        fs::create_dir_all(base.join("16x16/apps")).expect("dup theme dirs");
        fs::write(
            base.join("index.theme"),
            "[Icon Theme]\nName=FlameDup\nDirectories=16x16/apps\n\n[16x16/apps]\nSize=16\nType=Fixed\n",
        )
        .expect("dup index.theme");
        fs::write(base.join("16x16/apps/flame-dup-app.png"), b"").expect("dup icon");
    }
    let names = vec!["FlameDup".to_owned()];
    let index = IconLookupIndex::build(&names, &[first.clone(), second.clone()], 1);
    let (paths, hit) = index.lookup("flame-dup-app", 16);
    let copies = index
        .files()
        .get("flame-dup-app")
        .map_or(0, <[IndexedIconFile]>::len);
    let _ = fs::remove_dir_all(&first);
    let _ = fs::remove_dir_all(&second);
    assert!(hit);
    assert_eq!(
        paths,
        vec![first.join("icons/FlameDup/16x16/apps/flame-dup-app.png")]
    );
    assert_eq!(copies, 1, "same theme under two roots must be indexed once");
}

#[cfg(unix)]
#[test]
fn distinct_non_utf8_names_are_both_indexed() {
    use std::os::unix::ffi::OsStringExt;
    let data = fixture_root("rawname", line!());
    theme_fixture(&data, "RawNames", FIXED_16, None);
    let apps = data.join("icons/RawNames/16x16/apps");
    for raw in [b"a\xff.png".to_vec(), b"a\xfe.png".to_vec()] {
        fs::write(apps.join(std::ffi::OsString::from_vec(raw)), b"").expect("raw icon fixture");
    }
    let index = build_index(&data, &["RawNames"]);
    let _ = fs::remove_dir_all(&data);
    // Both names lossy-decode to `a\u{FFFD}`; the key must stay exact.
    let copies = index
        .files()
        .get("a\u{FFFD}")
        .map_or(0, <[IndexedIconFile]>::len);
    assert_eq!(
        copies, 2,
        "distinct raw names must not dedupe by lossy base"
    );
}

/// Measurement only (reads the host icon dirs): `cargo test -- --ignored --nocapture
/// measure_host_icon_index`.
#[test]
#[ignore = "reads host icon directories; measurement only"]
fn measure_host_icon_index() {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let data_dirs = vec![
        home.join(".local/share"),
        PathBuf::from("/usr/local/share"),
        PathBuf::from("/usr/share"),
    ];
    let names = vec![std::env::var("FLAMEWM_MEASURE_THEME").unwrap_or("PlagueSur".into())];
    let index = IconLookupIndex::build(&names, &data_dirs, 1);
    let started = std::time::Instant::now();
    let _ = index.lookup("folder", 48);
    println!("scan_ms={:.1}", started.elapsed().as_secs_f64() * 1e3);
    println!("{}", index.scan_footprint());
}

#[cfg(unix)]
#[test]
fn sorted_name_index_finds_first_middle_last_and_misses() {
    let data = fixture_root("sorted-names", line!());
    let base = data.join("icons/FlameSorted");
    fs::create_dir_all(base.join("16x16/apps")).expect("16 dir");
    fs::write(
        base.join("index.theme"),
        "[Icon Theme]\nName=FlameSorted\nDirectories=16x16/apps\n\n[16x16/apps]\nSize=16\nType=Fixed\n",
    )
    .expect("index.theme fixture");
    for icon in ["alpha.png", "mid.PNG", "zeta.svg", "zeta.png"] {
        fs::write(base.join("16x16/apps").join(icon), b"").expect("icon fixture");
    }
    let index = build_index(&data, &["FlameSorted"]);
    let hit = |name: &str| index.lookup(name, 16).0;
    assert_eq!(hit("alpha"), vec![base.join("16x16/apps/alpha.png")]);
    assert_eq!(hit("mid"), vec![base.join("16x16/apps/mid.PNG")]);
    // One best file per theme: PNG outranks SVG at the same size.
    assert_eq!(hit("zeta"), vec![base.join("16x16/apps/zeta.png")]);
    assert!(hit("0before").is_empty());
    assert!(hit("beta").is_empty());
    assert!(hit("zz").is_empty());
    let _ = fs::remove_dir_all(&data);
}

/// Equivalence dump on host icon dirs (ignored; run explicitly). Env:
/// `FLAMEWM_DUMP_NAMES` (one base name per line), `FLAMEWM_DUMP_OUT`,
/// optional `FLAMEWM_DUMP_THEME` (default `PlagueSur`).
#[test]
#[ignore = "reads host icon directories; equivalence dump only"]
fn dump_host_icon_lookups() {
    let (Some(names_path), Some(out_path)) = (
        std::env::var_os("FLAMEWM_DUMP_NAMES"),
        std::env::var_os("FLAMEWM_DUMP_OUT"),
    ) else {
        return;
    };
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let data_dirs = vec![
        home.join(".local/share"),
        PathBuf::from("/usr/local/share"),
        PathBuf::from("/usr/share"),
    ];
    let theme = std::env::var("FLAMEWM_DUMP_THEME").unwrap_or("PlagueSur".into());
    let index = IconLookupIndex::build(&[theme], &data_dirs, 1);
    let names = fs::read_to_string(names_path).unwrap();
    let mut out = String::new();
    for name in names.lines() {
        for size in [16u32, 22, 32, 48, 64, 128, 256] {
            let (paths, hit) = index.lookup(name, size);
            out.push_str(&format!("{name} {size} {hit} {paths:?}\n"));
        }
    }
    fs::write(out_path, out).unwrap();
}

#[test]
fn shared_listings_select_same_paths_as_fresh_build() {
    let data = fixture_root("shared-listings", line!());
    let mine = data.join("icons/FlameMine");
    for dir in ["16x16/apps", "scalable/apps", "16x16/status", "32x32/apps"] {
        fs::create_dir_all(mine.join(dir)).expect("mine dirs");
    }
    fs::write(
        mine.join("index.theme"),
        "[Icon Theme]\nName=FlameMine\nInherits=FlameBase\nDirectories=16x16/apps,scalable/apps,16x16/status,32x32/apps\n\n\
         [16x16/apps]\nSize=16\nType=Fixed\n\n[scalable/apps]\nSize=48\nType=Scalable\nMinSize=8\nMaxSize=512\n\n\
         [16x16/status]\nSize=16\nType=Fixed\nContext=Status\n\n[32x32/apps]\nSize=32\nType=Threshold\n",
    )
    .expect("mine index.theme");
    fs::write(mine.join("16x16/apps/shared-app.png"), b"").expect("mine png");
    fs::write(mine.join("16x16/apps/mine-only.png"), b"").expect("mine png");
    fs::write(mine.join("scalable/apps/vector-app.svg"), b"").expect("mine svg");
    fs::write(mine.join("16x16/status/flame-symbolic.svg"), b"").expect("mine symbolic");
    fs::write(mine.join("32x32/apps/shared-app.png"), b"").expect("mine 32 png");
    let base = data.join("icons/FlameBase");
    fs::create_dir_all(base.join("16x16/apps")).expect("base dirs");
    fs::create_dir_all(base.join("24x24/apps")).expect("base dirs");
    fs::write(
        base.join("index.theme"),
        "[Icon Theme]\nName=FlameBase\nDirectories=16x16/apps,24x24/apps\n\n[16x16/apps]\nSize=16\nType=Fixed\n\n[24x24/apps]\nSize=24\nType=Fixed\n",
    )
    .expect("base index.theme");
    fs::write(base.join("16x16/apps/shared-app.png"), b"").expect("base png");
    fs::write(base.join("24x24/apps/base-only.png"), b"").expect("base png");
    fs::write(base.join("24x24/apps/shared-app.png"), b"").expect("base png");

    let roots = [data.clone()];
    let names = ["FlameMine".to_owned()];
    let fresh = IconLookupIndex::build(&names, &roots, 1);
    let listings = Arc::new(DirListings::default());
    let shared = IconLookupIndex::build_shared(&names, &roots, 1, &listings);
    let lookups = [
        "shared-app", "mine-only", "base-only", "vector-app", "flame-symbolic", "hicolor-only", "missing",
    ];
    let mut hits = 0;
    for name in lookups {
        for px in [1, 16, 24, 32, 48, 64, 256] {
            let expected = fresh.lookup(name, px);
            assert_eq!(shared.lookup(name, px), expected, "{name} at {px}px");
            hits += usize::from(expected.1);
        }
    }
    let read_dirs = listings.by_dir.lock().map(|map| map.len()).unwrap_or(0);
    // A second one-shot build over the same listings must read nothing new.
    let again = IconLookupIndex::build_shared(&names, &roots, 1, &listings);
    assert_eq!(again.lookup("shared-app", 16), fresh.lookup("shared-app", 16));
    let read_dirs_after = listings.by_dir.lock().map(|map| map.len()).unwrap_or(0);
    let _ = fs::remove_dir_all(&data);
    assert!(hits > 0, "fixture must resolve some icons");
    assert!(read_dirs > 0, "shared scan must record its directory reads");
    assert_eq!(read_dirs, read_dirs_after, "second build reuses listings");
}
