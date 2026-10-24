//! Async shell icon loader: one worker thread owns IconResolver.
//! Main thread never resolves synchronously; it try_sends requests and
//! drains completed rasters per tick. No X calls happen in the worker.

use std::borrow::Cow;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};

use flamewm_integrations_linux::icon_service::ThemeRebuilder;
use flamewm_integrations_linux::icons::{IconResolver, IconSize, Rgb8Raster};

/// Swaps in a rebuilt lookup index when the theme directories changed and
/// notifies the resolver's generation-keyed caches. Returns true on swap.
/// The watch check and any full rebuild run on `rebuilder`'s thread: this
/// call only queues a check (if none is in flight) and applies a finished
/// result, so the loader worker never blocks on index build I/O.
fn refresh_icon_index(resolver: &mut IconResolver, rebuilder: &ThemeRebuilder) -> bool {
    rebuilder.request(resolver.shared_index().clone());
    let Some(next) = rebuilder.take() else {
        return false;
    };
    let generation = next.generation;
    resolver.set_shared_index(next);
    resolver.notify_theme_changed(generation);
    true
}

/// Resolves one shell icon raster: the requested name, then the generic
/// application role, then the bundled theme-independent glyph. Brand rasters
/// are never returned, and a miss is never cached as final by the caller.
fn resolve_icon_raster(
    resolver: &mut IconResolver,
    name: &str,
    size: IconSize,
) -> Option<Rgb8Raster> {
    let accept = |raster: &Rgb8Raster| !is_brand_fallback(raster);
    icon_name_candidates(name)
        .iter()
        .find_map(|candidate| {
            resolver
                .prepare_name(candidate.as_str(), size)
                .ok()
                .filter(accept)
        })
        .or_else(|| {
            resolver
                .prepare_name(GENERIC_APP_ICON, size)
                .ok()
                .filter(accept)
        })
        .or_else(|| {
            resolver
                .prepare_generic_application(size)
                .ok()
                .filter(accept)
        })
}

/// Lookup names for one task or Start name, most specific first: the raw
/// name, its lowercase, then the segment after the last `.` (WM_CLASS
/// `mini.xterm` yields `xterm`), lowercased. Empty and duplicate names drop.
pub fn icon_name_candidates(name: &str) -> Vec<String> {
    let raw = name.trim();
    let class_segment = raw.rsplit('.').next().unwrap_or(raw);
    let mut out: Vec<String> = Vec::with_capacity(4);
    for candidate in [
        Cow::Borrowed(raw),
        lowercase_if_needed(raw),
        Cow::Borrowed(class_segment),
        lowercase_if_needed(class_segment),
    ] {
        if !candidate.is_empty() && !out.iter().any(|seen| seen == candidate.as_ref()) {
            out.push(candidate.into_owned());
        }
    }
    out
}

/// ASCII-lowercases `s`, allocating only when it contains an uppercase byte.
fn lowercase_if_needed(s: &str) -> Cow<'_, str> {
    if s.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(s.to_ascii_lowercase())
    } else {
        Cow::Borrowed(s)
    }
}

/// Generic application glyph used when an application name misses. Brand
/// artwork is still rejected by `is_brand_fallback`.
const GENERIC_APP_ICON: &str = "application-x-executable";
/// Bound on visible requests waiting for worker queue capacity.
const RETRY_CAP: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconTarget {
    StartSlot(u8),
    TaskSlot(u8),
    LauncherSlot(u8),
}

#[derive(Debug, Clone)]
pub struct IconRequest {
    pub target: IconTarget,
    pub generation: u64,
    pub name: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependentSurface {
    Panel,
    Start,
    Launcher,
}

#[derive(Debug)]
pub struct IconResult {
    pub target: IconTarget,
    pub generation: u64,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub raster: Option<Rgb8Raster>,
}

/// Work items for the loader worker. Theme changes ride the same bounded
/// queue so they apply in order with the requests that follow them.
#[derive(Debug, Clone)]
pub enum WorkerMsg {
    Icon(IconRequest),
    Theme(String),
}

pub struct IconLoader {
    req_tx: SyncSender<WorkerMsg>,
    res_rx: Receiver<IconResult>,
    wake_reader: UnixStream,
    generation: u64,
    pending: std::collections::HashSet<(IconTarget, String, u32, u32)>,
    pub queue_full_drops: u64,
    pub stale_drops: u64,
    pub results_applied: u64,
    pub high_enqueued: u64,
    pub low_enqueued: u64,
    pub coalesced: u64,
    pub dropped_low: u64,
    /// Visible requests refused by a full worker queue; their keys stay
    /// pending and are re-issued by `flush_retries` on the next drain.
    retry: Vec<IconRequest>,
    /// Current Flame `iconTheme` preference; `set_theme` no-ops when unchanged.
    theme: String,
    /// Theme change refused by a full queue; re-sent by `flush_retries`.
    theme_outbox: Option<String>,
}

impl IconLoader {
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn spawn(packaged_root: std::path::PathBuf, theme: String) -> Self {
        let (req_tx, req_rx) = sync_channel::<WorkerMsg>(16);
        let initial_theme = theme.clone();
        let (res_tx, res_rx) = sync_channel::<IconResult>(16);
        let (wake_reader, mut wake_writer) = UnixStream::pair().expect("icon loader wake pair");
        wake_reader
            .set_nonblocking(true)
            .expect("wake reader nonblocking");
        std::thread::Builder::new()
            .name("shell-icon-loader".to_owned())
            .spawn(move || {
                let mut resolver = IconResolver::from_environment_with_theme(
                    packaged_root,
                    0,
                    Some(&initial_theme),
                );
                let rebuilder = ThemeRebuilder::new();
                while let Ok(msg) = req_rx.recv() {
                    let req = match msg {
                        WorkerMsg::Icon(req) => req,
                        // Off the UI thread: the index scan runs here.
                        WorkerMsg::Theme(name) => {
                            resolver.set_preferred_theme(Some(&name));
                            continue;
                        }
                    };
                    // Throttled (5s) mtime check runs off this thread; a finished
                    // rebuilt index is applied here and advances the resolver's
                    // theme generation when themes change.
                    refresh_icon_index(&mut resolver, &rebuilder);
                    // Generic app miss must never rasterize brand artwork:
                    // no empty-name retry (it resolves to the brand
                    // fallback) and any brand fallback raster is dropped to
                    // None. Miss -> caller keeps last valid or the
                    // transparent empty placeholder. Applies to every
                    // request type/role uniformly.
                    let size = IconSize::new(req.width, req.height);
                    let raster = resolve_icon_raster(&mut resolver, &req.name, size);
                    let res = IconResult {
                        target: req.target,
                        generation: req.generation,
                        name: req.name,
                        width: req.width,
                        height: req.height,
                        raster,
                    };
                    // Blocking send: a dropped result left its key pending
                    // forever. Only a closed receiver ends the worker.
                    if res_tx.send(res).is_err() {
                        break;
                    }
                    let _ = wake_writer.write_all(&[1]);
                }
            })
            .expect("spawn icon loader worker");
        Self {
            req_tx,
            res_rx,
            wake_reader,
            generation: 1,
            pending: std::collections::HashSet::new(),
            queue_full_drops: 0,
            stale_drops: 0,
            results_applied: 0,
            high_enqueued: 0,
            low_enqueued: 0,
            coalesced: 0,
            dropped_low: 0,
            retry: Vec::new(),
            theme,
            theme_outbox: None,
        }
    }

    /// Live `iconTheme` change. Starts a new icon generation (old results
    /// are dropped as stale, in-flight keys are forgotten so Start/taskbar
    /// re-request under the new theme). The worker rebuilds its index in
    /// queue order. No-op when the preference is unchanged.
    /// Returns `true` when the preference changed.
    pub fn set_theme(&mut self, theme: &str) -> bool {
        if theme == self.theme {
            return false;
        }
        self.theme = theme.to_owned();
        self.bump_generation();
        self.pending.clear();
        self.retry.clear();
        self.theme_outbox = Some(theme.to_owned());
        self.flush_theme();
        true
    }

    fn flush_theme(&mut self) {
        let Some(theme) = self.theme_outbox.take() else {
            return;
        };
        match self.req_tx.try_send(WorkerMsg::Theme(theme)) {
            Ok(()) => {}
            Err(TrySendError::Full(WorkerMsg::Theme(theme))) => self.theme_outbox = Some(theme),
            Err(_) => {}
        }
    }

    pub fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1).max(1);
    }

    /// Non-blocking request; queue-full and already-pending (target +
    /// icon key) requests are dropped and counted. Pending dedupe keeps one
    /// worker resolve per key; the result drain caches by key and marks only
    /// dependent surfaces.
    pub fn request(&mut self, target: IconTarget, name: &str, width: u32, height: u32) {
        self.high_enqueued += 1;
        let key = (target, name.to_owned(), width, height);
        if self.pending.contains(&key) {
            return;
        }
        self.pending.insert(key);
        flamewm_integrations_linux::icon_service::note_icon_request();
        let req = IconRequest {
            target,
            generation: self.generation,
            name: name.to_owned(),
            width,
            height,
        };
        self.submit(req);
    }

    fn submit(&mut self, req: IconRequest) {
        match self.req_tx.try_send(WorkerMsg::Icon(req)) {
            Ok(()) => {}
            Err(TrySendError::Full(WorkerMsg::Icon(returned))) => {
                self.queue_full_drops += 1;
                if self.retry.len() < RETRY_CAP {
                    // Key stays pending so duplicate requests coalesce.
                    self.retry.push(returned);
                } else {
                    self.pending.remove(&pending_key(&returned));
                }
            }
            Err(TrySendError::Disconnected(WorkerMsg::Icon(returned))) => {
                // Worker is gone: clear the key so it is never stranded.
                self.queue_full_drops += 1;
                self.pending.remove(&pending_key(&returned));
            }
            Err(_) => {}
        }
    }

    /// Re-issues refused requests. Stale-generation entries are discarded
    /// and their keys released.
    fn flush_retries(&mut self) {
        self.flush_theme();
        for req in std::mem::take(&mut self.retry) {
            if req.generation != self.generation {
                self.pending.remove(&pending_key(&req));
                continue;
            }
            match self.req_tx.try_send(WorkerMsg::Icon(req)) {
                Ok(()) => {}
                Err(TrySendError::Full(WorkerMsg::Icon(returned))) => self.retry.push(returned),
                Err(TrySendError::Disconnected(WorkerMsg::Icon(returned))) => {
                    self.pending.remove(&pending_key(&returned));
                }
                Err(_) => {}
            }
        }
    }

    /// Low-priority prewarm shim (counts only; same queue, no behavior change).
    pub fn request_low(&mut self, target: IconTarget, name: &str, width: u32, height: u32) {
        self.low_enqueued += 1;
        self.request(target, name, width, height);
    }

    /// Drain ready results; stale generations are dropped and counted.
    /// Ready keys leave the pending set so a later view change can re-queue.
    pub fn drain_ready(&mut self) -> Vec<IconResult> {
        let mut out = Vec::new();
        self.wake_drain();
        self.flush_retries();
        while let Ok(res) = self.res_rx.try_recv() {
            self.pending
                .remove(&(res.target, res.name.clone(), res.width, res.height));
            if res.generation != self.generation {
                self.stale_drops += 1;
                continue;
            }
            flamewm_integrations_linux::icon_service::observe_icon_raster(res.raster.is_some());
            out.push(res);
        }
        self.results_applied += out.len() as u64;
        out
    }

    #[must_use]
    pub fn wake_fd(&self) -> std::os::unix::io::RawFd {
        use std::os::unix::io::AsRawFd;
        self.wake_reader.as_raw_fd()
    }

    pub fn wake_drain(&self) {
        use std::io::Read;
        let _ = self.wake_reader.set_nonblocking(true);
        let mut buf = [0u8; 64];
        loop {
            match (&self.wake_reader).read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }

    /// Dependent surfaces for one icon result: Start slots repaint the
    /// submenu, task slots repaint the panel. Callers reproject only these.
    #[must_use]
    pub fn dependent_surface(target: IconTarget) -> DependentSurface {
        match target {
            IconTarget::StartSlot(_) => DependentSurface::Start,
            IconTarget::LauncherSlot(_) => DependentSurface::Launcher,
            IconTarget::TaskSlot(_) => DependentSurface::Panel,
        }
    }
}

fn pending_key(req: &IconRequest) -> (IconTarget, String, u32, u32) {
    (req.target, req.name.clone(), req.width, req.height)
}

/// True when a worker raster is brand artwork that a generic app miss
/// must never show. Covers the packaged brand fallback path and the
/// explicit brand names; other packaged/system hits pass through.
fn is_brand_fallback(raster: &Rgb8Raster) -> bool {
    raster.source.ends_with("flamewm-icon.svg") || raster.source.ends_with("flamewm-start.svg")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root")
            .to_path_buf()
    }

    #[test]
    fn icon_name_candidates_try_raw_then_lowercase_then_class_segment() {
        assert_eq!(
            icon_name_candidates("mini.xterm"),
            vec!["mini.xterm".to_owned(), "xterm".to_owned()]
        );
        assert_eq!(
            icon_name_candidates("XTerm"),
            vec!["XTerm".to_owned(), "xterm".to_owned()]
        );
        assert_eq!(icon_name_candidates("  "), Vec::<String>::new());
    }

    #[test]
    fn wm_class_segment_resolves_to_installed_theme_icon() {
        let themes = std::env::temp_dir().join(format!("flamewm-wmclass-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&themes);
        let icons = themes.join("icons/breeze/16x16/apps");
        std::fs::create_dir_all(&icons).expect("theme dir");
        std::fs::write(
            themes.join("icons/breeze/index.theme"),
            "[Icon Theme]\nDirectories=16x16/apps\n\n[16x16/apps]\nSize=16\nType=Fixed\n",
        )
        .expect("index.theme");
        let mut bytes = b"P6\n1 1\n255\n".to_vec();
        bytes.extend_from_slice(&[10, 200, 10]);
        std::fs::write(icons.join("xterm.ppm"), bytes).expect("xterm icon");
        let mut resolver = IconResolver::new(
            workspace_root(),
            vec!["Breeze".to_owned()],
            vec![themes.clone()],
            0,
        );
        let raster = resolve_icon_raster(&mut resolver, "mini.xterm", IconSize::new(16, 16))
            .expect("WM_CLASS segment must resolve");
        let _ = std::fs::remove_dir_all(&themes);
        assert!(
            raster.source.ends_with("xterm.ppm"),
            "got {}",
            raster.source
        );
    }

    #[test]
    fn task_icon_miss_with_empty_theme_still_yields_bundled_glyph() {
        let themes =
            std::env::temp_dir().join(format!("flamewm-empty-theme-{}", std::process::id()));
        std::fs::create_dir_all(&themes).expect("empty theme dir");
        let mut resolver = IconResolver::new(workspace_root(), Vec::new(), vec![themes], 0);
        let size = IconSize::new(25, 25);
        for _ in 0..2 {
            let raster = resolve_icon_raster(&mut resolver, "mini.xterm", size)
                .expect("task miss must resolve to the bundled glyph, not stay empty");
            assert!(
                raster.source.ends_with("application-x-executable.svg"),
                "bundled generic glyph expected, got {}",
                raster.source
            );
        }
    }

    #[test]
    fn refresh_icon_index_advances_generation_when_watch_mtime_is_stale() {
        use flamewm_integrations_linux::IconLookupIndex;
        use std::collections::HashMap;
        let root = std::env::temp_dir().join(format!(
            "flamewm-loader-refresh-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("watch root");
        let mut index = IconLookupIndex::new(
            vec![root.clone()],
            Vec::new(),
            HashMap::new(),
            HashMap::new(),
            3,
        );
        index.watch_mtime = Some(std::time::UNIX_EPOCH);
        let mut resolver = IconResolver::from_shared_index(
            root.clone(),
            Vec::new(),
            Vec::new(),
            3,
            std::sync::Arc::new(index),
        );
        let rebuilder = ThemeRebuilder::new();
        // The check runs off-thread: poll (bounded) until the swap lands.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut first = false;
        while !first && std::time::Instant::now() < deadline {
            first = refresh_icon_index(&mut resolver, &rebuilder);
            if !first {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        let second = refresh_icon_index(&mut resolver, &rebuilder);
        let _ = std::fs::remove_dir_all(&root);
        assert!(first, "stale watch mtime must swap in a rebuilt index");
        assert_eq!(resolver.theme_generation(), 4);
        assert_eq!(resolver.shared_index().generation, 4);
        assert!(!second, "second refresh inside 5s must be throttled");
    }

    #[test]
    fn request_channel_is_nonblocking_when_full() {
        let (tx, _rx) = sync_channel::<WorkerMsg>(1);
        tx.try_send(WorkerMsg::Icon(IconRequest {
            target: IconTarget::StartSlot(0),
            generation: 1,
            name: "a".to_owned(),
            width: 20,
            height: 20,
        }))
        .expect("first send fits");
        assert!(tx
            .try_send(WorkerMsg::Icon(IconRequest {
                target: IconTarget::StartSlot(1),
                generation: 1,
                name: "b".to_owned(),
                width: 20,
                height: 20,
            }))
            .is_err());
    }

    #[test]
    fn stale_generations_are_dropped() {
        let (req_tx, _req_rx) = sync_channel::<WorkerMsg>(16);
        let (res_tx, res_rx) = sync_channel::<IconResult>(16);
        let mut loader = loader_with_channels(req_tx, res_rx);
        loader.generation = 2;
        res_tx
            .try_send(IconResult {
                target: IconTarget::TaskSlot(0),
                generation: 1,
                name: "old".to_owned(),
                width: 25,
                height: 25,
                raster: None,
            })
            .expect("queue stale");
        res_tx
            .try_send(IconResult {
                target: IconTarget::TaskSlot(1),
                generation: 2,
                name: "new".to_owned(),
                width: 25,
                height: 25,
                raster: None,
            })
            .expect("queue fresh");
        let ready = loader.drain_ready();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].name, "new");
        assert_eq!(loader.stale_drops, 1);
    }

    #[test]
    fn queue_full_drop_counts_without_blocking() {
        let (tx, _rx) = sync_channel::<WorkerMsg>(1);
        let (_res_tx, res_rx) = sync_channel::<IconResult>(16);
        let mut loader = loader_with_channels(tx.clone(), res_rx);
        tx.try_send(WorkerMsg::Icon(IconRequest {
            target: IconTarget::StartSlot(0),
            generation: 1,
            name: "fill".to_owned(),
            width: 20,
            height: 20,
        }))
        .expect("fill");
        loader.request(IconTarget::StartSlot(1), "y", 20, 20);
        assert_eq!(loader.queue_full_drops, 1);
    }

    #[test]
    fn brand_fallback_sources_are_rejected() {
        use flamewm_integrations_linux::icons::IconOrigin;
        for source in [
            "packaged/assets/web/flamewm-icon.svg",
            "x/assets/branding/flamewm-start.svg",
        ] {
            assert!(is_brand_fallback(&Rgb8Raster {
                source: source.to_owned(),
                width: 20,
                height: 20,
                pixels: vec![1; 20 * 20 * 4],
                origin: IconOrigin::PackagedFallback,
                fallback_reason: None,
            }));
        }
        assert!(!is_brand_fallback(&Rgb8Raster {
            source: "packaged/assets/web/breeze/folder.svg".to_owned(),
            width: 20,
            height: 20,
            pixels: vec![1; 20 * 20 * 4],
            origin: IconOrigin::PackagedBreeze,
            fallback_reason: None,
        }));
    }

    fn loader_with_channels(
        req_tx: SyncSender<WorkerMsg>,
        res_rx: Receiver<IconResult>,
    ) -> IconLoader {
        let (wake_reader, _w) = UnixStream::pair().expect("wake pair");
        IconLoader {
            req_tx,
            res_rx,
            wake_reader,
            generation: 1,
            pending: std::collections::HashSet::new(),
            queue_full_drops: 0,
            stale_drops: 0,
            results_applied: 0,
            high_enqueued: 0,
            low_enqueued: 0,
            coalesced: 0,
            dropped_low: 0,
            retry: Vec::new(),
            theme: String::new(),
            theme_outbox: None,
        }
    }

    #[test]
    fn undeliverable_request_clears_its_pending_key() {
        let (tx, rx) = sync_channel::<WorkerMsg>(4);
        drop(rx);
        let (_res_tx, res_rx) = sync_channel::<IconResult>(4);
        let mut loader = loader_with_channels(tx, res_rx);
        loader.request(IconTarget::TaskSlot(3), "gone", 24, 24);
        assert!(loader.pending.is_empty());
        assert!(loader.retry.is_empty());
    }

    #[test]
    fn drained_result_clears_pending_key() {
        let (tx, _rx) = sync_channel::<WorkerMsg>(4);
        let (res_tx, res_rx) = sync_channel::<IconResult>(4);
        let mut loader = loader_with_channels(tx, res_rx);
        loader.request(IconTarget::TaskSlot(0), "firefox", 24, 24);
        assert_eq!(loader.pending.len(), 1);
        res_tx
            .try_send(IconResult {
                target: IconTarget::TaskSlot(0),
                generation: 1,
                name: "firefox".to_owned(),
                width: 24,
                height: 24,
                raster: None,
            })
            .expect("queue result");
        assert_eq!(loader.drain_ready().len(), 1);
        assert!(loader.pending.is_empty());
    }

    #[test]
    fn refused_visible_request_is_retried_after_capacity_frees() {
        let (tx, rx) = sync_channel::<WorkerMsg>(1);
        let (_res_tx, res_rx) = sync_channel::<IconResult>(4);
        let mut loader = loader_with_channels(tx.clone(), res_rx);
        tx.try_send(WorkerMsg::Icon(IconRequest {
            target: IconTarget::StartSlot(0),
            generation: 1,
            name: "fill".to_owned(),
            width: 20,
            height: 20,
        }))
        .expect("fill");
        loader.request(IconTarget::StartSlot(1), "retry-me", 20, 20);
        assert_eq!(loader.retry.len(), 1);
        assert_eq!(loader.pending.len(), 1);
        assert!(rx.try_recv().is_ok());
        let _ = loader.drain_ready();
        assert!(loader.retry.is_empty());
        let WorkerMsg::Icon(reissued) = rx.try_recv().expect("retried request queued") else {
            panic!("expected icon request");
        };
        assert_eq!(reissued.name, "retry-me");
    }

    #[test]
    fn stale_retry_is_discarded_and_key_released() {
        let (tx, rx) = sync_channel::<WorkerMsg>(1);
        let (_res_tx, res_rx) = sync_channel::<IconResult>(4);
        let mut loader = loader_with_channels(tx.clone(), res_rx);
        tx.try_send(WorkerMsg::Icon(IconRequest {
            target: IconTarget::StartSlot(0),
            generation: 1,
            name: "fill".to_owned(),
            width: 20,
            height: 20,
        }))
        .expect("fill");
        loader.request(IconTarget::StartSlot(1), "old", 20, 20);
        loader.bump_generation();
        assert!(rx.try_recv().is_ok());
        let _ = loader.drain_ready();
        assert!(loader.retry.is_empty());
        assert!(loader.pending.is_empty());
    }

    #[test]
    fn generic_application_icon_is_not_brand_artwork() {
        assert_ne!(GENERIC_APP_ICON, "flamewm-icon");
        assert!(!is_brand_fallback(&Rgb8Raster {
            source: format!("packaged/assets/web/breeze/{GENERIC_APP_ICON}.svg"),
            width: 20,
            height: 20,
            pixels: vec![1; 20 * 20 * 4],
            origin: flamewm_integrations_linux::icons::IconOrigin::PackagedBreeze,
            fallback_reason: None,
        }));
    }

    #[test]
    fn set_theme_is_noop_when_unchanged_and_sends_one_change_per_switch() {
        let (tx, rx) = sync_channel::<WorkerMsg>(4);
        let (_res_tx, res_rx) = sync_channel::<IconResult>(4);
        let mut loader = loader_with_channels(tx, res_rx);
        loader.theme = "ThemeA".to_owned();
        let generation = loader.generation();
        assert!(!loader.set_theme("ThemeA"));
        assert!(
            rx.try_recv().is_err(),
            "unchanged theme must not reach worker"
        );
        assert_eq!(loader.generation(), generation);

        loader.request(IconTarget::StartSlot(0), "old", 20, 20);
        assert!(loader.set_theme("ThemeB"));
        assert_eq!(loader.generation(), generation.wrapping_add(1).max(1));
        assert!(loader.pending.is_empty(), "in-flight keys are forgotten");
        let WorkerMsg::Icon(_) = rx.try_recv().expect("old request queued first") else {
            panic!("expected icon request first");
        };
        let WorkerMsg::Theme(name) = rx.try_recv().expect("theme change queued") else {
            panic!("expected theme message");
        };
        assert_eq!(name, "ThemeB");
        assert!(
            rx.try_recv().is_err(),
            "exactly one theme message per switch"
        );
    }

    #[test]
    fn theme_switch_rerequests_old_key_and_rejects_stale_reply() {
        let (tx, rx) = sync_channel::<WorkerMsg>(8);
        let (res_tx, res_rx) = sync_channel::<IconResult>(8);
        let mut loader = loader_with_channels(tx, res_rx);
        loader.theme = "ThemeA".to_owned();
        let old_generation = loader.generation();

        loader.request(IconTarget::TaskSlot(0), "app", 24, 24);
        assert!(loader.set_theme("ThemeB"));
        let new_generation = loader.generation();
        assert_ne!(old_generation, new_generation);

        // The key is forgotten by the theme switch, so it re-enqueues.
        loader.request(IconTarget::TaskSlot(0), "app", 24, 24);
        let mut icon_generations = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            if let WorkerMsg::Icon(req) = msg {
                icon_generations.push(req.generation);
            }
        }
        assert_eq!(icon_generations, vec![old_generation, new_generation]);

        // Old-generation reply for the same key is rejected; new one is accepted.
        res_tx
            .try_send(IconResult {
                target: IconTarget::TaskSlot(0),
                generation: old_generation,
                name: "app".to_owned(),
                width: 24,
                height: 24,
                raster: None,
            })
            .expect("queue stale");
        assert!(loader.drain_ready().is_empty());
        assert_eq!(loader.stale_drops, 1);

        res_tx
            .try_send(IconResult {
                target: IconTarget::TaskSlot(0),
                generation: new_generation,
                name: "app".to_owned(),
                width: 24,
                height: 24,
                raster: None,
            })
            .expect("queue fresh");
        let ready = loader.drain_ready();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].generation, new_generation);
        assert_eq!(loader.stale_drops, 1);
    }
}
