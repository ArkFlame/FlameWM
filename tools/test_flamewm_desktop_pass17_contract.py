"""Offline Pass 17 contracts, not a replacement for compiling or X11 acceptance."""
import pathlib
import re
import unittest
import xml.etree.ElementTree as ET

ROOT = pathlib.Path(__file__).resolve().parents[1]

def src(path):
    return (ROOT / path).read_text(encoding="utf-8")

class DesktopPass17Contract(unittest.TestCase):
    def test_regular_file_icon_does_not_inherit_pen_fallback(self):
        projection = src("crates/flamewm-desktop/src/icon_names.rs")
        desktop = "".join(src(p) for p in (
            "crates/flamewm-desktop/src/main.rs",
            "crates/flamewm-desktop/src/slots.rs",
            "crates/flamewm-desktop/src/refresh.rs",
        ))
        self.assertIn('"pdf" => "application-pdf"', projection)
        self.assertIn('"png" => "image-png"', projection)
        self.assertIn('"doc" | "docx" | "odt" => "x-office-document"', projection)
        self.assertIn('kind == DesktopItemKind::File', desktop)
        self.assertIn('projection::file_icon_name(path)', desktop)
        self.assertRegex(desktop, re.compile(r"let kind = item\.kind;\s*let icon = launcher_icon\(state, kind, &item\.path\);"))

    def test_drag_preview_matches_full_entry_style(self):
        html = src("ui/desktop/index.html")
        css = src("ui/desktop/desktop.css")
        main = src("crates/flamewm-desktop/src/main.rs")
        self.assertIn('id="desktop-drag-ghost"', html)
        self.assertRegex(css, r"\.desktop-drag-ghost\s*\{[^}]*opacity:0\.50;")
        drag = src("crates/flamewm-desktop/src/drag.rs")
        self.assertRegex(drag, re.compile(r"desktop_label_lines\(\s*&ghost\.label,\s*76\.0 \* factor,\s*13\.0 \* factor"))
        self.assertIn('ghost.count > 1', drag)
        self.assertIn('document.position("desktop-drag-ghost", gx, gy)', drag)

    def test_delete_always_requires_confirm_and_uses_trash(self):
        main = src("crates/flamewm-desktop/src/events.rs")
        trash = src("crates/flamewm-desktop/src/files.rs")
        html = src("ui/desktop/index.html")
        action = src("crates/flamewm-desktop-core/src/file_actions.rs")
        events = src("crates/flamewm-render-x11/src/app/events.rs")
        self.assertIn('if action.text.as_deref() == Some("\\u{7f}")', main)
        self.assertIn('state.pending_delete = Some(paths);', main)
        self.assertIn('Trash::move_to_trash(path, &home, &data_home, uid)', trash)
        self.assertNotIn('delete_permanent(', main + trash)
        self.assertIn('data-action="confirm.trash"', html)
        self.assertIn('Move to Trash', html)
        self.assertIn('Move to Trash', action)
        self.assertIn('XK_DELETE => Some("\\u{7f}".to_string())', events)

    def test_minimize_restores_existing_window_placement(self):
        wm = src("crates/flamewm-wm-x11/src/wm/events.rs")
        self.assertRegex(wm, re.compile(r"\.is_some_and\(\|state\| state\.minimized\)\s*\{\s*[^}]*self\.restore_minimized\(event\.window\)", re.M))
        prefs = src("crates/flamewm-platform/src/settings/product.rs")
        self.assertIn('window_open_animation: AnimationStyle::None', prefs)
        self.assertIn('window_open_animation_enabled: false', prefs)

    def test_radial_eight_slots_and_selected_native_sector_owner(self):
        html = src("ui/desktop/index.html")
        css = src("ui/desktop/desktop.css")
        build = src("crates/flamewm-desktop/build.rs")
        self.assertIn('id="convert-wheel-highlight"', html)
        self.assertIn('data-action="desktop.convert.surface"', html)
        self.assertNotIn('desktop-convert-wheel-art', html)
        for i in range(8):
            self.assertIn(f'id="convert-slot-{i}"', html)
            self.assertIn(f'data-action="convert.slot.{i}"', html)
            self.assertIn(f'.convert-wheel-pos-{i}', css)
        self.assertNotIn('wheel_art_path', build)
        self.assertIn('mod convert_wheel;', src("crates/flamewm-desktop/src/main.rs"))
        self.assertIn('assert_dynamic_text_target(output, &id)', build)

    def test_document_converter_routes_are_conditional(self):
        conversion = src("crates/flamewm-conversion-core/src/conversion.rs")
        self.assertIn('available_program("pandoc")', conversion)
        self.assertIn('available_program("pdftotext")', conversion)
        self.assertIn('"pdf" if has_pdftotext => vec!["txt"]', conversion)
        self.assertIn('"txt" =>', conversion)
        self.assertIn('"md" | "markdown" if has_pandoc', conversion)
        self.assertIn('rename_entry_no_replace(&temp, &dest)', conversion)
        self.assertIn('pub fn convert_file(', conversion)

    def test_prediction_is_completion_driven_not_fabricated_percentage(self):
        main = "".join(src(p) for p in (
            "crates/flamewm-desktop/src/menu_actions.rs",
            "crates/flamewm-desktop/src/menus.rs",
            "crates/flamewm-desktop/src/refresh.rs",
        ))
        html = src("ui/desktop/index.html")
        for value in ('desktop-convert-preview', 'desktop-convert-preview-icon',
                      'desktop-convert-preview-line-1', 'desktop-convert-preview-line-2',
                      'desktop-convert-preview-status'):
            self.assertIn(f'id="{value}"', html)
        self.assertIn('state.conversion_preview = Some(ConversionPreview', main)
        self.assertIn('worker_dirty.store(true, Ordering::Release)', main)
        self.assertRegex(main, re.compile(r"sync_conversion_preview\(document, state\)\?"))
        self.assertRegex(main, re.compile(r"let Some\(outcome\) = outcome else \{\s*return Ok\(\(\)\);\s*\};"))
        self.assertRegex(main, re.compile(r"state\.model\.set_position\(\s*path,\s*StoredPosition \{"))

if __name__ == "__main__":
    unittest.main()
