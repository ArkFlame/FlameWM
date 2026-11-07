//! StatusNotifier tray host for the panel `#tray-group`.
//!
//! Owns one `flamewm_tray::TrayWatcher` on the shell reactor (no thread, no timer). Items
//! project into `SLOT_COUNT` fixed template buttons (`tray-sni-1..N`). When more items than
//! fit exist, one slot is reserved for the chevron (`tray-sni-overflow`), which pages through
//! the list. Clicks forward to the item: primary = `Activate`, middle = `SecondaryActivate`,
//! secondary = `ContextMenu`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use flamewm_dbus_reactor::BusWatch;
use flamewm_tray::{IconPixmap, ItemAction, ItemKey, ItemProps, TrayWatcher, plan_overflow};
use flamewm_ui_x11::{
    ActionPhase, PointerButton, RuntimeImage, SurfaceEvent, SurfaceRuntime, UiDocumentAccess,
};

use crate::async_projection::schedule_visible;
use crate::icon_loader::{IconLoader, IconTarget};
use crate::projection::project_tray_button;
use crate::taskbar_scale;

/// Fixed item buttons in the panel markup. With more items, one slot is the chevron's.
pub const SLOT_COUNT: usize = 6;
/// Action prefix of the item buttons: `tray.sni.<slot>`.
pub const SLOT_ACTION_PREFIX: &str = "tray.sni.";
/// Action of the overflow chevron.
pub const OVERFLOW_ACTION: &str = "tray.sni.overflow";
/// Tray image edge in CSS px (`.tray-button img`).
const ICON_CSS_PX: f32 = 17.0;
/// Generic glyph for an item with neither a name nor a pixmap.
const FALLBACK_ICON: &str = "application-x-executable";

/// Which items the panel shows on one page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayWindow {
    /// Index of the first shown item in registry order.
    pub first: usize,
    /// Number of items shown on this page.
    pub shown: usize,
    /// Number of pages (1 when everything fits).
    pub pages: usize,
    /// True when the chevron is visible.
    pub chevron: bool,
}

/// Pure paging policy: everything fits in `SLOT_COUNT`, or one slot goes to the chevron and
/// the list pages `SLOT_COUNT - 1` items at a time. `page` wraps modulo the page count.
#[must_use]
pub fn tray_window(total: usize, page: usize) -> TrayWindow {
    let plan = plan_overflow(total, SLOT_COUNT);
    if !plan.chevron {
        return TrayWindow {
            first: 0,
            shown: total,
            pages: 1,
            chevron: false,
        };
    }
    let per_page = plan.visible;
    let pages = total.div_ceil(per_page);
    let first = (page % pages) * per_page;
    TrayWindow {
        first,
        shown: per_page.min(total - first),
        pages,
        chevron: true,
    }
}

/// Device-px edge of a tray icon request (`ICON_CSS_PX` scaled by the taskbar factor).
#[must_use]
pub fn icon_device_px() -> u32 {
    (ICON_CSS_PX * taskbar_scale::factor()).round().max(1.0) as u32
}

/// Pixmap closest to the target edge from above, else the largest one available.
#[must_use]
pub fn best_pixmap(pixmaps: &[IconPixmap], edge: u32) -> Option<&IconPixmap> {
    pixmaps
        .iter()
        .filter(|pixmap| pixmap.width >= edge && pixmap.height >= edge)
        .min_by_key(|pixmap| pixmap.width.max(pixmap.height))
        .or_else(|| {
            pixmaps
                .iter()
                .max_by_key(|pixmap| u64::from(pixmap.width) * u64::from(pixmap.height))
        })
}

/// Converts one `ARGB32` (network order) pixmap to straight RGBA8 for the UI document.
#[must_use]
pub fn pixmap_to_image(pixmap: &IconPixmap) -> RuntimeImage {
    let mut hasher = DefaultHasher::new();
    pixmap.argb.hash(&mut hasher);
    let pixels = pixmap
        .argb
        .chunks_exact(4)
        .flat_map(|argb| [argb[1], argb[2], argb[3], argb[0]])
        .collect();
    RuntimeImage {
        source: format!(
            "tray-sni-{:016x}-{}x{}",
            hasher.finish(),
            pixmap.width,
            pixmap.height
        ),
        width: pixmap.width,
        height: pixmap.height,
        pixels,
    }
}

pub struct TrayHost {
    watcher: TrayWatcher,
    entries: Vec<(ItemKey, ItemProps)>,
    revision: Option<u64>,
    page: usize,
    dirty: bool,
}

impl TrayHost {
    /// Claims the StatusNotifier watcher name. `None` when another watcher already owns it;
    /// the shell then runs without a tray.
    #[must_use]
    pub fn connect() -> Option<Self> {
        match TrayWatcher::connect() {
            Ok(watcher) => Some(Self {
                watcher,
                entries: Vec::new(),
                revision: None,
                page: 0,
                dirty: true,
            }),
            Err(error) => {
                eprintln!("flamewm-shell: StatusNotifier tray disabled: {}", error.message);
                None
            }
        }
    }

    #[must_use]
    pub fn watch(&self) -> BusWatch {
        self.watcher.watch()
    }

    /// Drains bus traffic and refreshes the snapshot when the watcher revision moved.
    /// Projection happens later in the shell tick, which reads `take_dirty`.
    pub fn on_ready(&mut self) -> Result<(), String> {
        self.watcher.on_ready().map_err(|error| error.message)?;
        let revision = self.watcher.revision();
        if self.revision != Some(revision) {
            self.revision = Some(revision);
            self.entries = self.watcher.entries();
            self.dirty = true;
        }
        Ok(())
    }

    /// True once after any change that the panel must show.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Re-arms projection after a failed panel update.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn window(&self) -> TrayWindow {
        tray_window(self.entries.len(), self.page)
    }

    fn item_for_slot(&self, slot: usize) -> Option<&(ItemKey, ItemProps)> {
        let window = self.window();
        (slot < window.shown)
            .then(|| self.entries.get(window.first + slot))
            .flatten()
    }

    fn cycle_page(&mut self) {
        let pages = self.window().pages;
        self.page = (self.page + 1) % pages.max(1);
        self.dirty = true;
    }

    /// Routes a tray button release. Returns `true` when the event belongs to the tray and
    /// must not reach the panel router. Only inside releases act, so the press is ignored.
    pub fn handle_event(&mut self, event: &SurfaceEvent, runtime: &mut SurfaceRuntime) -> bool {
        let action = event.action.action.as_str();
        if !action.starts_with(SLOT_ACTION_PREFIX)
            || event.action.phase != ActionPhase::Release
            || !event.action.inside
        {
            return false;
        }
        if action == OVERFLOW_ACTION {
            self.cycle_page();
            return true;
        }
        let Some(slot) = action
            .strip_prefix(SLOT_ACTION_PREFIX)
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|slot| *slot < SLOT_COUNT)
        else {
            return false;
        };
        let method = match PointerButton::from_raw_x(event.action.button) {
            PointerButton::Primary => ItemAction::Activate,
            PointerButton::Middle => ItemAction::SecondaryActivate,
            PointerButton::Secondary => ItemAction::ContextMenu,
            _ => return true,
        };
        let Some((key, _)) = self.item_for_slot(slot) else {
            return true;
        };
        let Ok(origin) = runtime.surface_device_rect(event.surface) else {
            return true;
        };
        let x = origin.x as i32 + event.action.x as i32;
        let y = origin.y as i32 + event.action.y as i32;
        if let Err(error) = self.watcher.invoke(key, method, x, y) {
            eprintln!("flamewm-shell: tray item call failed: {}", error.message);
        }
        true
    }

    /// Projects visible item slots and the chevron into one panel document.
    pub fn project(
        &self,
        document: &mut impl UiDocumentAccess,
        loader: Option<&mut IconLoader>,
    ) -> Result<(), String> {
        let mut loader = loader;
        let edge = icon_device_px();
        SNI_SHOWN.store(self.window().shown > 0, std::sync::atomic::Ordering::Relaxed);
        for slot in 0..SLOT_COUNT {
            let slot_id = format!("tray-sni-{}", slot + 1);
            let icon_id = format!("{slot_id}-icon");
            let Some((_, props)) = self.item_for_slot(slot) else {
                document.visible(&slot_id, false)?;
                project_tray_button(document, &slot_id, false)?;
                continue;
            };
            document.visible(&slot_id, true)?;
            project_tray_button(document, &slot_id, true)?;
            if let Some(pixmap) = best_pixmap(&props.icon_pixmaps, edge) {
                document.image_rgba8(&icon_id, pixmap_to_image(pixmap))?;
                continue;
            }
            let name = if props.icon_name.is_empty() {
                FALLBACK_ICON
            } else {
                props.icon_name.as_str()
            };
            schedule_visible(
                document,
                &icon_id,
                &mut loader.as_deref_mut(),
                IconTarget::TraySlot(slot as u8),
                name,
                edge,
                edge,
            )?;
        }
        let chevron = self.window().chevron;
        document.visible("tray-sni-overflow", chevron)?;
        project_tray_button(document, "tray-sni-overflow", chevron)?;
        Ok(())
    }
}

/// Whether any StatusNotifier item is projected; keeps the tray capsule painted.
static SNI_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn sni_shown() -> bool {
    SNI_SHOWN.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_fits_without_chevron_or_paging() {
        for total in 0..=SLOT_COUNT {
            let window = tray_window(total, 3);
            assert_eq!(
                window,
                TrayWindow {
                    first: 0,
                    shown: total,
                    pages: 1,
                    chevron: false
                }
            );
        }
    }

    #[test]
    fn overflow_pages_through_all_items_and_wraps() {
        // 13 items: one chevron slot, 5 items per page -> 3 pages (5, 5, 3).
        let pages: Vec<TrayWindow> = (0..4).map(|page| tray_window(13, page)).collect();
        assert_eq!(pages[0].first, 0);
        assert_eq!(pages[0].shown, 5);
        assert_eq!(pages[1].first, 5);
        assert_eq!(pages[2].first, 10);
        assert_eq!(pages[2].shown, 3);
        assert!(pages.iter().all(|window| window.chevron && window.pages == 3));
        assert_eq!(pages[3].first, 0, "page wraps to the first page");
    }

    #[test]
    fn best_pixmap_prefers_smallest_size_at_or_above_edge() {
        let make = |edge: u32| IconPixmap {
            width: edge,
            height: edge,
            argb: vec![0; (edge * edge * 4) as usize],
        };
        let pixmaps = [make(16), make(32), make(24)];
        assert_eq!(best_pixmap(&pixmaps, 17).map(|p| p.width), Some(24));
        assert_eq!(best_pixmap(&pixmaps, 40).map(|p| p.width), Some(32));
        assert_eq!(best_pixmap(&[], 17), None);
    }

    #[test]
    fn pixmap_conversion_rotates_argb_to_rgba() {
        let pixmap = IconPixmap {
            width: 1,
            height: 1,
            argb: vec![0x80, 0x11, 0x22, 0x33],
        };
        let image = pixmap_to_image(&pixmap);
        assert_eq!(image.pixels, vec![0x11, 0x22, 0x33, 0x80]);
        assert_eq!((image.width, image.height), (1, 1));
    }
}
