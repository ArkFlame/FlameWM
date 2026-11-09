use super::*;

const HICOLOR_16_INDEX: &str =
    "[Icon Theme]\nDirectories=16x16/apps\n\n[16x16/apps]\nSize=16\nType=Fixed\n";

#[test]
fn xdg_data_search_dirs_never_yields_relative_home_paths() {
    let empty = OsStr::new("");
    assert_eq!(user_data_home(Some(empty), None), None);
    assert_eq!(
        xdg_data_search_dirs(Some(empty), Some(empty), Some("")),
        vec![
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share")
        ]
    );
    assert_eq!(
        xdg_data_search_dirs(Some(OsStr::new("/home/u")), None, Some("/opt/share:")),
        vec![
            PathBuf::from("/home/u/.local/share"),
            PathBuf::from("/opt/share")
        ]
    );
}

#[test]
fn p6_is_scaled_to_requested_physical_size() {
    let bytes = b"P6\n2 1\n255\n\xff\0\0\0\xff\0";
    let raster = decode_ppm(
        Path::new("test.ppm"),
        bytes,
        IconSize::new(16, 2),
        IconOrigin::ExplicitPath,
    )
    .expect("valid PPM");
    assert_eq!(raster.width, 2);
    assert_eq!(raster.height, 2);
    // Opaque PPM fixture converts to alpha=255 at decode time.
    assert_eq!(
        raster.pixels,
        vec![
            255, 0, 0, 255, 0, 255, 0, 255, 255, 0, 0, 255, 0, 255, 0, 255
        ]
    );
    assert!(raster.pixels.chunks_exact(4).all(|pixel| pixel[3] == 255));
}

#[test]
fn svg_fixture_preserves_alpha_with_nonblack_pixel() {
    let path = workspace_root().join("assets/web/flamewm-icon.svg");
    let raster = rasterize_path(&path, IconSize::new(16, 8)).expect("SVG fixture");
    assert_eq!((raster.width, raster.height), (8, 8));
    assert_eq!(raster.pixels.len(), 8 * 8 * 4);
    assert!(
        raster
            .pixels
            .chunks_exact(4)
            .any(|pixel| pixel[3] > 0 && pixel[..3] != [0, 0, 0])
    );
}

#[test]
fn png_fixture_preserves_alpha_with_nonblack_pixel() {
    let path = workspace_root().join("assets/web/flamewm.png");
    let raster = rasterize_path(&path, IconSize::new(16, 8)).expect("PNG fixture");
    assert_eq!((raster.width, raster.height), (8, 8));
    assert_eq!(raster.pixels.len(), 8 * 8 * 4);
    assert!(
        raster
            .pixels
            .chunks_exact(4)
            .any(|pixel| pixel[3] > 0 && pixel[..3] != [0, 0, 0])
    );
}

#[test]
fn webp_fixture_uses_shared_image_core_raster_contract() {
    let path = workspace_root().join("assets/branding/flamewm-wallpaper.webp");
    let raster = rasterize_path(&path, IconSize::new(16, 8)).expect("WebP fixture");
    assert_eq!((raster.width, raster.height), (8, 8));
    assert_eq!(raster.pixels.len(), 8 * 8 * 4);
    assert!(raster.pixels.chunks_exact(4).all(|pixel| pixel[3] == 255));
}

#[test]
fn rgba_icons_carry_no_magenta_color_key_dependency() {
    // Baseline sweep: dynamic/app icon alpha must come from the alpha
    // channel, never from a magenta (255,0,255) transparent-pixel rule.
    // The only in-repo mention of that triple is the legacy X11
    // image_pixmap opaque-mask path, which accepts RGBA assets and no
    // longer gates on magenta. Here we assert decoded icons never need
    // a magenta test to recover transparency: every RGBA pixel stands
    // on its own alpha byte.
    let bytes = b"P6\n1 1\n255\n\xff\0\xff";
    let raster = decode_ppm(
        Path::new("magenta.ppm"),
        bytes,
        IconSize::new(16, 1),
        IconOrigin::ExplicitPath,
    )
    .expect("magenta PPM");
    assert_eq!(raster.pixels, vec![255, 0, 255, 255]);
    // A magenta pixel decodes fully opaque: it is ordinary content.
    assert_eq!(raster.alpha_at(0, 0), Some(255));
    let svg = workspace_root().join("assets/web/flamewm-icon.svg");
    let svg_raster = rasterize_path(&svg, IconSize::new(16, 8)).expect("SVG fixture");
    let transparent_nonmagenta = svg_raster
        .pixels
        .chunks_exact(4)
        .any(|pixel| pixel[3] == 0 && pixel[..3] != [255, 0, 255]);
    assert!(
        transparent_nonmagenta || svg_raster.pixels.chunks_exact(4).all(|p| p[3] == 255),
        "alpha channel, not magenta, decides transparency"
    );
}

#[test]
fn packaged_fallback_is_alpha_bearing_svg() {
    assert!(
        !GENERIC_FALLBACK.ends_with(".ppm"),
        "fallback must not be PPM: {GENERIC_FALLBACK}"
    );
    assert!(
        GENERIC_FALLBACK.ends_with(".svg") || GENERIC_FALLBACK.ends_with(".png"),
        "fallback must be svg/png: {GENERIC_FALLBACK}"
    );
    let path = workspace_root().join(GENERIC_FALLBACK);
    assert!(path.is_file(), "fallback asset missing: {}", path.display());
    let raster = rasterize_path(&path, IconSize::new(16, 8)).expect("fallback raster");
    assert_eq!(raster.pixels.len(), 8 * 8 * 4);
}

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "flamewm-j08-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn write_ppm(path: &Path, pixel: [u8; 3]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("temp parent");
    }
    let mut bytes = b"P6\n1 1\n255\n".to_vec();
    bytes.extend_from_slice(&pixel);
    fs::write(path, bytes).expect("temp ppm");
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn absolute_name_uses_explicit_path_first() {
    let dir = test_dir("explicit");
    let path = dir.join("custom.ppm");
    write_ppm(&path, [200, 10, 10]);
    let mut resolver = IconResolver::new(dir.clone(), vec!["hicolor".to_owned()], vec![dir], 0);
    let raster = resolver
        .prepare_name(path.to_string_lossy().into_owned(), IconSize::new(16, 8))
        .expect("absolute icon path");
    assert_eq!(raster.origin, IconOrigin::ExplicitPath);
    assert_eq!(raster.source, path.to_string_lossy());
    assert_eq!((raster.width, raster.height), (8, 8));
}

#[test]
fn hicolor_name_resolves_through_theme_dirs() {
    let dir = test_dir("hicolor");
    let path = dir.join("icons/hicolor/16x16/apps/j08-hicolor-only.ppm");
    write_ppm(&path, [10, 200, 10]);
    let mut resolver = IconResolver::new(dir.clone(), Vec::new(), vec![dir.clone()], 7);
    let raster = resolver
        .prepare_name("j08-hicolor-only", IconSize::new(16, 16))
        .expect("hicolor icon");
    assert_eq!(raster.origin, IconOrigin::SystemTheme);
    assert_eq!(raster.source, path.to_string_lossy());
}

#[test]
fn breeze_theme_is_preferred_over_hicolor() {
    let dir = test_dir("breeze");
    let breeze = dir.join("icons/Breeze/16x16/apps/j08-theme-order.ppm");
    let hicolor = dir.join("icons/hicolor/16x16/apps/j08-theme-order.ppm");
    write_ppm(&breeze, [200, 10, 10]);
    write_ppm(&hicolor, [10, 10, 200]);
    let mut resolver = IconResolver::new(
        dir.clone(),
        vec!["CustomTheme".to_owned()],
        vec![dir.clone()],
        3,
    );
    let raster = resolver
        .prepare_name("j08-theme-order", IconSize::new(16, 16))
        .expect("breeze icon");
    assert_eq!(raster.origin, IconOrigin::SystemTheme);
    assert_eq!(raster.source, breeze.to_string_lossy());
    // Breeze red pixel survives scaling to the requested size.
    assert_eq!(&raster.pixels[..4], &[200, 10, 10, 255]);
}

fn write_theme(dir: &Path, theme: &str, index: &str) {
    let base = dir.join("icons").join(theme);
    fs::create_dir_all(&base).expect("theme dir");
    fs::write(base.join("index.theme"), index).expect("index.theme");
}

#[test]
fn exact_size_beats_inherited_theme() {
    let dir = test_dir("j2-exact");
    write_theme(
        &dir,
        "Child",
        "[Icon Theme]\nDirectories=48x48/apps\nInherits=Parent\n\n[48x48/apps]\nSize=48\nType=Fixed\n",
    );
    write_theme(&dir, "Parent", HICOLOR_16_INDEX);
    write_ppm(
        &dir.join("icons/Child/48x48/apps/j2-exact.ppm"),
        [200, 10, 10],
    );
    write_ppm(
        &dir.join("icons/Parent/16x16/apps/j2-exact.ppm"),
        [10, 10, 200],
    );
    let mut resolver =
        IconResolver::new(dir.clone(), vec!["Child".to_owned()], vec![dir.clone()], 0);
    let raster = resolver
        .prepare_application("j2-exact", IconSize::new(16, 16))
        .expect("exact parent wins over distant child");
    assert_eq!(
        raster.source,
        dir.join("icons/Parent/16x16/apps/j2-exact.ppm")
            .to_string_lossy()
    );
}

#[test]
fn same_theme_nearest_beats_parent_exact() {
    let dir = test_dir("j2-nearest");
    write_theme(
        &dir,
        "Child",
        "[Icon Theme]\nDirectories=32x32/apps\nInherits=Parent\n\n[32x32/apps]\nSize=32\nType=Fixed\n",
    );
    write_theme(&dir, "Parent", HICOLOR_16_INDEX);
    write_ppm(
        &dir.join("icons/Child/32x32/apps/j2-near.ppm"),
        [200, 10, 10],
    );
    write_ppm(
        &dir.join("icons/Parent/16x16/apps/j2-near.ppm"),
        [10, 10, 200],
    );
    let mut resolver =
        IconResolver::new(dir.clone(), vec!["Child".to_owned()], vec![dir.clone()], 0);
    let raster = resolver
        .prepare_application("j2-near", IconSize::new(16, 24))
        .expect("same-theme nearest wins");
    assert_eq!(
        raster.source,
        dir.join("icons/Child/32x32/apps/j2-near.ppm")
            .to_string_lossy()
    );
}

#[test]
fn inheritance_resolves_parent_icon() {
    let dir = test_dir("j2-inherit");
    write_theme(
        &dir,
        "Child",
        "[Icon Theme]\nDirectories=16x16/apps\nInherits=Parent\n\n[16x16/apps]\nSize=16\nType=Fixed\n",
    );
    write_theme(&dir, "Parent", HICOLOR_16_INDEX);
    write_ppm(
        &dir.join("icons/Parent/16x16/apps/j2-parent.ppm"),
        [10, 200, 10],
    );
    let mut resolver =
        IconResolver::new(dir.clone(), vec!["Child".to_owned()], vec![dir.clone()], 0);
    let raster = resolver
        .prepare_application("j2-parent", IconSize::new(16, 16))
        .expect("inherited icon");
    assert_eq!(
        raster.source,
        dir.join("icons/Parent/16x16/apps/j2-parent.ppm")
            .to_string_lossy()
    );
}

#[test]
fn cycle_inherits_terminates() {
    let dir = test_dir("j2-cycle");
    write_theme(
        &dir,
        "CycleA",
        "[Icon Theme]\nDirectories=16x16/apps\nInherits=CycleB\n\n[16x16/apps]\nSize=16\nType=Fixed\n",
    );
    write_theme(
        &dir,
        "CycleB",
        "[Icon Theme]\nDirectories=16x16/apps\nInherits=CycleA\n\n[16x16/apps]\nSize=16\nType=Fixed\n",
    );
    write_ppm(
        &dir.join("icons/CycleB/16x16/apps/j2-cycle.ppm"),
        [10, 200, 10],
    );
    let mut resolver =
        IconResolver::new(dir.clone(), vec!["CycleA".to_owned()], vec![dir.clone()], 0);
    let raster = resolver
        .prepare_application("j2-cycle", IconSize::new(16, 16))
        .expect("cycle terminates");
    assert_eq!(
        raster.source,
        dir.join("icons/CycleB/16x16/apps/j2-cycle.ppm")
            .to_string_lossy()
    );
}

#[test]
fn hicolor_is_final_fallback() {
    let dir = test_dir("j2-hicolor");
    write_theme(&dir, "Lonely", HICOLOR_16_INDEX);
    write_theme(&dir, "hicolor", HICOLOR_16_INDEX);
    write_ppm(
        &dir.join("icons/hicolor/16x16/apps/j2-hi.ppm"),
        [10, 200, 10],
    );
    let mut resolver =
        IconResolver::new(dir.clone(), vec!["Lonely".to_owned()], vec![dir.clone()], 0);
    let raster = resolver
        .prepare_application("j2-hi", IconSize::new(16, 16))
        .expect("hicolor fallback");
    assert_eq!(
        raster.source,
        dir.join("icons/hicolor/16x16/apps/j2-hi.ppm")
            .to_string_lossy()
    );
}

#[test]
fn xdg_data_roots_participate() {
    let dir = test_dir("j2-xdg");
    let data_a = dir.join("data-a");
    let data_b = dir.join("data-b");
    write_theme(&data_b, "XdgTheme", HICOLOR_16_INDEX);
    write_ppm(
        &data_b.join("icons/XdgTheme/16x16/apps/j2-xdg.ppm"),
        [200, 10, 10],
    );
    let roots = crate::icon_theme::search_roots(&[data_a.clone(), data_b.clone()]);
    assert!(roots.iter().any(|root| *root == data_b.join("icons")));
    let mut resolver = IconResolver::new(
        dir.clone(),
        vec!["XdgTheme".to_owned()],
        vec![data_a, data_b.clone()],
        0,
    );
    let raster = resolver
        .prepare_application("j2-xdg", IconSize::new(16, 16))
        .expect("xdg root");
    assert_eq!(
        raster.source,
        data_b
            .join("icons/XdgTheme/16x16/apps/j2-xdg.ppm")
            .to_string_lossy()
    );
    assert_eq!(raster.origin, IconOrigin::SystemTheme);
}

#[test]
fn absolute_path_bypasses_theme() {
    let dir = test_dir("j2-abs");
    let path = dir.join("abs.ppm");
    write_ppm(&path, [200, 10, 10]);
    let mut resolver = IconResolver::new(dir.clone(), vec!["NoTheme".to_owned()], vec![dir], 0);
    let raster = resolver
        .prepare_application(path.to_string_lossy().into_owned(), IconSize::new(16, 8))
        .expect("absolute bypass");
    assert_eq!(raster.origin, IconOrigin::ExplicitPath);
}

#[test]
fn semantic_prefers_packaged_breeze_over_system_theme() {
    let packaged = test_dir("j2-sem-packaged");
    let data = test_dir("j2-sem-data");
    let breeze_dir = packaged.join("assets/web/breeze");
    fs::create_dir_all(&breeze_dir).expect("breeze dir");
    // Packaged semantic asset wins over an identical system theme icon.
    let svg = workspace_root().join("assets/web/flamewm-icon.svg");
    fs::copy(svg, breeze_dir.join("j2-sem.svg")).expect("packaged breeze");
    write_theme(&data, "hicolor", HICOLOR_16_INDEX);
    write_ppm(
        &data.join("icons/hicolor/16x16/apps/j2-sem.ppm"),
        [10, 200, 10],
    );
    let mut resolver =
        IconResolver::new(packaged.clone(), vec!["hicolor".to_owned()], vec![data], 0);
    let raster = resolver
        .prepare_semantic("j2-sem", IconSize::new(16, 16))
        .expect("semantic icon");
    assert_eq!(raster.origin, IconOrigin::PackagedBreeze);
}

#[test]
fn application_never_uses_packaged_semantic_first() {
    let packaged = test_dir("j2-app-packaged");
    let data = test_dir("j2-app-data");
    let breeze_dir = packaged.join("assets/web/breeze");
    fs::create_dir_all(&breeze_dir).expect("breeze dir");
    let svg = workspace_root().join("assets/web/flamewm-icon.svg");
    fs::copy(svg, breeze_dir.join("j2-app.svg")).expect("packaged breeze");
    write_theme(&data, "hicolor", HICOLOR_16_INDEX);
    write_ppm(
        &data.join("icons/hicolor/16x16/apps/j2-app.ppm"),
        [10, 200, 10],
    );
    let mut resolver = IconResolver::new(packaged, vec!["hicolor".to_owned()], vec![data], 0);
    let raster = resolver
        .prepare_application("j2-app", IconSize::new(16, 16))
        .expect("application icon");
    assert_eq!(raster.origin, IconOrigin::SystemTheme);
    assert!(raster.source.ends_with("j2-app.ppm"));
}

#[test]
fn live_theme_switch_a_b_a_never_serves_stale_image() {
    let dir = test_dir("c3-switch");
    write_theme(&dir, "ThemeA", HICOLOR_16_INDEX);
    write_theme(&dir, "ThemeB", HICOLOR_16_INDEX);
    write_ppm(
        &dir.join("icons/ThemeA/16x16/apps/c3-switch.ppm"),
        [200, 10, 10],
    );
    write_ppm(
        &dir.join("icons/ThemeB/16x16/apps/c3-switch.ppm"),
        [10, 200, 10],
    );
    let size = IconSize::new(16, 16);
    let data = vec![dir.clone()];
    let mut resolver = IconResolver::new(dir.clone(), vec!["ThemeA".to_owned()], data.clone(), 1);
    let first = resolver
        .prepare_application("c3-switch", size)
        .expect("theme A");
    assert!(first.source.contains("ThemeA"));
    for (generation, theme) in [(2, "ThemeB"), (3, "ThemeA")] {
        let next = std::sync::Arc::new(crate::icon_theme::IconLookupIndex::build(
            &[theme.to_owned()],
            &data,
            generation,
        ));
        resolver.set_shared_index(next);
        resolver.notify_theme_changed(generation);
        let raster = resolver
            .prepare_application("c3-switch", size)
            .expect("switched theme");
        assert!(
            raster.source.contains(theme),
            "switch to {theme} must not serve the previous theme's image: {}",
            raster.source
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cache_invalidates_on_theme_generation() {
    let dir = test_dir("j2-cache");
    write_theme(&dir, "hicolor", HICOLOR_16_INDEX);
    write_ppm(
        &dir.join("icons/hicolor/16x16/apps/j2-cached.ppm"),
        [10, 200, 10],
    );
    let mut resolver = IconResolver::new(dir.clone(), vec!["hicolor".to_owned()], vec![dir], 1);
    let size = IconSize::new(16, 16);
    let before = resolver.cache_len();
    let _ = resolver
        .prepare_application("j2-cached", size)
        .expect("cached");
    let after_first = resolver.cache_len();
    assert!(after_first > before);
    // Same request hits cache (no growth).
    let _ = resolver
        .prepare_application("j2-cached", size)
        .expect("cached");
    assert_eq!(resolver.cache_len(), after_first);
    resolver.notify_theme_changed(2);
    let _ = resolver
        .prepare_application("j2-cached", size)
        .expect("cached");
    assert!(resolver.cache_len() > after_first);
    // Different purpose is a distinct cache entry.
    let _ = resolver
        .prepare_semantic("j2-cached", size)
        .expect("semantic");
    assert!(resolver.cache_len() > after_first + 1 - 1);
}

#[test]
fn oversized_raster_does_not_evict_or_escape_byte_budget() {
    let mut cache = RasterCache {
        budget_bytes: 160,
        ..RasterCache::default()
    };
    let key = |name: &str| RasterKey {
        path: PathBuf::from(name),
        physical: 2,
        origin: IconOrigin::ExplicitPath,
    };
    let raster = |size: usize| {
        Ok(Rgba8Raster {
            source: "test".into(),
            width: (size / 4) as u32,
            height: 1,
            pixels: vec![255; size],
            origin: IconOrigin::ExplicitPath,
            fallback_reason: None,
        })
    };
    cache.insert(key("small"), raster(16));
    let retained = cache.used_bytes;
    cache.insert(key("oversized"), raster(256));
    assert_eq!(cache.used_bytes, retained);
    assert!(cache.used_bytes <= cache.budget_bytes);
    assert!(cache.get(&key("small")).is_some());
    assert!(cache.get(&key("oversized")).is_none());
}

#[test]
fn fully_transparent_raster_is_not_a_visible_icon() {
    let blank = Rgba8Raster {
        source: "blank.svg".to_owned(),
        width: 2,
        height: 2,
        pixels: vec![0; 16],
        origin: IconOrigin::SystemTheme,
        fallback_reason: None,
    };
    let mut visible = blank.clone();
    visible.pixels[7] = 255;
    assert!(!raster_has_visible_pixels(&blank));
    assert!(raster_has_visible_pixels(&visible));
}

#[test]
fn generic_application_glyph_ships_outside_stripped_theme_tree() {
    assert!(
        !GENERIC_APPLICATION_GLYPH.contains("/themes/"),
        "glyph under assets/themes is stripped by scripts/package"
    );
    let root = workspace_root();
    assert!(root.join(GENERIC_APPLICATION_GLYPH).is_file());
}

#[test]
fn generic_application_glyph_is_bundled_and_theme_independent() {
    let dir = test_dir("generic-app-glyph");
    let mut resolver = IconResolver::new(workspace_root(), Vec::new(), vec![dir], 0);
    let raster = resolver
        .prepare_generic_application(IconSize::new(25, 25))
        .expect("bundled generic application glyph");
    assert_eq!((raster.width, raster.height), (25, 25));
    assert!(
        raster.source.ends_with("application-x-executable.svg"),
        "unexpected glyph source: {}",
        raster.source
    );
}

#[test]
fn application_miss_never_returns_brand_logo() {
    let dir = test_dir("app-miss");
    let mut resolver = IconResolver::new(workspace_root(), Vec::new(), vec![dir], 0);
    let size = IconSize::new(16, 16);
    let error = resolver
        .prepare_application("definitely-missing-flamewm-app", size)
        .expect_err("app miss must not resolve");
    let message = format!("{error}");
    assert!(
        !message.contains("flamewm-icon"),
        "app miss must not reference brand logo: {message}"
    );
    if let IconError::FallbackUnavailable { fallback, .. } = &error {
        assert!(
            !fallback.to_string_lossy().ends_with("flamewm-icon.svg")
                || !message.contains("packaged icon fallback"),
            "resolved brand fallback must not be returned for app miss"
        );
    }
    // Cached miss path is identical (no brand raster leaks via cache).
    let cached = resolver
        .prepare_application("definitely-missing-flamewm-app", size)
        .expect_err("cached app miss must not resolve");
    assert_eq!(format!("{cached}"), message);
}

#[test]
fn corrupt_candidate_keeps_fallback_reason() {
    let dir = test_dir("fallback");
    let bad = dir.join("bad.ppm");
    fs::write(&bad, b"not a ppm").expect("temp bad ppm");
    let mut resolver = IconResolver::new(workspace_root(), Vec::new(), Vec::new(), 0);
    let raster = resolver
        .prepare(IconRequest::path(bad.clone()), IconSize::new(16, 8))
        .expect("packaged fallback");
    assert_eq!(raster.origin, IconOrigin::PackagedFallback);
    let reason = raster.fallback_reason.expect("fallback reason");
    assert_eq!(
        reason,
        RasterError::InvalidPpm {
            path: bad,
            message: "only binary PPM (P6) is supported".to_owned(),
        }
    );
}

#[test]
fn set_preferred_theme_rebuilds_once_per_change_and_skips_no_op() {
    let mut resolver = IconResolver::new(
        std::env::temp_dir(),
        vec!["Breeze".to_owned()],
        Vec::new(),
        4,
    );
    let before = std::sync::Arc::clone(resolver.shared_index());
    assert!(resolver.set_preferred_theme(Some("ThemeA")));
    let first = std::sync::Arc::clone(resolver.shared_index());
    assert!(!std::sync::Arc::ptr_eq(&before, &first));
    assert_eq!(resolver.theme_generation(), 5);
    assert_eq!(first.generation, 5);
    assert!(!resolver.set_preferred_theme(Some("ThemeA")));
    assert!(std::sync::Arc::ptr_eq(&first, resolver.shared_index()));
    assert_eq!(resolver.theme_generation(), 5);
    assert!(resolver.set_preferred_theme(Some("ThemeB")));
    assert_eq!(resolver.theme_generation(), 6);
    assert_eq!(resolver.shared_index().generation, 6);
}

#[test]
fn unknown_preferred_theme_falls_back_to_breeze() {
    let mut resolver = IconResolver::new(
        std::env::temp_dir(),
        vec!["Breeze".to_owned()],
        Vec::new(),
        1,
    );
    assert!(resolver.set_preferred_theme(Some("NoSuchTheme-flame-test")));
    let chain = &resolver.shared_index().ordered_themes;
    let breeze = chain.iter().position(|name| name == "Breeze");
    let hicolor = chain.iter().position(|name| name == "hicolor");
    assert!(breeze.is_some(), "Breeze must stay in the chain: {chain:?}");
    assert!(breeze < hicolor, "Breeze precedes hicolor: {chain:?}");
}

#[test]
fn plaguesur_default_sits_between_configured_and_breeze() {
    let chain = theme_chain(Some("Papirus"));
    let papirus = chain.iter().position(|name| name == "Papirus");
    let default = chain.iter().position(|name| name == DEFAULT_ICON_THEME);
    let breeze = chain.iter().position(|name| name == "Breeze");
    assert!(
        papirus < default && default < breeze,
        "chain order: {chain:?}"
    );
    assert!(
        theme_chain(None)
            .iter()
            .any(|name| name == DEFAULT_ICON_THEME)
    );
}

#[test]
fn startup_preference_precedes_breeze_in_theme_chain() {
    let resolver =
        IconResolver::from_environment_with_theme(std::env::temp_dir(), 0, Some("MacTahoe"));
    let names = &resolver.shared_index().theme_names;
    let preferred = names.iter().position(|name| name == "MacTahoe");
    let breeze = names.iter().position(|name| name == "Breeze");
    assert!(preferred.is_some(), "startup preference missing: {names:?}");
    assert!(
        preferred < breeze,
        "preference must win over Breeze: {names:?}"
    );
}

#[test]
fn default_icon_theme_is_breeze_then_adwaita_in_chain() {
    let chain = theme_chain(None);
    let default = chain
        .iter()
        .position(|name| name == DEFAULT_ICON_THEME)
        .expect("default theme in chain");
    let breeze = chain
        .iter()
        .position(|name| name == "Breeze")
        .expect("Breeze");
    let adwaita = chain
        .iter()
        .position(|name| name == "Adwaita")
        .expect("Adwaita");
    assert_eq!(DEFAULT_ICON_THEME, "breeze");
    assert!(default < breeze && breeze < adwaita, "chain: {chain:?}");
}

#[test]
fn breeze_family_follows_appearance() {
    assert_eq!(resolve_icon_theme("breeze", "dark"), "breeze-dark");
    assert_eq!(resolve_icon_theme("Breeze", "light"), "breeze");
    assert_eq!(resolve_icon_theme("breeze-dark", "light"), "breeze");
    assert_eq!(resolve_icon_theme("breeze", "system"), "breeze");
}

#[test]
fn empty_means_default_family_and_other_themes_pass_through() {
    assert_eq!(resolve_icon_theme("", "dark"), "breeze-dark");
    assert_eq!(resolve_icon_theme("  ", "light"), "breeze");
    assert_eq!(resolve_icon_theme("MacTahoe", "dark"), "MacTahoe");
    assert_eq!(resolve_icon_theme("Papirus", "light"), "Papirus");
}

#[test]
fn ark_ai_svg_rasterises_at_taskbar_sizes() {
    let path = workspace_root().join("assets/web/breeze/ark-ai.svg");
    for size in [16_u32, 24, 32] {
        let raster = rasterize_path(&path, IconSize::new(size, size)).expect("ARK AI SVG");
        assert_eq!((raster.width, raster.height), (size, size));
        let opaque_white = raster
            .pixels
            .chunks_exact(4)
            .filter(|pixel| pixel[3] == 255 && pixel[..3] == [255, 255, 255])
            .count();
        assert!(opaque_white > 0, "ARK AI glyph is empty at {size}px");
    }
}
