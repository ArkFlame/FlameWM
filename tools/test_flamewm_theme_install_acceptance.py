"""Pure unit tests for the theme install acceptance runner. No X server or display is touched.

Run with:
    python3 -I tools/test_flamewm_theme_install_acceptance.py
"""

from __future__ import annotations

import os
import struct
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


class CursorFixtureTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def test_xcursor_bytes_follow_the_xcursor_layout_read_by_cursor_rs(self) -> None:
        data = ti.xcursor_bytes()
        magic, header_len, version, ntoc = struct.unpack_from("<4sIII", data, 0)
        self.assertEqual((magic, header_len, ntoc), (b"Xcur", 16, 1))
        toc_type, toc_size, toc_pos = struct.unpack_from("<III", data, 16)
        self.assertEqual((toc_type, toc_size, toc_pos), (0xFFFD0002, 32, 28))
        chunk = struct.unpack_from("<9I", data, toc_pos)
        self.assertEqual(chunk[:2], (36, 0xFFFD0002))
        self.assertEqual(chunk[4:6], (32, 32))  # width, height
        self.assertEqual(len(data), toc_pos + 36 + 32 * 32 * 4)
        self.assertEqual(data[toc_pos + 36:toc_pos + 40], bytes((220, 130, 60, 255)))  # B, G, R, A

    def test_cursor_index_is_a_cursor_theme_not_an_icon_theme(self) -> None:
        text = ti.cursor_index_text(ti.CURSOR_ID)
        self.assertIn(f"Name={ti.CURSOR_ID}", text)
        self.assertNotIn("Directories=", text)

    def test_cursor_folder_has_a_left_ptr_xcursor_and_an_arrow_link(self) -> None:
        folder = ti.write_cursor_folder(self.root / ti.CURSOR_ID, ti.CURSOR_ID)
        self.assertEqual((folder / "cursors" / ti.CURSOR_FILE).read_bytes()[:4], b"Xcur")
        self.assertTrue((folder / "cursors" / "arrow").is_symlink())
        self.assertEqual(os.readlink(folder / "cursors" / "arrow"), ti.CURSOR_FILE)

    def test_escape_cursor_folder_links_left_ptr_to_an_absolute_host_path(self) -> None:
        folder = ti.write_cursor_folder(self.root / ti.CURSOR_ESCAPE_ID, ti.CURSOR_ESCAPE_ID, escape=True)
        link = folder / "cursors" / ti.CURSOR_FILE
        self.assertTrue(link.is_symlink())
        self.assertEqual(os.readlink(link), "/etc/passwd")

    def test_corrupt_cursor_fixture_keeps_the_magic_but_is_too_short_to_decode(self) -> None:
        folder = ti.write_cursor_folder(self.root / ti.CURSOR_CORRUPT_ID, ti.CURSOR_CORRUPT_ID,
                                        left_ptr=ti.xcursor_bytes()[:40])
        data = (folder / "cursors" / ti.CURSOR_FILE).read_bytes()
        self.assertEqual(data[:4], b"Xcur")
        self.assertLess(len(data), 64)

    def test_preview_counts_must_reach_the_threshold_and_be_on_screen(self) -> None:
        self.assertFalse(ti.preview_visible(None))
        self.assertFalse(ti.preview_visible(0))
        self.assertFalse(ti.preview_visible(ti.PREVIEW_MIN_PIXELS - 1))
        self.assertTrue(ti.preview_visible(ti.PREVIEW_MIN_PIXELS))

    def test_preview_crop_sits_left_of_the_row_name(self) -> None:
        x, y, width, height = ti.preview_rect((300, 200))
        self.assertLess(x + width, 300)
        self.assertLessEqual(y, 200)
        self.assertGreaterEqual(y + height, 200)


class CursorLocationTests(unittest.TestCase):
    def test_cursor_title_is_the_word_above_its_installed_note(self) -> None:
        idle = [("icon", 300, 103), ("icon", 254, 276), ("installed", 254, 294), ("cursor", 253, 324),
                ("installed", 254, 343), ("cursor", 300, 343)]
        self.assertEqual(ti.icon_title_point(idle, "cursor"), (253, 324))
        self.assertIsNone(ti.icon_title_point([("cursor", 253, 343), ("installed", 254, 343)], "cursor"))

    def test_cursor_picker_row_and_status_use_the_cursor_theme_id(self) -> None:
        status = [("installed", 254, 395), (ti.normalize_word(ti.CURSOR_ID), 299, 395)]
        self.assertTrue(ti.installed_status_seen(status, ti.CURSOR_ID))
        self.assertFalse(ti.installed_status_seen(status))  # the icon id does not match the cursor status
        picker = [("ap-test-cursors", 300, 200), ("installed", 254, 395)]
        self.assertEqual(ti.picker_row_point(picker, ti.CURSOR_ID), (300, 200))
        self.assertIsNone(ti.picker_row_point(picker))


class ArgumentTests(unittest.TestCase):
    def test_geometry_must_be_wxh(self) -> None:
        with self.assertRaises(SystemExit):
            ti.parse_args(["--run-dir", "/tmp/x", "--geometry", "1280by800"])
        args = ti.parse_args(["--run-dir", "/tmp/x", "--geometry", "1280x800"])
        self.assertEqual((args.screen_w, args.screen_h), (1280, 800))


if __name__ == "__main__":
    unittest.main()
