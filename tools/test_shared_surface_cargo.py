"""Regression checks for extracted archives containing newer shared-surface APIs."""
from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path
from subprocess import CompletedProcess

sys.path.insert(0, str(Path(__file__).resolve().parent))
from ensure_shared_surface_cargo import CRATES, UI_INPUTS, UI_TREES, SHARED_UI_INPUTS, run


class CargoFixture:
    def __init__(self, root: Path):
        self.root = root
        self.cleaned = []
        self.target = root / "target"
        self.metadata = {
            "workspace_root": str(root),
            "target_directory": str(self.target),
            "packages": [
                {"name": crate, "manifest_path": str(root / "crates" / crate / "Cargo.toml")}
                for crate in CRATES
            ],
        }

    def __call__(self, args, **kwargs):
        if args[1] == "metadata":
            import json
            return CompletedProcess(args, 0, json.dumps(self.metadata), "")
        assert args[1:3] == ["clean", "-p"]
        assert args[-1] == "--release"
        self.cleaned.append(args[3])
        return CompletedProcess(args, 0, "", "")


class SharedSurfaceCargoTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        for crate in CRATES:
            directory = self.root / "crates" / crate
            (directory / "src").mkdir(parents=True)
            (directory / "Cargo.toml").write_text("[package]\nname = '" + crate + "'\n")
            (directory / "src/lib.rs").write_text("pub fn current() {}\n")
        sources = list(UI_INPUTS)
        sources.extend(f"{folder}/sample.html" for folder in UI_TREES.values())
        sources.extend(rel for names in SHARED_UI_INPUTS.values() for rel in names)
        for ui in sources:
            path = self.root / ui
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("fixture\n")
        self.cargo = CargoFixture(self.root)

    def test_initial_clean_then_noop_then_changed_only(self):
        self.assertEqual(run(self.root, executor=self.cargo), list(CRATES))
        self.assertEqual(run(self.root, executor=self.cargo), [])
        self.assertEqual(self.cargo.cleaned, list(CRATES))
        core = self.root / "crates/flamewm-ui-core/src/surface.rs"
        core.write_text("pub fn surface() {}\n")
        self.assertEqual(run(self.root, executor=self.cargo), ["flamewm-ui-core"])
        ui = self.root / UI_INPUTS[1]
        ui.write_text("new markup")
        self.assertEqual(run(self.root, executor=self.cargo), ["flamewm-task-manager"])
        self.assertEqual(self.cargo.cleaned.count("flamewm-ui-x11"), 1)
        self.assertEqual(self.cargo.cleaned.count("flamewm-task-manager"), 2)

    def test_pass17_shell_css_and_template_changes_invalidate_shell_only(self):
        self.assertEqual(run(self.root, executor=self.cargo), list(CRATES))
        for file in ("ui/shell/sample.html", "ui/shared/menu.css"):
            target = self.root / file
            target.write_text(target.read_text() + "new ui input\n")
            self.assertEqual(run(self.root, executor=self.cargo), ["flamewm-shell"])
        self.assertEqual(self.cargo.cleaned.count("flamewm-shell"), 3)

    def test_pass17_terminal_pool_and_html_changes_invalidate_terminal_only(self):
        self.assertEqual(run(self.root, executor=self.cargo), list(CRATES))
        for file in ("crates/flamewm-terminal/src/lib.rs", "ui/terminal/sample.html"):
            target = self.root / file
            target.write_text(target.read_text() + "new input\n")
            self.assertEqual(run(self.root, executor=self.cargo), ["flamewm-terminal"])

    def test_pass17_frame_and_renderer_changes_invalidate_only_respective_crates(self):
        self.assertEqual(run(self.root, executor=self.cargo), list(CRATES))
        for crate in ("flamewm-wm-x11", "flamewm-render-x11", "flamewm-render-core"):
            file = self.root / "crates" / crate / "src/lib.rs"
            file.write_text(file.read_text() + "// repaint\n")
            self.assertEqual(run(self.root, executor=self.cargo), [crate])

    def test_archived_backdated_sources_still_invalidate_by_content(self):
        import os
        self.assertEqual(run(self.root, executor=self.cargo), list(CRATES))
        source = self.root / "ui/shell/sample.html"
        stamp = source.stat().st_mtime
        source.write_text("changed source, but restored archive timestamp")
        os.utime(source, (stamp - 86400, stamp - 86400))
        self.assertEqual(run(self.root, executor=self.cargo), ["flamewm-shell"])
        self.assertEqual(run(self.root, executor=self.cargo), [])

    def test_missing_shell_ui_folder_does_not_clean_anything(self):
        (self.root / "ui/shell/sample.html").unlink()
        (self.root / "ui/shell").rmdir()
        with self.assertRaisesRegex(RuntimeError, "Missing UI input directory"):
            run(self.root, executor=self.cargo)
        self.assertEqual(self.cargo.cleaned, [])

    def test_failed_metadata_does_not_clean(self):
        self.cargo.metadata["workspace_root"] = "/untrusted"
        with self.assertRaisesRegex(RuntimeError, "workspace root"):
            run(self.root, executor=self.cargo)
        self.assertEqual(self.cargo.cleaned, [])

    def test_missing_inputs_do_not_mutate_cargo(self):
        (self.root / UI_INPUTS[1]).unlink()
        with self.assertRaisesRegex(RuntimeError, "Missing source input"):
            run(self.root, executor=self.cargo)
        self.assertEqual(self.cargo.cleaned, [])

    def test_failed_clean_does_not_stamp_completion(self):
        class FailClean(CargoFixture):
            def __call__(self, args, **kwargs):
                if args[1] == "clean":
                    return CompletedProcess(args, 1, "", "")
                return super().__call__(args, **kwargs)
        with self.assertRaisesRegex(RuntimeError, "cargo clean failed"):
            run(self.root, executor=FailClean(self.root))
        state = self.root / "target/flamewm"
        self.assertFalse(any(state.glob("*.sha256")) if state.exists() else False)


if __name__ == "__main__":
    unittest.main()
