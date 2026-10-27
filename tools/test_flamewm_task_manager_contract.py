"""Static integration checks, not a substitute for Rust compilation or live UI."""
from pathlib import Path
import re
import tomllib
import unittest
from html.parser import HTMLParser

ROOT = Path(__file__).resolve().parents[1]

class Ids(HTMLParser):
    def __init__(self):
        super().__init__()
        self.ids = []
        self.actions = []
    def handle_starttag(self, tag, attrs):
        data = dict(attrs)
        if "id" in data: self.ids.append(data["id"])
        if "data-action" in data: self.actions.append(data["data-action"])

class TaskManagerSourceContract(unittest.TestCase):
    def test_template_identifiers_are_unique_and_complete(self):
        root = Ids()
        root.feed((ROOT / "ui/task-manager/task-manager.html").read_text())
        self.assertEqual(len(root.ids), len(set(root.ids)))
        for i in range(22):
            self.assertIn(f"row-{i}", root.ids)
            self.assertIn(f"r-{i}-name", root.ids)
            self.assertIn(f"tm.row.{i}", root.actions)
        for i in range(16):
            self.assertIn(f"core-{i}", root.ids)
            self.assertIn(f"core-pct-{i}", root.ids)
        for action in ["tm.sort.cpu", "tm.sort.mem", "tm.sort.pid", "tm.search", "tm.terminate", "tm.confirm-terminate", "tm.cancel"]:
            self.assertIn(action, root.actions)
    def test_shared_surfaces_and_compile_error_fields(self):
        ctx = (ROOT / "crates/flamewm-ui-core/src/context_menu.rs").read_text()
        skin = (ROOT / "crates/flamewm-skin/src/recipes/decoration.rs").read_text()
        app = (ROOT / "crates/flamewm-task-manager/src/lib.rs").read_text()
        self.assertIn("surface_stroke", ctx)
        self.assertIn("pub surface_border_rgb", skin)
        self.assertIn("pub surface_border_alpha", skin)
        self.assertIn("project_surface_border", app)
    def test_priority_and_identity_are_from_procfs(self):
        model = (ROOT / "crates/flamewm-task-manager/src/model.rs").read_text()
        self.assertIn("priority: fields[15].parse()", model)
        self.assertIn("nice: fields[16].parse()", model)
        self.assertIn("start: fields[19].parse()", model)
        self.assertIn("check_identity(pid, start)", model)
    def test_installation_and_multicall_wiring(self):
        cargo = tomllib.loads((ROOT / "crates/flamewm-apps/Cargo.toml").read_text())
        self.assertIn("flamewm-task-manager", cargo["dependencies"])
        for filename in ["scripts/xephyr", "scripts/package", "packaging/install-manifest.txt", "crates/flamewm-apps/src/main.rs"]:
            self.assertIn("flamewm-task-manager", (ROOT / filename).read_text(), filename)
        self.assertTrue((ROOT / "packaging/flamewm-task-manager.desktop").is_file())
    def test_initial_viewport_matches_fixed_header(self):
        source = (ROOT / "crates/flamewm-task-manager/src/lib.rs").read_text()
        self.assertIn("viewport: visible_rows(HEIGHT)", source)
        self.assertIn("app.viewport = visible_rows(*height)", source)

if __name__ == "__main__": unittest.main()
