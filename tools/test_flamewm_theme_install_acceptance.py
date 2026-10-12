"""Pure unit tests for the theme install acceptance runner. No X server or display is touched.

Run with:
    python3 -I tools/test_flamewm_theme_install_acceptance.py
"""

from __future__ import annotations

import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_theme_install_acceptance as ti  # noqa: E402


class FixtureTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def test_folder_and_archive_carry_the_same_named_theme(self) -> None:
        folder = ti.write_theme_folder(self.root / "src" / ti.THEME_ID, ti.THEME_ID)
        self.assertIn(f"Name={ti.THEME_ID}", (folder / "index.theme").read_text(encoding="utf-8"))
        self.assertTrue((folder / "apps" / "48" / ti.THEME_FILE).is_file())
        archive = ti.write_theme_archive(folder, self.root / f"{ti.THEME_ID}.tar.gz")
        with tarfile.open(archive, "r:gz") as tar:
            names = tar.getnames()
        self.assertIn(f"{ti.THEME_ID}/index.theme", names)
        self.assertIn(f"{ti.THEME_ID}/apps/48/{ti.THEME_FILE}", names)

    def test_escape_fixture_is_a_real_symlink_leaving_the_tree(self) -> None:
        outside = self.root / "outside.png"
        outside.write_bytes(b"x")
        escape = ti.write_theme_folder(self.root / ti.ESCAPE_ID, ti.ESCAPE_ID, escape_target=outside)
        link = escape / "apps" / "48" / "escape.png"
        self.assertTrue(link.is_symlink())
        self.assertEqual(link.resolve(), outside.resolve())
        self.assertFalse(link.resolve().is_relative_to(escape.resolve()))

    def test_corrupt_archive_is_not_readable_as_tar_gz(self) -> None:
        corrupt = ti.write_corrupt_archive(self.root / f"{ti.CORRUPT_ID}.tar.gz")
        with self.assertRaises((tarfile.TarError, OSError, EOFError)):
            with tarfile.open(corrupt, "r:gz") as tar:
                tar.getnames()

    def test_installed_tree_comparison_rejects_missing_or_changed_files(self) -> None:
        source = ti.write_theme_folder(self.root / "src", ti.THEME_ID)
        installed = self.root / "installed"
        self.assertFalse(ti.installed_matches(source, installed))
        installed.mkdir()
        (installed / "index.theme").write_text((source / "index.theme").read_text(encoding="utf-8"), encoding="utf-8")
        (installed / "apps" / "48").mkdir(parents=True)
        (installed / "apps" / "48" / ti.THEME_FILE).write_bytes(b"different")
        self.assertFalse(ti.installed_matches(source, installed))
        (installed / "apps" / "48" / ti.THEME_FILE).write_bytes((source / "apps" / "48" / ti.THEME_FILE).read_bytes())
        self.assertTrue(ti.installed_matches(source, installed))


class ButtonLocationTests(unittest.TestCase):
    def test_ocr_tokens_drop_trailing_ellipsis(self) -> None:
        self.assertEqual(ti.normalize_word("folder..."), "folder")
        self.assertEqual(ti.normalize_word("Folder\u2026"), "folder")
        self.assertEqual(ti.normalize_word("AP-Test-Icons"), "ap-test-icons")

    def test_folder_label_on_the_install_row_is_chosen_over_the_status_note(self) -> None:
        centres = [
            ("archive", 80, 300),      # status note "Archive (.tar.gz, .tar.xz) or folder" on another row
            ("folder", 300, 300),
            ("install", 150, 340),     # "Install theme" title row
            ("install", 420, 420),     # "Install archive…" button row
            ("folder", 560, 420),      # "Install folder…" button on that row (OCR drops the ellipsis)
        ]
        self.assertEqual(ti.locate_pair(centres, ti.FOLDER_WORDS), (560, 420))

    def test_no_anchor_on_the_same_row_means_no_click_target(self) -> None:
        centres = [("folder", 560, 420), ("install", 420, 200)]
        self.assertIsNone(ti.locate_pair(centres, ti.FOLDER_WORDS))

    def test_archive_button_is_located_separately_from_folder(self) -> None:
        centres = [("install", 300, 420), ("archive", 380, 420), ("install", 500, 420), ("folder", 600, 420)]
        self.assertEqual(ti.locate_pair(centres, ti.ARCHIVE_WORDS), (380, 420))
        self.assertEqual(ti.locate_pair(centres, ti.FOLDER_WORDS), (600, 420))


class StatusTests(unittest.TestCase):
    def test_bare_installed_note_does_not_count_as_an_install(self) -> None:
        idle = [("installed", 254, 291), ("installed", 254, 343), ("install", 253, 375)]
        self.assertFalse(ti.installed_status_seen(idle))

    def test_installed_name_on_the_status_row_counts(self) -> None:
        status = [("installed", 254, 395), ("ap-test-icons", 299, 395), ("installed", 254, 291)]
        self.assertTrue(ti.installed_status_seen(status))
        self.assertFalse(ti.installed_status_seen([("installed", 254, 395), ("ap-test-icons", 299, 500)]))

    def test_failed_status_is_detected_by_its_word(self) -> None:
        self.assertTrue(ti.failed_status_seen([("failed", 300, 395)]))
        self.assertFalse(ti.failed_status_seen([("installed", 300, 395)]))


class PickerLocationTests(unittest.TestCase):
    def test_icon_title_is_the_word_above_its_installed_note_not_the_subtitle(self) -> None:
        idle = [("icon", 300, 103), ("icon", 254, 276), ("installed", 254, 294), ("cursor", 253, 324),
                ("installed", 254, 343)]
        self.assertEqual(ti.icon_title_point(idle), (254, 276))
        self.assertIsNone(ti.icon_title_point([("icon", 300, 103)]))

    def test_picker_row_ignores_the_installed_status_line(self) -> None:
        status_only = [("ap-test-icons", 299, 395), ("installed", 254, 395)]
        self.assertIsNone(ti.picker_row_point(status_only))
        picker = status_only + [("ap-test-icons", 300, 200)]
        self.assertEqual(ti.picker_row_point(picker), (300, 200))


class VerdictTests(unittest.TestCase):
    def test_any_missing_observation_is_not_run_not_pass(self) -> None:
        status, reason = ti.expected_entry_status({"a": True, "b": None}, ("a", "b"))
        self.assertEqual(status, ti.NOT_RUN)
        self.assertIn("b", reason)

    def test_a_false_observation_fails_even_with_others_missing(self) -> None:
        status, _ = ti.expected_entry_status({"a": False, "b": None}, ("a", "b"))
        self.assertEqual(status, ti.FAIL)

    def test_all_true_observations_pass(self) -> None:
        self.assertEqual(ti.expected_entry_status({"a": True}, ("a",))[0], ti.PASS)

    def test_document_exit_codes_are_fail_closed(self) -> None:
        all_pass = {sid: ti.entry(sid, ti.PASS, "ok") for sid in ti.IDS}
        self.assertEqual(ti.compose_document(all_pass)["exit_code"], 0)
        with_fail = dict(all_pass, TI04=ti.entry("TI04", ti.FAIL, "installed escape"))
        self.assertEqual(ti.compose_document(with_fail)["exit_code"], 1)
        missing = {"TI01": ti.entry("TI01", ti.PASS, "ok")}
        document = ti.compose_document(missing)
        self.assertEqual(document["exit_code"], 3)
        self.assertEqual(document["summary"][ti.NOT_RUN], len(ti.IDS) - 1)

    def test_every_theme_id_appears_once_in_order(self) -> None:
        document = ti.compose_document({})
        self.assertEqual([e["id"] for e in document["results"]], list(ti.IDS))


class ArgumentTests(unittest.TestCase):
    def test_geometry_must_be_wxh(self) -> None:
        with self.assertRaises(SystemExit):
            ti.parse_args(["--run-dir", "/tmp/x", "--geometry", "1280by800"])
        args = ti.parse_args(["--run-dir", "/tmp/x", "--geometry", "1280x800"])
        self.assertEqual((args.screen_w, args.screen_h), (1280, 800))


if __name__ == "__main__":
    unittest.main()
