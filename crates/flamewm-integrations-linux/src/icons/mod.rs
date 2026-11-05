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

use crate::icon_theme::IconThemeCaches as ThemeLookupCaches;
use crate::icon_theme::{configured_icon_theme, unique_paths};

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

/// Compatibility alias for the J06 `RuntimeImage` bridge consumers
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
        let hit = self.entries.get(key)?.clone();
        if let Some(position) = self.order.iter().position(|entry| entry == key) {
            self.order.remove(position);
            self.order.push_back(key.clone());
        }
        Some(hit)
    }

    fn insert(&mut self, key: RasterKey, result: Result<Rgba8Raster, RasterError>) {
        if self.entries.contains_key(&key) {
            return;
        }
        let cost = Self::raster_bytes(&result);
        // A single oversized icon must not evict the working set and then
        // remain cached above the byte budget. The caller still gets the raster.
        if cost > self.budget_bytes {
            return;
        }
        while !self.entries.is_empty() && self.used_bytes + cost > self.budget_bytes {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(removed) = self.entries.remove(&old) {
                self.used_bytes = self.used_bytes.saturating_sub(Self::raster_bytes(&removed));
            }
        }
        self.order.push_back(key.clone());
        self.used_bytes += cost;
        self.entries.insert(key, result);
    }

    fn memory_estimate_bytes(&self) -> usize {
        self.used_bytes + self.entries.len() * 96
    }
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
        let mut resolver = Self::from_shared_index(
            packaged_root,
            theme_names,
            data_dirs,
            theme_generation,
            index,
        );
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
                ..RasterCache::default()
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

    /// Estimated bytes held by theme lookup caches + bounded raster LRU.
    #[must_use]
    pub fn memory_estimate_bytes(&self) -> usize {
        self.theme_caches.memory_estimate_bytes()
            + self.raster.memory_estimate_bytes()
            + self.request_bytes()
    }

    fn enforce_request_bound(&mut self) {
        while self.cache.len() > self.max_entries {
            let Some(first) = self.cache.keys().next().cloned() else {
                break;
            };
            self.cache.remove(&first);
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
        let result = if size.physical == 0 {
            Err(IconError::InvalidSize {
                logical: size.logical,
                physical: size.physical,
            })
        } else {
            resolve(&mut *self)
        };
        // Warm path: seed the bounded raster LRU so the repeat request needs
        // no FS probe or decode even before the request-level entry is hit.
        if let Ok(raster) = &result {
            let raster_key = RasterKey {
                path: PathBuf::from(&raster.source),
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
                decode_natural(
                    path,
                    "XPM",
                    flamewm_image_core::xpm::decode(bytes),
                    size,
                    origin,
                )
            },
            "svg" => decode_svg,
            "png" => |path, bytes, size, origin| {
                decode_natural(
                    path,
                    "PNG",
                    flamewm_image_core::png::decode(bytes),
                    size,
                    origin,
                )
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
        return Err(invalid_raster_error(
            path,
            format,
            "dimensions must be non-zero",
        ));
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
pub const DEFAULT_ICON_THEME: &str = "MacTahoe";

/// Theme search chain: `FLAMEWM_ICON_THEME`, the Flame `iconTheme`
/// preference, the desktop's configured theme (kdeglobals, then GTK), then
/// Breeze. hicolor is appended by the inheritance chain.
#[must_use]
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
        .chain(
            system
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
        )
        .collect();
    unique_paths(dirs)
}

#[cfg(test)]
mod tests;
