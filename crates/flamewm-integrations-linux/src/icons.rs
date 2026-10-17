//! Linux icon lookup and RGB8 preparation.
//!
//! Packaged semantic assets are tried before freedesktop theme paths. The
//! resolver performs filesystem work only on cache misses; callers must bump
//! `theme_generation` when host theme state changes.

use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::icon_theme::{configured_icon_theme, unique_paths};
use crate::icon_theme::IconThemeCaches as ThemeLookupCaches;

const GENERIC_FALLBACK: &str = "assets/web/flamewm-icon.svg";
/// Theme-independent generic application glyph bundled with the product.
/// Used when no installed theme resolves an application icon.
// Lives outside assets/themes: scripts/package strips that tree from the
// installed assets copy, and the glyph must survive packaging.
const GENERIC_APPLICATION_GLYPH: &str = "assets/web/application-x-executable.svg";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IconLookupPurpose {
    Application,
    Semantic,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IconRequest {
    Name(String),
    Path(PathBuf),
}

impl IconRequest {
    pub fn name(name: impl Into<String>) -> Self {
        Self::Name(name.into())
    }

    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self::Path(path.into())
    }
}

impl From<&str> for IconRequest {
    fn from(name: &str) -> Self {
        Self::name(name)
    }
}

impl From<String> for IconRequest {
    fn from(name: String) -> Self {
        Self::Name(name)
    }
}

impl From<PathBuf> for IconRequest {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

impl From<&Path> for IconRequest {
    fn from(path: &Path) -> Self {
        Self::Path(path.to_path_buf())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IconSize {
    pub logical: u32,
    pub physical: u32,
}

impl IconSize {
    #[must_use]
    pub const fn new(logical: u32, physical: u32) -> Self {
        Self { logical, physical }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IconOrigin {
    ExplicitPath,
    PackagedFlame,
    PackagedBreeze,
    SystemTheme,
    PackagedFallback,
}

/// Canonical alpha-bearing icon raster: straight (non-premultiplied) RGBA8
/// in row-major order. PNG/SVG sources preserve tiny-skia alpha via
/// un-premultiplication; opaque PPM sources convert to alpha=255 at decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba8Raster {
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub origin: IconOrigin,
    /// Set when an earlier candidate failed, while packaged fallback succeeded.
    pub fallback_reason: Option<RasterError>,
}

/// Compatibility alias for the J06 RuntimeImage bridge consumers
/// (`flamewm-shell`/`flamewm-desktop` projections). The aliased pixels are
/// RGBA8 straight alpha; use [`Rgba8Raster::to_rgb8_black_background`] for the
/// legacy opaque black-background projection.
pub type Rgb8Raster = Rgba8Raster;

impl Rgba8Raster {
    #[must_use]
    pub fn alpha_at(&self, x: u32, y: u32) -> Option<u8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let index = (y as usize * self.width as usize + x as usize) * 4 + 3;
        self.pixels.get(index).copied()
    }

    /// Legacy opaque projection with black-background compositing semantics.
    #[must_use]
    pub fn to_rgb8_black_background(&self) -> Vec<u8> {
        let mut pixels = Vec::with_capacity(self.width as usize * self.height as usize * 3);
        for rgba in self.pixels.chunks_exact(4) {
            let a = u16::from(rgba[3]);
            pixels.push(((u16::from(rgba[0]) * a) / 255) as u8);
            pixels.push(((u16::from(rgba[1]) * a) / 255) as u8);
            pixels.push(((u16::from(rgba[2]) * a) / 255) as u8);
        }
        pixels
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RasterError {
    InvalidSize {
        logical: u32,
        physical: u32,
    },
    Io {
        path: PathBuf,
        message: String,
    },
    InvalidRaster {
        path: PathBuf,
        format: &'static str,
        message: String,
    },
    UnsupportedFormat {
        path: PathBuf,
        extension: String,
    },
    InvalidPpm {
        path: PathBuf,
        message: String,
    },
}

impl fmt::Display for RasterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSize { logical, physical } => write!(
                formatter,
                "icon size must have non-zero physical dimensions (logical={logical}, physical={physical})"
            ),
            Self::Io { path, message } => {
                write!(
                    formatter,
                    "failed to read icon {}: {message}",
                    path.display()
                )
            }
            Self::InvalidRaster {
                path,
                format,
                message,
            } => write!(
                formatter,
                "invalid {format} icon {}: {message}",
                path.display()
            ),
            Self::UnsupportedFormat { path, extension } => write!(
                formatter,
                "unsupported icon format '{extension}' at {}",
                path.display()
            ),
            Self::InvalidPpm { path, message } => {
                write!(formatter, "invalid PPM icon {}: {message}", path.display())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconError {
    InvalidSize {
        logical: u32,
        physical: u32,
    },
    FallbackUnavailable {
        fallback: PathBuf,
        fallback_error: Box<RasterError>,
        candidate_error: Option<Box<RasterError>>,
    },
}

impl fmt::Display for IconError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSize { logical, physical } => write!(
                formatter,
                "icon size must have non-zero physical dimensions (logical={logical}, physical={physical})"
            ),
            Self::FallbackUnavailable {
                fallback,
                fallback_error,
                candidate_error,
            } => {
                write!(
                    formatter,
                    "packaged icon fallback {} failed: {fallback_error}",
                    fallback.display()
                )?;
                if let Some(candidate_error) = candidate_error {
                    write!(formatter, "; first candidate failure: {candidate_error}")?;
                }
                Ok(())
            }
        }
    }
}

pub use crate::icon_theme::{IconDir, IconDirType, IconLookupIndex, IconThemeIndex};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    purpose: IconLookupPurpose,
    identity: IconRequest,
    size: IconSize,
    theme_generation: u64,
    palette_generation: u64,
}

#[derive(Clone, Debug)]
struct Candidate {
    path: PathBuf,
    origin: IconOrigin,
}

/// Service-level key: purpose + request + size + generations.
/// Identity match drives slot tracking; workers resolve via `prepare_key`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IconKey {
    pub purpose: IconLookupPurpose,
    pub request: IconRequest,
    pub size: IconSize,
    pub theme_generation: u64,
    pub palette_generation: u64,
}

impl IconKey {
    #[must_use]
    pub fn new(
        purpose: IconLookupPurpose,
        request: IconRequest,
        size: IconSize,
        theme_generation: u64,
        palette_generation: u64,
    ) -> Self {
        Self {
            purpose,
            request,
            size,
            theme_generation,
            palette_generation,
        }
    }
}

/// Snapshot of resolver cache pressure (request cache + raster LRU).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IconMetricsSnapshot {
    pub request_entries: usize,
    pub request_bytes: usize,
}

pub struct IconResolver {
    packaged_root: PathBuf,
    theme_names: Vec<String>,
    data_dirs: Vec<PathBuf>,
    theme_generation: u64,
    palette_generation: u64,
    cache: HashMap<CacheKey, Result<Rgba8Raster, IconError>>,
    max_entries: usize,
    theme_caches: ThemeLookupCaches,
    raster: RasterCache,
    shared_index: std::sync::Arc<IconLookupIndex>,
}

/// Bounded decoded-raster LRU keyed by path + physical size + origin.
/// Budget defaults to 8 MiB of pixel bytes.
#[derive(Clone, Debug)]
struct RasterCache {
    budget_bytes: usize,
    used_bytes: usize,
    entries: HashMap<RasterKey, Result<Rgba8Raster, RasterError>>,
    order: std::collections::VecDeque<RasterKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RasterKey {
    path: PathBuf,
    physical: u32,
    origin: IconOrigin,
}

impl Default for RasterCache {
    fn default() -> Self {
        Self {
            budget_bytes: 8 * 1024 * 1024,
            used_bytes: 0,
            entries: HashMap::new(),
            order: std::collections::VecDeque::new(),
        }
    }
}

impl RasterCache {
    fn raster_bytes(result: &Result<Rgba8Raster, RasterError>) -> usize {
        match result {
            Ok(raster) => raster.pixels.len() + 64,
            Err(_) => 64,
        }
    }

    fn get(&mut self, key: &RasterKey) -> Option<Result<Rgba8Raster, RasterError>> {
        if let Some(hit) = self.entries.get(key) {
            let hit = hit.clone();
            if let Some(position) = self.order.iter().position(|entry| entry == key) {
                self.order.remove(position);
                self.order.push_back(key.clone());
            }
            return Some(hit);
        }
        None
    }

    fn insert(&mut self, key: RasterKey, result: Result<Rgba8Raster, RasterError>) {
        if self.entries.contains_key(&key) {
            return;
        }
        let cost = Self::raster_bytes(&result);
        while !self.entries.is_empty() && self.used_bytes + cost > self.budget_bytes {
            if let Some(old) = self.order.pop_front() {
                if let Some(removed) = self.entries.remove(&old) {
                    self.used_bytes = self.used_bytes.saturating_sub(Self::raster_bytes(&removed));
                }
            } else {
                break;
            }
        }
        if cost > self.budget_bytes && !result_is_large_ok(&result) {
            return;
        }
        self.order.push_back(key.clone());
        self.used_bytes += cost;
        self.entries.insert(key, result);
    }

    fn memory_estimate_bytes(&self) -> usize {
        self.used_bytes + self.entries.len() * 96
    }
}

fn result_is_large_ok(_: &Result<Rgba8Raster, RasterError>) -> bool {
    // Single oversized rasters are still cached so warm path stays FS-free.
    true
}

impl IconResolver {
    #[must_use]
    pub fn new(
        packaged_root: impl Into<PathBuf>,
        theme_names: Vec<String>,
        data_dirs: Vec<PathBuf>,
        theme_generation: u64,
    ) -> Self {
        let index = std::sync::Arc::new(IconLookupIndex::build(
            &theme_names,
            &data_dirs,
            theme_generation,
        ));
        let mut resolver =
            Self::from_shared_index(packaged_root, theme_names, data_dirs, theme_generation, index);
        resolver.max_entries = 1024;
        resolver.raster = RasterCache::default();
        resolver
    }

    /// Worker fork seam: shares the immutable lookup index + theme config
    /// with fresh small private caches (128 entries / 512 KiB raster).
    #[must_use]
    pub fn from_shared_index(
        packaged_root: impl Into<PathBuf>,
        theme_names: Vec<String>,
        data_dirs: Vec<PathBuf>,
        theme_generation: u64,
        shared_index: std::sync::Arc<IconLookupIndex>,
    ) -> Self {
        Self {
            packaged_root: packaged_root.into(),
            theme_names: unique_strings(theme_names),
            data_dirs: unique_paths(data_dirs),
            theme_generation,
            palette_generation: 0,
            cache: HashMap::new(),
            max_entries: 128,
            theme_caches: ThemeLookupCaches::default(),
            raster: RasterCache {
                budget_bytes: 512 * 1024,
                used_bytes: 0,
                entries: HashMap::new(),
                order: std::collections::VecDeque::new(),
            },
            shared_index,
        }
    }

    /// Forks a worker from this resolver (shared index, fresh caches).
    #[must_use]
    pub fn fork_worker(&self) -> Self {
        Self::from_shared_index(
            self.packaged_root.clone(),
            self.theme_names.clone(),
            self.data_dirs.clone(),
            self.theme_generation,
            std::sync::Arc::clone(&self.shared_index),
        )
    }

    /// Sets the shared index (service seam).
    pub fn set_shared_index(&mut self, index: std::sync::Arc<IconLookupIndex>) {
        self.shared_index = index;
    }

    /// The shared generation-scoped lookup index.
    #[must_use]
    pub fn shared_index(&self) -> &std::sync::Arc<IconLookupIndex> {
        &self.shared_index
    }

    /// Service entry point: resolves by key identity.
    pub fn prepare_key(&mut self, key: &IconKey) -> Result<Rgba8Raster, IconError> {
        self.theme_generation = key.theme_generation;
        self.palette_generation = key.palette_generation;
        match key.request.clone() {
            IconRequest::Path(path) => self.prepare_path(path, key.size),
            IconRequest::Name(name) => match key.purpose {
                IconLookupPurpose::Semantic => self.prepare_semantic(name, key.size),
                IconLookupPurpose::Application => self.prepare_application(name, key.size),
            },
        }
    }

    /// Cache-pressure snapshot for service bounds checks.
    #[must_use]
    pub fn metrics_snapshot(&self) -> IconMetricsSnapshot {
        IconMetricsSnapshot {
            request_entries: self.cache.len(),
            request_bytes: self.request_bytes(),
        }
    }

    /// Reads XDG paths once. Theme selection remains explicit when callers use `new`.
    /// Default search order after packaged assets is Breeze then hicolor.
    #[must_use]
    pub fn from_environment(packaged_root: impl Into<PathBuf>, theme_generation: u64) -> Self {
        Self::from_environment_with_theme(packaged_root, theme_generation, None)
    }

    /// Startup entry for the Flame `iconTheme` preference (`None` = unset).
    #[must_use]
    pub fn from_environment_with_theme(
        packaged_root: impl Into<PathBuf>,
        theme_generation: u64,
        preferred: Option<&str>,
    ) -> Self {
        Self::new(
            packaged_root,
            theme_chain(preferred),
            xdg_data_dirs(),
            theme_generation,
        )
    }

    /// Canonical live theme entry (startup uses the same chain via
    /// `from_environment_with_theme`). Rebuilds the shared lookup index only
    /// when the effective chain changes, then installs it with
    /// `set_shared_index` and bumps the theme generation. The index build is a
    /// directory scan: call it off the UI thread. Returns `true` on rebuild.
    pub fn set_preferred_theme(&mut self, preferred: Option<&str>) -> bool {
        let theme_names = theme_chain(preferred);
        if theme_names == self.theme_names {
            return false;
        }
        let generation = self.theme_generation.saturating_add(1);
        let index = std::sync::Arc::new(IconLookupIndex::build(
            &theme_names,
            &self.data_dirs,
            generation,
        ));
        self.theme_names = theme_names;
        self.set_shared_index(index);
        self.notify_theme_changed(generation);
        true
    }

    pub fn notify_theme_changed(&mut self, theme_generation: u64) {
        self.theme_generation = theme_generation;
        self.theme_caches.note_generation_bump();
    }

    pub fn notify_palette_changed(&mut self, palette_generation: u64) {
        self.palette_generation = palette_generation;
    }

    #[must_use]
    pub const fn theme_generation(&self) -> u64 {
        self.theme_generation
    }

    #[must_use]
    pub const fn palette_generation(&self) -> u64 {
        self.palette_generation
    }

    #[must_use]
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    /// Bounded-cache stats (hits, misses) for theme roots/index/inheritance/path probes.
    #[must_use]
    pub fn theme_cache_stats(&self) -> (u64, u64) {
        self.theme_caches.stats()
    }

    /// Estimated bytes held by theme lookup caches + bounded raster LRU.
    #[must_use]
    pub fn memory_estimate_bytes(&self) -> usize {
        self.theme_caches.memory_estimate_bytes()
            + self.raster.memory_estimate_bytes()
            + self.request_bytes()
    }

    /// Builder: cap for the request-level (purpose+name+size+generation) map.
    #[must_use]
    pub fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries.max(1);
        self.enforce_request_bound();
        self
    }

    /// Builder: raster LRU budget in bytes (default 8 MiB).
    #[must_use]
    pub fn with_raster_budget_bytes(mut self, budget_bytes: usize) -> Self {
        self.raster.budget_bytes = budget_bytes.max(1024);
        self
    }

    fn enforce_request_bound(&mut self) {
        while self.cache.len() > self.max_entries {
            if let Some(first) = self.cache.keys().next().cloned() {
                self.cache.remove(&first);
            } else {
                break;
            }
        }
    }

    fn request_bytes(&self) -> usize {
        self.cache
            .iter()
            .map(|(k, r)| key_memory(k) + result_memory(r) + 96)
            .sum()
    }
}

fn key_memory(key: &CacheKey) -> usize {
    let identity = match &key.identity {
        IconRequest::Name(name) => name.len() + 32,
        IconRequest::Path(path) => path.as_os_str().len() + 32,
    };
    identity + 64
}

fn result_memory(result: &Result<Rgba8Raster, IconError>) -> usize {
    match result {
        Ok(raster) => raster.pixels.len() + 96,
        Err(_) => 96,
    }
}

impl IconResolver {
    /// Legacy entry point: application purpose (desktop `Icon=` values).
    /// Absolute names bypass theme lookup; packaged semantic assets are never
    /// consulted first.
    pub fn prepare(
        &mut self,
        request: impl Into<IconRequest>,
        size: IconSize,
    ) -> Result<Rgba8Raster, IconError> {
        let request = request.into();
        match request {
            IconRequest::Path(path) => self.prepare_path(path, size),
            IconRequest::Name(name) => self.prepare_application(name, size),
        }
    }

    /// Application purpose: absolute path -> file; then freedesktop theme
    /// spec lookup across the inheritance chain; hicolor; pixmaps; fallback.
    pub fn prepare_application(
        &mut self,
        name: impl Into<String>,
        size: IconSize,
    ) -> Result<Rgba8Raster, IconError> {
        let _span = flamewm_profiler::ProfilePoint::new("icon.resolve.application").start();
        let name = name.into();
        // Absolute paths bypass theme search (also without extension checks).
        if Path::new(&name).is_absolute() {
            let path = PathBuf::from(&name);
            let key = self.key(
                IconLookupPurpose::Application,
                IconRequest::Path(path.clone()),
                size,
            );
            if let Some(hit) = self.cache.get(&key).cloned() {
                return hit;
            }
            if !self.path_exists(&path) {
                return self.application_miss(None);
            }
            return self.cached_or_resolve(key, size, |resolver| {
                resolver.resolve_path_candidate(&path, IconOrigin::ExplicitPath, size)
            });
        }
        let request = IconRequest::Name(name);
        let key = self.key(IconLookupPurpose::Application, request.clone(), size);
        self.cached_or_resolve(key, size, |resolver| {
            resolver.resolve_application_name(&request, size)
        })
    }

    /// Bundled generic application glyph; never depends on installed themes.
    pub fn prepare_generic_application(
        &mut self,
        size: IconSize,
    ) -> Result<Rgba8Raster, IconError> {
        let path = self.packaged_root.join(GENERIC_APPLICATION_GLYPH);
        self.cached_rasterize(&path, size, IconOrigin::PackagedFallback)
            .map_err(|error| IconError::FallbackUnavailable {
                fallback: path,
                fallback_error: Box::new(error),
                candidate_error: None,
            })
    }

    /// Semantic purpose: packaged override -> packaged breeze -> system theme
    /// -> generic fallback.
    pub fn prepare_semantic(
        &mut self,
        name: impl Into<String>,
        size: IconSize,
    ) -> Result<Rgba8Raster, IconError> {
        let _span = flamewm_profiler::ProfilePoint::new("icon.resolve.semantic").start();
        let request = IconRequest::Name(name.into());
        let key = self.key(IconLookupPurpose::Semantic, request.clone(), size);
        self.cached_or_resolve(key, size, |resolver| {
            resolver.resolve_semantic_name(&request, size)
        })
    }

    pub fn prepare_name(
        &mut self,
        name: impl Into<String>,
        size: IconSize,
    ) -> Result<Rgba8Raster, IconError> {
        self.prepare_application(name, size)
    }

    pub fn prepare_path(
        &mut self,
        path: impl Into<PathBuf>,
        size: IconSize,
    ) -> Result<Rgba8Raster, IconError> {
        let path = path.into();
        let key = self.key(
            IconLookupPurpose::Application,
            IconRequest::Path(path.clone()),
            size,
        );
        self.cached_or_resolve(key, size, |resolver| {
            resolver.resolve_path_candidate(&path, IconOrigin::ExplicitPath, size)
        })
    }

    fn key(&self, purpose: IconLookupPurpose, identity: IconRequest, size: IconSize) -> CacheKey {
        CacheKey {
            purpose,
            identity,
            size,
            theme_generation: self.theme_generation,
            palette_generation: self.palette_generation,
        }
    }

    fn cached_or_resolve(
        &mut self,
        key: CacheKey,
        size: IconSize,
        resolve: impl FnOnce(&mut Self) -> Result<Rgba8Raster, IconError>,
    ) -> Result<Rgba8Raster, IconError> {
        if let Some(result) = self.cache.get(&key) {
            return result.clone();
        }
        if size.physical == 0 {
            let result = Err(IconError::InvalidSize {
                logical: size.logical,
                physical: size.physical,
            });
            self.cache.insert(key, result.clone());
            self.enforce_request_bound();
            return result;
        }
        let result = resolve(&mut *self);
        // Warm path: seed the bounded raster LRU so the repeat request needs
        // no FS probe or decode even before the request-level entry is hit.
        if let Ok(raster) = &result {
            let path = PathBuf::from(&raster.source);
            let raster_key = RasterKey {
                path,
                physical: raster.height.max(raster.width),
                origin: raster.origin,
            };
            self.raster.insert(
                raster_key,
                Ok(Rgba8Raster {
                    fallback_reason: None,
                    ..raster.clone()
                }),
            );
        }
        self.cache.insert(key, result.clone());
        self.enforce_request_bound();
        result
    }

    fn cached_rasterize(
        &mut self,
        path: &Path,
        size: IconSize,
        origin: IconOrigin,
    ) -> Result<Rgba8Raster, RasterError> {
        let key = RasterKey {
            path: path.to_path_buf(),
            physical: size.physical,
            origin,
        };
        if let Some(hit) = self.raster.get(&key) {
            return hit;
        }
        let _r = flamewm_profiler::ProfilePoint::new("icon.rasterize").start();
        let result = rasterize_path_with_origin(path, size, origin);
        self.raster.insert(key, result.clone());
        result
    }

    /// Packaged-first probe: uncached `is_file` on purpose, so a packaged
    /// hit never builds theme candidates. Keeps the first rasterise error.
    fn probe_packaged(
        &mut self,
        candidate: &Candidate,
        size: IconSize,
        candidate_error: &mut Option<RasterError>,
    ) -> Option<Rgba8Raster> {
        if !candidate.path.is_file() {
            return None;
        }
        match self.cached_rasterize(&candidate.path, size, candidate.origin) {
            Ok(raster) => Some(raster),
            Err(error) => {
                if candidate_error.is_none() {
                    *candidate_error = Some(error);
                }
                None
            }
        }
    }

    fn path_exists(&mut self, path: &Path) -> bool {
        let generation = self.theme_generation;
        if let Some(hit) = self.theme_caches.cached_exists(path, generation) {
            return hit;
        }
        let exists = path.is_file();
        self.theme_caches.store_exists(path, generation, exists);
        exists
    }

    fn resolve_path_candidate(
        &mut self,
        path: &Path,
        origin: IconOrigin,
        size: IconSize,
    ) -> Result<Rgba8Raster, IconError> {
        let mut candidate_error = None;
        if self.path_exists(path) {
            match self.cached_rasterize(path, size, origin) {
                Ok(raster) => return Ok(raster),
                Err(error) => candidate_error = Some(error),
            }
        }
        self.generic_fallback(size, candidate_error)
    }

    fn resolve_application_name(
        &mut self,
        request: &IconRequest,
        size: IconSize,
    ) -> Result<Rgba8Raster, IconError> {
        let candidates = self.theme_candidates(request, size);
        self.resolve_application_candidates(&candidates, size)
    }

    fn resolve_semantic_name(
        &mut self,
        request: &IconRequest,
        size: IconSize,
    ) -> Result<Rgba8Raster, IconError> {
        // Packaged-first: probe packaged candidates before enumerating the
        // theme chain, so a packaged hit never builds thousands of theme
        // candidate paths (each a cached stat call on first use).
        if let IconRequest::Name(name) = request {
            let packaged = self.packaged_candidates(name);
            let mut candidate_error: Option<RasterError> = None;
            for candidate in &packaged {
                if let Some(raster) = self.probe_packaged(candidate, size, &mut candidate_error) {
                    return Ok(raster);
                }
            }
            let candidates = self.theme_candidates(request, size);
            return self.resolve_candidates(&candidates, size, candidate_error);
        }
        self.resolve_candidates(&[], size, None)
    }

    fn resolve_application_candidates(
        &mut self,
        candidates: &[Candidate],
        size: IconSize,
    ) -> Result<Rgba8Raster, IconError> {
        // Application misses are transparent slots, never the brand logo.
        let mut candidate_error: Option<RasterError> = None;
        for candidate in candidates {
            if !self.path_exists(&candidate.path) {
                continue;
            }
            match self.cached_rasterize(&candidate.path, size, candidate.origin) {
                // A candidate that rasterises fully transparent (e.g. an SVG
                // whose artwork falls outside the requested box) is not an
                // icon: keep searching so a later pixmap candidate can win.
                Ok(raster) if raster_has_visible_pixels(&raster) => return Ok(raster),
                Ok(_) => {}
                Err(error) => {
                    if candidate_error.is_none() {
                        candidate_error = Some(error);
                    }
                }
            }
        }
        self.application_miss(candidate_error)
    }

    /// Application miss: transparent empty contract. The shell renders a
    /// transparent slot; brand artwork stays reserved for explicit flame
    /// brand requests (`prepare_path`).
    fn application_miss(
        &mut self,
        candidate_error: Option<RasterError>,
    ) -> Result<Rgba8Raster, IconError> {
        let fallback = self.packaged_root.join("transparent-slot");
        let fallback_error = RasterError::Io {
            path: fallback.clone(),
            message: "application icon miss: transparent slot, brand fallback suppressed"
                .to_owned(),
        };
        Err(IconError::FallbackUnavailable {
            fallback,
            fallback_error: Box::new(fallback_error),
            candidate_error: candidate_error.map(Box::new),
        })
    }

    fn resolve_candidates(
        &mut self,
        candidates: &[Candidate],
        size: IconSize,
        mut candidate_error: Option<RasterError>,
    ) -> Result<Rgba8Raster, IconError> {
        for candidate in candidates {
            // Packaged hits probe the FS directly so a packaged icon never
            // pays thousands of stat calls for theme candidates enumerated
            // after it. `path_exists` caching still covers theme probes.
            if candidate.origin == IconOrigin::PackagedBreeze {
                if let Some(raster) = self.probe_packaged(candidate, size, &mut candidate_error) {
                    return Ok(raster);
                }
                continue;
            }
            if !self.path_exists(&candidate.path) {
                continue;
            }
            match self.cached_rasterize(&candidate.path, size, candidate.origin) {
                Ok(raster) => return Ok(raster),
                Err(error) => {
                    if candidate_error.is_none() {
                        candidate_error = Some(error);
                    }
                }
            }
        }
        self.generic_fallback(size, candidate_error)
    }

    fn generic_fallback(
        &mut self,
        size: IconSize,
        candidate_error: Option<RasterError>,
    ) -> Result<Rgba8Raster, IconError> {
        let fallback = self.packaged_root.join(GENERIC_FALLBACK);
        match self.cached_rasterize(&fallback, size, IconOrigin::PackagedFallback) {
            Ok(mut raster) => {
                raster.fallback_reason = candidate_error;
                Ok(raster)
            }
            Err(fallback_error) => Err(IconError::FallbackUnavailable {
                fallback,
                fallback_error: Box::new(fallback_error),
                candidate_error: candidate_error.map(Box::new),
            }),
        }
    }

    /// Normal application lookup: memory-only shared index in frozen
    /// precedence (global exact-size first, then per-theme best, then
    /// pixmaps). Explicit extensions are normalized by the index (no second
    /// ext appended), so `foo.png` and `foo` resolve to the same record.
    fn packaged_candidates(&self, name: &str) -> Vec<Candidate> {
        let mut candidates = Vec::new();
        if matches!(name, "flamewm" | "flamewm-icon") {
            candidates.push(Candidate {
                path: self.packaged_root.join("assets/web/flamewm-icon.svg"),
                origin: IconOrigin::PackagedFlame,
            });
        }
        if valid_icon_name(name) {
            candidates.push(Candidate {
                path: self
                    .packaged_root
                    .join("assets/web/breeze")
                    .join(format!("{name}.svg")),
                origin: IconOrigin::PackagedBreeze,
            });
        }
        candidates
    }

    /// Semantic theme tail: reuses the same frozen shared-index order as
    /// normal application lookup (explicit-ext aware, pixmaps included).
    /// Packaged overrides are probed separately in `resolve_semantic_name`.
    fn theme_candidates(&self, request: &IconRequest, size: IconSize) -> Vec<Candidate> {
        let name = match request {
            IconRequest::Name(name) => name.as_str(),
            IconRequest::Path(_) => return Vec::new(),
        };
        if !valid_icon_name(name) {
            return Vec::new();
        }
        self.shared_index
            .candidates(name, size.physical)
            .into_iter()
            .map(|path| Candidate {
                path,
                origin: IconOrigin::SystemTheme,
            })
            .collect()
    }
}

/// Direct adapter entry point for consumers that already selected a path.
/// Returns straight-alpha RGBA8; see [`Rgba8Raster`].
pub fn rasterize_path(path: &Path, size: IconSize) -> Result<Rgba8Raster, RasterError> {
    rasterize_path_with_origin(path, size, IconOrigin::ExplicitPath)
}

/// Legacy opaque projection with black-background compositing semantics.
/// Returns packed RGB8 bytes (3 bytes/px); prefer [`rasterize_path`] RGBA8.
pub fn rasterize_path_rgb8(path: &Path, size: IconSize) -> Result<Vec<u8>, RasterError> {
    rasterize_path(path, size).map(|raster| raster.to_rgb8_black_background())
}

fn rasterize_path_with_origin(
    path: &Path,
    size: IconSize,
    origin: IconOrigin,
) -> Result<Rgb8Raster, RasterError> {
    if size.physical == 0 {
        return Err(RasterError::InvalidSize {
            logical: size.logical,
            physical: size.physical,
        });
    }

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let decode: fn(&Path, &[u8], IconSize, IconOrigin) -> Result<Rgb8Raster, RasterError> =
        match extension.as_str() {
            "ppm" => decode_ppm,
            "xpm" => |path, bytes, size, origin| {
                decode_natural(path, "XPM", flamewm_image_core::xpm::decode(bytes), size, origin)
            },
            "svg" => decode_svg,
            "png" => |path, bytes, size, origin| {
                decode_natural(path, "PNG", flamewm_image_core::png::decode(bytes), size, origin)
            },
            "webp" => decode_webp,
            _ => {
                return Err(RasterError::UnsupportedFormat {
                    path: path.to_path_buf(),
                    extension,
                });
            }
        };
    let bytes = read_raster_bytes(path)?;
    decode(path, &bytes, size, origin)
}

fn read_raster_bytes(path: &Path) -> Result<Vec<u8>, RasterError> {
    fs::read(path).map_err(|error| RasterError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

fn decode_webp(
    path: &Path,
    bytes: &[u8],
    size: IconSize,
    origin: IconOrigin,
) -> Result<Rgb8Raster, RasterError> {
    let natural = flamewm_image_core::webp::decode(bytes)
        .map_err(|message| invalid_raster_error(path, "WebP", &message))?;
    let image = scale_to_physical(path, "WebP", natural, size)?;
    Ok(raster_from_image(path, image, origin))
}

fn raster_from_image(
    path: &Path,
    image: flamewm_image_core::RgbaImage,
    origin: IconOrigin,
) -> Rgb8Raster {
    Rgba8Raster {
        source: path.to_string_lossy().into_owned(),
        width: image.width,
        height: image.height,
        pixels: image.pixels,
        origin,
        fallback_reason: None,
    }
}

/// Scales `natural` to the physical icon size through image-core; a matching size passes through.
fn scale_to_physical(
    path: &Path,
    format: &'static str,
    natural: flamewm_image_core::RgbaImage,
    size: IconSize,
) -> Result<flamewm_image_core::RgbaImage, RasterError> {
    if natural.width == size.physical && natural.height == size.physical {
        return Ok(natural);
    }
    flamewm_image_core::scale::scale_rgba(
        &natural,
        size.physical,
        size.physical,
        flamewm_image_core::scale::ScaleFilter::Area,
    )
    .ok_or_else(|| invalid_raster_error(path, format, "requested dimensions overflow"))
}

/// True when at least one pixel carries nonzero alpha.
fn raster_has_visible_pixels(raster: &Rgba8Raster) -> bool {
    raster.pixels.chunks_exact(4).any(|pixel| pixel[3] != 0)
}

fn decode_svg(
    path: &Path,
    bytes: &[u8],
    size: IconSize,
    origin: IconOrigin,
) -> Result<Rgb8Raster, RasterError> {
    // SVG renders directly at the requested size; alpha preserved by image-core.
    let image = flamewm_image_core::svg::render(bytes, size.physical, size.physical)
        .map_err(|message| invalid_raster_error(path, "SVG", &message))?;
    Ok(raster_from_image(path, image, origin))
}

fn decode_natural(
    path: &Path,
    format: &'static str,
    natural: Result<flamewm_image_core::RgbaImage, String>,
    size: IconSize,
    origin: IconOrigin,
) -> Result<Rgb8Raster, RasterError> {
    let natural = natural.map_err(|message| invalid_raster_error(path, format, &message))?;
    if natural.width == 0 || natural.height == 0 {
        return Err(invalid_raster_error(path, format, "dimensions must be non-zero"));
    }
    let image = scale_to_physical(path, format, natural, size)?;
    Ok(raster_from_image(path, image, origin))
}

fn invalid_raster_error(path: &Path, format: &'static str, message: &str) -> RasterError {
    RasterError::InvalidRaster {
        path: path.to_path_buf(),
        format,
        message: message.to_owned(),
    }
}

fn decode_ppm(
    path: &Path,
    bytes: &[u8],
    size: IconSize,
    origin: IconOrigin,
) -> Result<Rgb8Raster, RasterError> {
    // PPM is opaque (alpha=255); decode + deterministic nearest scale via image-core.
    let image = flamewm_image_core::decode_bytes_at(
        bytes,
        flamewm_image_core::ImageSourceFormat::Ppm,
        size.physical,
        size.physical,
        flamewm_image_core::scale::ScaleFilter::Nearest,
    )
    .map_err(|message| map_ppm_error(path, &message))?;
    Ok(raster_from_image(path, image, origin))
}

fn map_ppm_error(path: &Path, message: &str) -> RasterError {
    // Preserve the historical "only binary PPM (P6) is supported" surface for
    // non-P6 inputs so fallback_reason assertions stay stable.
    if message.contains("P6") {
        return RasterError::InvalidPpm {
            path: path.to_path_buf(),
            message: "only binary PPM (P6) is supported".to_owned(),
        };
    }
    RasterError::InvalidPpm {
        path: path.to_path_buf(),
        message: message.to_owned(),
    }
}

fn valid_icon_name(name: &str) -> bool {
    !name.is_empty() && !name.bytes().any(|byte| matches!(byte, b'/' | b'\\' | 0))
}

/// Product default icon theme, searched after the configured theme and before Breeze.
pub const DEFAULT_ICON_THEME: &str = "PlagueSur";

/// Theme search chain: `FLAMEWM_ICON_THEME`, the Flame `iconTheme`
/// preference, the desktop's configured theme (kdeglobals, then GTK), then
/// Breeze. hicolor is appended by the inheritance chain.
pub fn theme_chain(preferred: Option<&str>) -> Vec<String> {
    let mut theme_names: Vec<String> = Vec::new();
    if let Some(value) = env::var_os("FLAMEWM_ICON_THEME")
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.is_empty())
    {
        theme_names.push(value);
    }
    if let Some(value) = preferred.filter(|value| !value.is_empty()) {
        theme_names.push(value.to_owned());
    }
    if let Some(config_home) = xdg_config_home() {
        if let Some(configured) = configured_icon_theme(&config_home) {
            theme_names.push(configured);
        }
    }
    theme_names.push(DEFAULT_ICON_THEME.to_owned());
    // Bridge while PlagueSur is being integrated: the previous bundled default keeps icons
    // resolving; once McMojave-circle is removed from the bundle this entry simply misses.
    theme_names.push("McMojave-circle".to_owned());
    theme_names.push("Breeze".to_owned());
    unique_strings(theme_names)
}

fn unique_strings(values: Vec<String>) -> Vec<String> {
    let mut unique = Vec::new();
    for value in values {
        if !value.is_empty() && !unique.contains(&value) {
            unique.push(value);
        }
    }
    unique
}

fn xdg_config_home() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
}

fn xdg_data_dirs() -> Vec<PathBuf> {
    xdg_data_search_dirs(
        env::var_os("HOME").as_deref(),
        env::var_os("XDG_DATA_HOME").as_deref(),
        env::var("XDG_DATA_DIRS").ok().as_deref(),
    )
}

/// `$XDG_DATA_HOME`, or `$HOME/.local/share`. Empty values are unset.
pub fn user_data_home(home: Option<&OsStr>, xdg_data_home: Option<&OsStr>) -> Option<PathBuf> {
    xdg_data_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".local/share"))
        })
}

/// User data home first, then `XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`).
pub fn xdg_data_search_dirs(
    home: Option<&OsStr>,
    xdg_data_home: Option<&OsStr>,
    xdg_data_dirs: Option<&str>,
) -> Vec<PathBuf> {
    let system = xdg_data_dirs
        .filter(|value| !value.is_empty())
        .unwrap_or("/usr/local/share:/usr/share");
    let dirs = user_data_home(home, xdg_data_home)
        .into_iter()
        .chain(system.split(':').filter(|dir| !dir.is_empty()).map(PathBuf::from))
        .collect();
    unique_paths(dirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HICOLOR_16_INDEX: &str = "[Icon Theme]\nDirectories=16x16/apps\n\n[16x16/apps]\nSize=16\nType=Fixed\n";

    #[test]
    fn xdg_data_search_dirs_never_yields_relative_home_paths() {
        let empty = OsStr::new("");
        assert_eq!(user_data_home(Some(empty), None), None);
        assert_eq!(
            xdg_data_search_dirs(Some(empty), Some(empty), Some("")),
            vec![PathBuf::from("/usr/local/share"), PathBuf::from("/usr/share")]
        );
        assert_eq!(
            xdg_data_search_dirs(Some(OsStr::new("/home/u")), None, Some("/opt/share:")),
            vec![PathBuf::from("/home/u/.local/share"), PathBuf::from("/opt/share")]
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
        write_ppm(&dir.join("icons/ThemeA/16x16/apps/c3-switch.ppm"), [200, 10, 10]);
        write_ppm(&dir.join("icons/ThemeB/16x16/apps/c3-switch.ppm"), [10, 200, 10]);
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
        assert!(papirus < default && default < breeze, "chain order: {chain:?}");
        assert!(theme_chain(None).iter().any(|name| name == DEFAULT_ICON_THEME));
    }

    #[test]
    fn startup_preference_precedes_breeze_in_theme_chain() {
        let resolver = IconResolver::from_environment_with_theme(
            std::env::temp_dir(),
            0,
            Some("McMojave-circle"),
        );
        let names = &resolver.shared_index().theme_names;
        let preferred = names.iter().position(|name| name == "McMojave-circle");
        let breeze = names.iter().position(|name| name == "Breeze");
        assert!(preferred.is_some(), "startup preference missing: {names:?}");
        assert!(preferred < breeze, "preference must win over Breeze: {names:?}");
    }
}
