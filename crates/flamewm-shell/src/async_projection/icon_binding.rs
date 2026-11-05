//! Icon target bindings: exact node binding, epochs and worker-result application.
use super::*;

/// Start and task slot ranges; prewarm sentinels past the range never bind a node.
pub(super) fn target_in_range(target: IconTarget) -> bool {
    match target {
        IconTarget::StartSlot(n) => u32::from(n) < START_APP_SLOT_COUNT as u32,
        IconTarget::LauncherSlot(n) => usize::from(n) < crate::launcher::LAUNCHER_ROWS,
        IconTarget::TaskSlot(n) => u32::from(n) < TASK_SLOT_COUNT as u32,
    }
}

/// Exact icon target binding. A worker completion mutates its node only
/// when the live binding still equals key + generation + view epoch;
/// otherwise it is a stale drop. `view_epoch` is domain-scoped: Start
/// slots use the start epoch (bumped on category/search/rebuild), task
/// slots use the task epoch (bumped on task-slot reprojection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconBinding {
    pub key: IconCacheKey,
    pub target: IconTarget,
    pub generation: u64,
    pub view_epoch: u64,
}

/// Bindings are keyed by target and raster edge: per-output panels share one
/// target id but may request different edges (`HiDPI` mix), and each must keep
/// its own exact-size binding instead of overwriting the other's.
pub(super) type BindingKey = (IconTarget, u32);

pub(super) fn bindings() -> &'static std::sync::Mutex<HashMap<BindingKey, IconBinding>> {
    static B: OnceLock<std::sync::Mutex<HashMap<BindingKey, IconBinding>>> = OnceLock::new();
    B.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

pub(super) static START_EPOCH: AtomicU64 = AtomicU64::new(0);

pub(super) static TASK_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Bump the start view epoch: category, search query, or model rebuild.
pub fn bump_start_view_epoch() -> u64 {
    START_EPOCH.fetch_add(1, Ordering::SeqCst) + 1
}

/// Bump the task view epoch: task-slot reprojection.
pub fn bump_task_view_epoch() -> u64 {
    TASK_EPOCH.fetch_add(1, Ordering::SeqCst) + 1
}

pub(super) fn epoch_for_target(target: IconTarget) -> u64 {
    match target {
        IconTarget::StartSlot(_) | IconTarget::LauncherSlot(_) => {
            START_EPOCH.load(Ordering::SeqCst)
        }
        IconTarget::TaskSlot(_) => TASK_EPOCH.load(Ordering::SeqCst),
    }
}

/// Exact image node id for one icon target, using source ids only.
#[must_use]
pub fn icon_node_id_for_target(target: IconTarget) -> String {
    match target {
        IconTarget::StartSlot(n) => format!("start-app-slot-{}-icon", u32::from(n) + 1),
        IconTarget::LauncherSlot(n) => format!("launcher-row-{}-icon", u32::from(n) + 1),
        IconTarget::TaskSlot(n) => format!("task-slot-{}-icon", u32::from(n) + 1),
    }
}

pub(super) fn record_binding(target: IconTarget, name: &str, w: u32, h: u32, generation: u64) {
    let binding = IconBinding {
        key: (name.to_owned(), w, h),
        target,
        generation,
        view_epoch: epoch_for_target(target),
    };
    if let Ok(mut bindings) = bindings().lock() {
        bindings.insert((target, w), binding);
    }
}

/// Verify-then-apply one worker completion against the live binding.
/// The caller inserts the raster into the icon cache first (cache is
/// view-independent); this only gates node mutation. On match: mutates
/// exactly the one image node for `target` (existing invalidation drives
/// node/surface dirtiness). On mismatch: stale drop, counted, document
/// untouched. Out-of-range prewarm sentinels (`StartSlot(n)` at or above
/// the slot count, never bound to a node) are cache-only: no counter, no
/// mutation. Returns true when the node was mutated.
pub fn apply_icon_result(
    document: &mut impl UiDocumentAccess,
    target: IconTarget,
    name: &str,
    w: u32,
    h: u32,
    generation: u64,
    raster: impl Into<IconRaster>,
) -> Result<bool, String> {
    if !target_in_range(target) {
        return Ok(false);
    }
    let expected = IconBinding {
        key: (name.to_owned(), w, h),
        target,
        generation,
        view_epoch: epoch_for_target(target),
    };
    let matches = bindings()
        .lock()
        .ok()
        .and_then(|bindings| bindings.get(&(target, w)).cloned())
        .is_some_and(|current| current == expected);
    if !matches {
        stale_counter().increment();
        return Ok(false);
    }
    paint_raster(document, &icon_node_id_for_target(target), raster.into())?;
    Ok(true)
}

/// Hands the shared pixel buffer to the document; no per-document copy.
pub(super) fn paint_raster(
    document: &mut impl UiDocumentAccess,
    id: &str,
    raster: IconRaster,
) -> Result<(), String> {
    document.image_rgba8_shared(
        id,
        raster.source,
        raster.width,
        raster.height,
        raster.pixels,
    )
}

/// Repaints one target's node from the view-independent cache when its live
/// binding names a key whose raster is already cached. Covers a completion
/// dropped as stale after an epoch bump: the raster is cached, so the node
/// must be repainted without waiting for another projection.
pub fn apply_cached_binding(
    document: &mut impl UiDocumentAccess,
    target: IconTarget,
    edge: u32,
) -> Result<bool, String> {
    if !target_in_range(target) {
        return Ok(false);
    }
    let bound = bindings().lock().ok().and_then(|bindings| {
        bindings
            .get(&(target, edge))
            .map(|binding| binding.key.clone())
    });
    let Some((name, w, h)) = bound else {
        return Ok(false);
    };
    let Some(raster) = cached_image(&name, w, h) else {
        return Ok(false);
    };
    paint_raster(document, &icon_node_id_for_target(target), raster)?;
    Ok(true)
}

#[cfg(test)]
pub fn test_record_binding(target: IconTarget, name: &str, w: u32, h: u32, generation: u64) {
    record_binding(target, name, w, h, generation);
}

#[cfg(test)]
pub(crate) fn icon_test_lock() -> &'static std::sync::Mutex<()> {
    static M: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
pub fn test_reset_icon_bindings() {
    START_EPOCH.store(0, Ordering::SeqCst);
    TASK_EPOCH.store(0, Ordering::SeqCst);
    if let Ok(mut bindings) = bindings().lock() {
        bindings.clear();
    }
    if let Ok(mut cache) = icon_cache().lock() {
        cache.clear();
    }
}

/// Visible-row request: cache hit applies now; miss applies the neutral
/// packaged/system image when a worker already resolved it, else the
/// transparent empty placeholder, and queues one HIGH worker request.
/// Miss order: last-valid (caller cache) -> neutral packaged/system ->
/// empty/transparent. Generic app misses never show brand artwork: the
/// worker drops brand fallback rasters to None, and the caller keeps the
/// last valid image or the transparent placeholder. Never flame-red, never
/// retains a stale icon.
pub(crate) fn schedule_visible(
    document: &mut impl UiDocumentAccess,
    icon_id: &str,
    loader: &mut Option<&mut IconLoader>,
    target: IconTarget,
    name: &str,
    w: u32,
    h: u32,
) -> Result<(), String> {
    if let Some(hit) = cached_image(name, w, h) {
        paint_raster(document, icon_id, hit)?;
        // The node now shows this key: bind it so a late completion for an
        // earlier key cannot overwrite it.
        if let Some(loader) = loader.as_deref_mut() {
            record_binding(target, name, w, h, loader.generation());
        }
        return Ok(());
    }
    // Miss: last-valid is absent (caller cache missed); pending worker
    // results arrive as neutral packaged/system rasters via
    // `note_icon_result`. Until then apply transparent empty so the row
    // never shows a stale icon and never a flame-red square.
    document.image_rgb8(icon_id, empty_image(w, h))?;
    fallback_counter().increment();
    if let Some(loader) = loader.as_deref_mut() {
        let generation = loader.generation();
        loader.request(target, name, w, h);
        request_counter().increment();
        // Bind this node to key + generation + view epoch; only the
        // matching completion may mutate it.
        record_binding(target, name, w, h, generation);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_projection::tests::*;

    #[test]
    fn icon_binding_match_mutates_exact_node_only() {
        let _guard = icon_test_lock().lock();
        test_reset_icon_bindings();
        let mut doc = FakeDoc::default();
        test_record_binding(IconTarget::StartSlot(0), "icon-a", 20, 20, 7);
        let applied = apply_icon_result(
            &mut doc,
            IconTarget::StartSlot(0),
            "icon-a",
            20,
            20,
            7,
            sized_test_image("a", 20),
        )
        .expect("apply ok");
        assert!(applied, "live binding must mutate");
        assert_eq!(doc.images, vec!["start-app-slot-1-icon".to_owned()]);
    }

    #[test]
    fn icon_binding_stale_on_epoch_bump_touches_nothing() {
        let _guard = icon_test_lock().lock();
        test_reset_icon_bindings();
        let mut doc = FakeDoc::default();
        test_record_binding(IconTarget::StartSlot(0), "icon-a", 20, 20, 7);
        bump_start_view_epoch();
        let applied = apply_icon_result(
            &mut doc,
            IconTarget::StartSlot(0),
            "icon-a",
            20,
            20,
            7,
            sized_test_image("a", 20),
        )
        .expect("stale check ok");
        assert!(!applied, "epoch bump must stale the completion");
        assert!(doc.images.is_empty(), "stale drop touches no node");
    }

    #[test]
    fn icon_binding_stale_on_key_or_generation_mismatch() {
        let _guard = icon_test_lock().lock();
        test_reset_icon_bindings();
        let mut doc = FakeDoc::default();
        test_record_binding(IconTarget::TaskSlot(1), "icon-a", 25, 25, 3);
        let wrong_name = apply_icon_result(
            &mut doc,
            IconTarget::TaskSlot(1),
            "icon-b",
            25,
            25,
            3,
            sized_test_image("b", 25),
        )
        .expect("stale check ok");
        assert!(!wrong_name, "key mismatch must stale");
        let wrong_gen = apply_icon_result(
            &mut doc,
            IconTarget::TaskSlot(1),
            "icon-a",
            25,
            25,
            4,
            sized_test_image("b", 25),
        )
        .expect("stale check ok");
        assert!(!wrong_gen, "generation mismatch must stale");
        assert!(doc.images.is_empty(), "stale drops touch no node");
    }

    #[test]
    fn icon_ready_after_epoch_bump_repaints_bound_node() {
        let _guard = icon_test_lock().lock();
        test_reset_icon_bindings();
        let mut doc = FakeDoc::default();
        test_record_binding(IconTarget::TaskSlot(1), "icon-a", 25, 25, 3);
        // A projection bumps the task epoch without re-binding this slot.
        bump_task_view_epoch();
        note_icon_result("icon-a", 25, 25, sized_test_image("a", 20));
        let completed = apply_icon_result(
            &mut doc,
            IconTarget::TaskSlot(1),
            "icon-a",
            25,
            25,
            3,
            sized_test_image("a", 20),
        )
        .expect("stale check ok");
        assert!(!completed, "epoch bump stales the late completion");
        assert!(doc.images.is_empty());
        let repainted =
            apply_cached_binding(&mut doc, IconTarget::TaskSlot(1), 25).expect("cached repaint ok");
        assert!(repainted, "cached raster for the bound key must repaint");
        assert_eq!(doc.images, vec!["task-slot-2-icon".to_owned()]);
    }

    #[test]
    fn visible_row_miss_applies_fallback_now() {
        let _guard = icon_test_lock().lock();
        let model = StartModel::new(vec![flamewm_api::applications::DesktopApplication {
            id: flamewm_api::DesktopAppId::new("org.example.x"),
            name: "X".to_owned(),
            generic_name: String::new(),
            comment: String::new(),
            startup_wm_class: String::new(),
            argv: Vec::new(),
            keywords: Vec::new(),
            icon_name: "missing-icon".to_owned(),
            categories: Vec::new(),
        }]);
        let doc = project_start_doc(&model, StartCategory::All, "");
        assert!(
            doc.images.contains(&"start-app-slot-1-icon".to_owned()),
            "miss applies Flame fallback now, never retains old icon"
        );
    }
}
