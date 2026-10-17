//! C09 lookup-correctness guards: normalization, per-theme order,
//! pixmaps, empty contract, snapshot stability, 20-worker concurrency.

use std::fs;
use std::path::{Path, PathBuf};

use flamewm_integrations_linux::icon_service::{
    ICON_RESOLVE_CONCURRENCY, IconJob, IconPriority, IconService,
};
use flamewm_integrations_linux::icons::{IconLookupPurpose, IconRequest, IconResolver, IconSize};

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "flamewm-c09-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|v| v.as_nanos())
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

fn write_theme(dir: &Path, theme: &str, index: &str) {
    let base = dir.join("icons").join(theme);
    fs::create_dir_all(&base).expect("theme dir");
    fs::write(base.join("index.theme"), index).expect("index.theme");
}

fn key(name: &str) -> flamewm_integrations_linux::icons::IconKey {
    flamewm_integrations_linux::icons::IconKey::new(
        IconLookupPurpose::Application,
        IconRequest::name(name),
        IconSize::new(16, 16),
        1,
        1,
    )
}

#[test]
fn twenty_concurrent_requests_complete_with_twenty_workers() {
    assert_eq!(ICON_RESOLVE_CONCURRENCY, 20);
    // Hermetic: empty theme/data dirs so no live system icon scan
    // (a from_environment resolver walks real Breeze/hicolor trees;
    // 20 workers x thousands of cold stats hung this test).
    let dir = test_dir("workers");
    let mut service = IconService::new(20, 256, move || {
        IconResolver::new(dir.clone(), Vec::new(), Vec::new(), 0)
    });
    assert_eq!(service.worker_count(), 20);
    for index in 0..20 {
        service
            .submit(IconJob {
                key: key(&format!("c09-{index}")),
                priority: IconPriority::High,
            })
            .expect("submit");
    }
    let mut got = 0;
    for _ in 0..20 {
        if service.take_ready().is_some() {
            got += 1;
        }
    }
    assert_eq!(got, 20);
}

#[test]
fn no_rescan_after_remove_dir_without_invalidation() {
    let dir = test_dir("noscan");
    write_theme(
        &dir,
        "Snap",
        "[Icon Theme]\nDirectories=16x16/apps\n\n[16x16/apps]\nSize=16\nType=Fixed\n",
    );
    let file = dir.join("icons/Snap/16x16/apps/c09-snap2.ppm");
    write_ppm(&file, [10, 200, 10]);
    let resolver = IconResolver::new(dir.clone(), vec!["Snap".to_owned()], vec![dir.clone()], 0);
    assert!(!resolver.shared_index().lookup("c09-snap2", 16).0.is_empty());
    fs::remove_dir_all(dir.join("icons/Snap/16x16")).expect("remove dir");
    let (paths, hit) = resolver.shared_index().lookup("c09-snap2", 16);
    assert!(hit, "snapshot selection without rescan");
    assert_eq!(paths, vec![file]);
}
