//! Async shell icon loader: one worker thread owns IconResolver.
//! Main thread never resolves synchronously; it try_sends requests and
//! drains completed rasters per tick. No X calls happen in the worker.

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

/// Generic application glyph used when an application name misses. Brand
/// artwork is still rejected by `is_brand_fallback`.
const GENERIC_APP_ICON: &str = "application-x-executable";
/// Bound on visible requests waiting for worker queue capacity.
const RETRY_CAP: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconTarget {
    StartSlot(u8),
    TaskSlot(u8),
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

pub struct IconLoader {
    req_tx: SyncSender<IconRequest>,
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
}

impl IconLoader {
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn spawn(packaged_root: std::path::PathBuf) -> Self {
        let (req_tx, req_rx) = sync_channel::<IconRequest>(16);
        let (res_tx, res_rx) = sync_channel::<IconResult>(16);
        let (wake_reader, mut wake_writer) = UnixStream::pair().expect("icon loader wake pair");
        wake_reader
            .set_nonblocking(true)
            .expect("wake reader nonblocking");
        std::thread::Builder::new()
            .name("shell-icon-loader".to_owned())
            .spawn(move || {
                let mut resolver = IconResolver::from_environment(packaged_root, 0);
                let rebuilder = ThemeRebuilder::new();
                while let Ok(req) = req_rx.recv() {
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
                    let accept = |raster: &Rgb8Raster| !is_brand_fallback(raster);
                    let raster = resolver
                        .prepare_name(&req.name, size)
                        .ok()
                        .filter(accept)
                        .or_else(|| {
                            // Miss -> generic application role, never brand.
                            resolver
                                .prepare_name(GENERIC_APP_ICON, size)
                                .ok()
                                .filter(accept)
                        });
                    let res = IconResult {
                        target: req.target,
                        generation: req.generation,
                        name: req.name.clone(),
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
        match self.req_tx.try_send(req) {
            Ok(()) => {}
            Err(TrySendError::Full(returned)) => {
                self.queue_full_drops += 1;
                if self.retry.len() < RETRY_CAP {
                    // Key stays pending so duplicate requests coalesce.
                    self.retry.push(returned);
                } else {
                    self.pending.remove(&pending_key(&returned));
                }
            }
            Err(TrySendError::Disconnected(returned)) => {
                // Worker is gone: clear the key so it is never stranded.
                self.queue_full_drops += 1;
                self.pending.remove(&pending_key(&returned));
            }
        }
    }

    /// Re-issues refused requests. Stale-generation entries are discarded
    /// and their keys released.
    fn flush_retries(&mut self) {
        for req in std::mem::take(&mut self.retry) {
            if req.generation != self.generation {
                self.pending.remove(&pending_key(&req));
                continue;
            }
            match self.req_tx.try_send(req) {
                Ok(()) => {}
                Err(TrySendError::Full(returned)) => self.retry.push(returned),
                Err(TrySendError::Disconnected(returned)) => {
                    self.pending.remove(&pending_key(&returned));
                }
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
        let _ = self.wake_reader.set_nonblocking(true);
        let mut wake_buf = [0u8; 64];
        use std::io::Read;
        let _ = (&self.wake_reader).read(&mut wake_buf);
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
        let (tx, _rx) = sync_channel::<IconRequest>(1);
        tx.try_send(IconRequest {
            target: IconTarget::StartSlot(0),
            generation: 1,
            name: "a".to_owned(),
            width: 20,
            height: 20,
        })
        .expect("first send fits");
        assert!(tx
            .try_send(IconRequest {
                target: IconTarget::StartSlot(1),
                generation: 1,
                name: "b".to_owned(),
                width: 20,
                height: 20,
            })
            .is_err());
    }

    #[test]
    fn stale_generations_are_dropped() {
        let (req_tx, req_rx) = sync_channel::<IconRequest>(16);
        let (res_tx, res_rx) = sync_channel::<IconResult>(16);
        let (wake_reader, _wake_writer) = UnixStream::pair().expect("wake pair");
        let mut loader = IconLoader {
            req_tx,
            res_rx,
            wake_reader,
            generation: 2,
            pending: std::collections::HashSet::new(),
            queue_full_drops: 0,
            stale_drops: 0,
            results_applied: 0,
            high_enqueued: 0,
            low_enqueued: 0,
            coalesced: 0,
            dropped_low: 0,
            retry: Vec::new(),
        };
        let _ = req_rx;
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
        let (tx, rx) = sync_channel::<IconRequest>(1);
        let (res_tx, res_rx) = sync_channel::<IconResult>(16);
        let (wake_reader, _w) = UnixStream::pair().expect("wake pair");
        let _ = rx;
        let _ = res_tx;
        let mut loader = IconLoader {
            req_tx: tx.clone(),
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
        };
        tx.try_send(IconRequest {
            target: IconTarget::StartSlot(0),
            generation: 1,
            name: "fill".to_owned(),
            width: 20,
            height: 20,
        })
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
        req_tx: SyncSender<IconRequest>,
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
        }
    }

    #[test]
    fn undeliverable_request_clears_its_pending_key() {
        let (tx, rx) = sync_channel::<IconRequest>(4);
        drop(rx);
        let (_res_tx, res_rx) = sync_channel::<IconResult>(4);
        let mut loader = loader_with_channels(tx, res_rx);
        loader.request(IconTarget::TaskSlot(3), "gone", 24, 24);
        assert!(loader.pending.is_empty());
        assert!(loader.retry.is_empty());
    }

    #[test]
    fn drained_result_clears_pending_key() {
        let (tx, _rx) = sync_channel::<IconRequest>(4);
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
        let (tx, rx) = sync_channel::<IconRequest>(1);
        let (_res_tx, res_rx) = sync_channel::<IconResult>(4);
        let mut loader = loader_with_channels(tx.clone(), res_rx);
        tx.try_send(IconRequest {
            target: IconTarget::StartSlot(0),
            generation: 1,
            name: "fill".to_owned(),
            width: 20,
            height: 20,
        })
        .expect("fill");
        loader.request(IconTarget::StartSlot(1), "retry-me", 20, 20);
        assert_eq!(loader.retry.len(), 1);
        assert_eq!(loader.pending.len(), 1);
        assert!(rx.try_recv().is_ok());
        let _ = loader.drain_ready();
        assert!(loader.retry.is_empty());
        let reissued = rx.try_recv().expect("retried request queued");
        assert_eq!(reissued.name, "retry-me");
    }

    #[test]
    fn stale_retry_is_discarded_and_key_released() {
        let (tx, rx) = sync_channel::<IconRequest>(1);
        let (_res_tx, res_rx) = sync_channel::<IconResult>(4);
        let mut loader = loader_with_channels(tx.clone(), res_rx);
        tx.try_send(IconRequest {
            target: IconTarget::StartSlot(0),
            generation: 1,
            name: "fill".to_owned(),
            width: 20,
            height: 20,
        })
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
}
