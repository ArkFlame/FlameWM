//! `StatusNotifier` tray host for the panel `#tray-group`.
//!
//! Owns one `flamewm_tray::TrayWatcher` on the shell reactor (no thread, no timer). Items
//! project into `SLOT_COUNT` fixed template buttons (`tray-sni-1..N`). When more items than
//! fit exist, one slot is reserved for the chevron (`tray-sni-overflow`), which pages through
//! the list. Clicks forward to the item: primary = `Activate`, middle = `SecondaryActivate`,
//! secondary = `ContextMenu`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use flamewm_api::Point;
use flamewm_dbus_reactor::BusWatch;
use flamewm_tray::{
    parse_layout, plan_overflow, top_level_rows, IconPixmap, ItemAction, ItemKey, ItemProps,
    LayoutReply, TrayMenuRow, TrayWatcher,
};
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
/// Rows the flat dbusmenu popup can show; the task-menu markup carries this many `tray.menu.<n>`.
pub const TRAY_MENU_ROWS: usize = 10;
/// Action prefix of the dbusmenu popup rows.
pub const TRAY_MENU_ACTION_PREFIX: &str = "tray.menu.";
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

/// Next page after `page`, wrapping to page 0 after the last of `pages` pages.
#[must_use]
pub const fn next_page(page: usize, pages: usize) -> usize {
    (page + 1) % if pages == 0 { 1 } else { pages }
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

/// Dbusmenu popup row `action` (`tray.menu.<n>`) of an open tray menu, as the `Event` target.
/// `None` for other menus, other actions, out-of-range rows, and disabled rows.
#[must_use]
pub fn menu_row_target(
    kind: &crate::ContextMenuKind,
    action: &str,
) -> Option<(ItemKey, String, i32)> {
    let crate::ContextMenuKind::TrayItem {
        key,
        menu_path,
        rows,
    } = kind
    else {
        return None;
    };
    let index = action
        .strip_prefix(TRAY_MENU_ACTION_PREFIX)?
        .parse::<usize>()
        .ok()
        .filter(|index| *index < TRAY_MENU_ROWS)?;
    let row = rows.get(index).filter(|row| row.enabled)?;
    Some((key.clone(), menu_path.clone(), row.id))
}

/// A dbusmenu layout that is ready to show as the flat popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayMenuOpen {
    pub key: ItemKey,
    pub menu_path: String,
    pub rows: Vec<TrayMenuRow>,
    /// Root-coordinate device pixels of the click.
    pub pointer: Point,
}

/// A right-click waiting for its item's `GetLayout` reply.
struct MenuRequest {
    key: ItemKey,
    menu_path: String,
    pointer: Point,
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
    menu_request: Option<MenuRequest>,
}

impl TrayHost {
    /// Claims the `StatusNotifier` watcher name. `None` when another watcher already owns it;
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
                menu_request: None,
            }),
            Err(error) => {
                eprintln!(
                    "flamewm-shell: StatusNotifier tray disabled: {}",
                    error.message
                );
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
        self.page = next_page(self.page, self.window().pages);
        self.dirty = true;
    }

    /// Takes the layout of a pending right-click. A malformed or empty layout, or a failed
    /// `GetLayout`, falls back to the item's own `ContextMenu` call.
    pub fn take_menu_ready(&mut self) -> Option<TrayMenuOpen> {
        let key = self.menu_request.as_ref()?.key.clone();
        let reply = self.watcher.take_layout(&key)?;
        let request = self.menu_request.take()?;
        let rows = match reply {
            LayoutReply::Menu(node) => top_level_rows(&parse_layout(&node)),
            LayoutReply::Failed => Vec::new(),
        };
        if rows.is_empty() {
            self.invoke_context_menu(&request.key, request.pointer);
            return None;
        }
        Some(TrayMenuOpen {
            key: request.key,
            menu_path: request.menu_path,
            rows,
            pointer: request.pointer,
        })
    }

    /// Sends the dbusmenu `clicked` event for an open popup row, nonfatal on bus failure.
    pub fn send_menu_event(&self, key: &ItemKey, menu_path: &str, id: i32) {
        if let Err(error) = self.watcher.send_menu_event(key, menu_path, id) {
            crate::log_limit::emit(
                "tray menu event failed",
                format_args!("flamewm-shell: tray menu event failed: {}", error.message),
            );
        }
    }

    fn invoke_context_menu(&self, key: &ItemKey, pointer: Point) {
        if let Err(error) = self
            .watcher
            .invoke(key, ItemAction::ContextMenu, pointer.x, pointer.y)
        {
            crate::log_limit::emit(
                "tray item call failed",
                format_args!("flamewm-shell: tray item call failed: {}", error.message),
            );
        }
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
        let button = PointerButton::from_raw_x(event.action.button);
        let method = match button {
            PointerButton::Primary => ItemAction::Activate,
            PointerButton::Middle => ItemAction::SecondaryActivate,
            PointerButton::Secondary => ItemAction::ContextMenu,
            _ => return true,
        };
        let Some((key, props)) = self.item_for_slot(slot) else {
            return true;
        };
        let key = key.clone();
        let menu_path = props.menu_path.clone();
        let Ok(origin) = runtime.surface_device_rect(event.surface) else {
            return true;
        };
        let x = origin.x as i32 + event.action.x as i32;
        let y = origin.y as i32 + event.action.y as i32;
        // An item with a dbusmenu object shows its entries in the panel popup; its own
        // ContextMenu call is the fallback when the layout is unavailable.
        if let (PointerButton::Secondary, Some(path)) = (button, menu_path) {
            match self.watcher.request_layout(&key, &path) {
                Ok(()) => {
                    self.menu_request = Some(MenuRequest {
                        key,
                        menu_path: path,
                        pointer: Point { x, y },
                    });
                    return true;
                }
                Err(error) => {
                    crate::log_limit::emit(
                        "tray layout request failed",
                        format_args!(
                            "flamewm-shell: tray layout request failed: {}",
                            error.message
                        ),
                    );
                }
            }
        }
        if let Err(error) = self.watcher.invoke(&key, method, x, y) {
            crate::log_limit::emit(
                "tray item call failed",
                format_args!("flamewm-shell: tray item call failed: {}", error.message),
            );
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
        SNI_SHOWN.store(
            self.window().shown > 0,
            std::sync::atomic::Ordering::Relaxed,
        );
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

/// Whether any `StatusNotifier` item is projected; keeps the tray capsule painted.
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
        assert!(pages
            .iter()
            .all(|window| window.chevron && window.pages == 3));
        assert_eq!(pages[3].first, 0, "page wraps to the first page");
    }

    #[test]
    fn page_cycle_wraps_back_to_the_first_page() {
        let mut page = 0;
        let mut visited = Vec::new();
        for _ in 0..4 {
            visited.push(tray_window(13, page).first);
            page = next_page(page, tray_window(13, page).pages);
        }
        assert_eq!(visited, vec![0, 5, 10, 0], "three pages, then page 1 again");
        assert_eq!(next_page(0, 0), 0, "no pages stays on page 0");
    }

    #[test]
    fn menu_row_target_resolves_only_enabled_rows_of_tray_menus() {
        let key = ItemKey::new(":1.9", "/StatusNotifierItem").expect("key");
        let kind = crate::ContextMenuKind::TrayItem {
            key: key.clone(),
            menu_path: "/MenuBar".to_owned(),
            rows: vec![
                TrayMenuRow {
                    id: 4,
                    label: "Open".into(),
                    enabled: true,
                },
                TrayMenuRow {
                    id: 5,
                    label: "Quit".into(),
                    enabled: false,
                },
            ],
        };
        assert_eq!(
            menu_row_target(&kind, "tray.menu.0"),
            Some((key, "/MenuBar".to_owned(), 4))
        );
        assert_eq!(menu_row_target(&kind, "tray.menu.1"), None, "disabled");
        assert_eq!(menu_row_target(&kind, "tray.menu.2"), None, "no such row");
        assert_eq!(menu_row_target(&kind, "tray.menu.x"), None, "not an index");
        assert_eq!(menu_row_target(&kind, "tray.sni.1"), None, "other prefix");
        let other = crate::ContextMenuKind::Taskbar;
        assert_eq!(menu_row_target(&other, "tray.menu.0"), None, "other menu");
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
