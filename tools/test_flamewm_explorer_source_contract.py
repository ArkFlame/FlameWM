"""Source-level gates for the new FlameWM Explorer. Native Rust/UI tests remain required."""
from __future__ import annotations
import re
import unittest
from html.parser import HTMLParser
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

class Nodes(HTMLParser):
    def __init__(self):
        super().__init__()
        self.ids = []
        self.actions = []
    def handle_starttag(self, tag, attrs):
        values = dict(attrs)
        if "id" in values:
            self.ids.append(values["id"])
        if "data-action" in values:
            self.actions.append(values["data-action"])

class ExplorerContract(unittest.TestCase):
    def test_compiled_template_declares_all_projected_rows(self):
        doc = Nodes()
        doc.feed((ROOT / "ui/explorer/explorer.html").read_text())
        self.assertEqual(len(doc.ids), len(set(doc.ids)), "duplicate DOM id")
        for i in range(22):
            for prefix in ("row", "name", "size", "modified"):
                self.assertIn(f"{prefix}-{i}", doc.ids)
            self.assertIn(f"explorer.row.{i}", doc.actions)
        for i in range(15):
            self.assertIn(f"place-{i}", doc.ids)
            self.assertIn(f"explorer.place.{i}", doc.actions)
        for id in ("progress-popup", "context-popup", "address", "status-message", "job-progress", "page-label"):
            self.assertIn(id, doc.ids)
        for action in ("explorer.back", "explorer.forward", "explorer.up", "explorer.reload", "explorer.search", "explorer.address", "explorer.tab.new", "explorer.tab.close"):
            self.assertIn(action, doc.actions)

    def test_strict_css_properties_have_known_compiler_owners(self):
        css = (ROOT / "ui/explorer/explorer.css").read_text()
        source = (ROOT / "crates/flamewm-render-compiler/src/compile.rs").read_text()
        declarations = []
        for body in re.findall(r"\{([^{}]*)\}", css):
            for decl in body.split(";"):
                if ":" in decl:
                    declarations.append(decl.split(":", 1)[0].strip())
        self.assertTrue(declarations)
        for prop in declarations:
            if prop.startswith("--"):
                continue
            self.assertRegex(source, '"' + re.escape(prop) + '"', "unknown CSS property " + prop)
        self.assertNotIn("text-align:", css)
        self.assertNotIn("border-bottom:", css)

    def test_worker_carries_generation_and_no_file_io_in_projection(self):
        main = (ROOT / "crates/flamewm-explorer/src/main.rs").read_text()
        model = (ROOT / "crates/flamewm-explorer/src/lib.rs").read_text()
        self.assertIn("WorkerJob::Scan{generation,path,hidden,sort}", main)
        self.assertIn("generation!=self.generation", model)
        self.assertIn("UnixStream::pair()", main)
        self.assertIn("run_with_controller_events_role_and_two_readable_fds", main)
        self.assertIn("fn project(&self", main)
        self.assertIn("flamewm_ui_core::context_menu::MenuMetrics::canonical()", main)
        self.assertIn("Trash::move_to_trash", main)
        self.assertIn("create_new_folder", main)
        self.assertNotIn("std::process::Command::new(\"sh\")", main)

    def test_packages_and_xdg_default_are_wired(self):
        package = (ROOT / "scripts/package").read_text()
        manifest = (ROOT / "packaging/install-manifest.txt").read_text()
        session = (ROOT / "crates/flamewm-wm/src/session_app_compat.rs").read_text()
        entry = (ROOT / "packaging/flamewm-explorer.desktop").read_text()
        self.assertIn("flamewm-explorer", package)
        self.assertIn("flamewm-explorer.desktop", manifest)
        self.assertIn("MimeType=inode/directory;", entry)
        self.assertIn('defaults.insert("inode/directory"', session)
        self.assertIn('desktop_id_installed(data_dirs, "flamewm-explorer.desktop")', session)
        self.assertIn('data-action="settings.default-file-manager.cycle"', (ROOT / "ui/settings.html").read_text())
        self.assertIn('SettingsAction::FileManagerCycle', (ROOT / "crates/flamewm-settings/src/lib.rs").read_text())

    def test_safe_operations_and_local_xdg_directory_handler(self):
        source = (ROOT / "crates/flamewm-explorer/src/lib.rs").read_text()
        manifest = (ROOT / "crates/flamewm-explorer/Cargo.toml").read_text()
        entry = (ROOT / "packaging/flamewm-explorer.desktop").read_text()
        self.assertIn("rustix::fs::RenameFlags::NOREPLACE", source)
        self.assertIn('rustix = { workspace = true }', manifest)
        self.assertIn('Exec=flamewm-explorer %f', entry)
        self.assertIn('remove_file(to)', source)
        self.assertIn('remove_dir_all(to)', source)

    def test_theme_uses_existing_snapshot_instead_of_private_config(self):
        source=(ROOT / "crates/flamewm-explorer/src/main.rs").read_text()
        self.assertIn("ControlSignal::SettingsSnapshotChanged", source)
        self.assertIn("ControlRequest::GetSettings", source)
        self.assertIn("ExplorerTheme::from_snapshot", source)

if __name__ == "__main__": unittest.main()
