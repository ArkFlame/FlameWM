"""Pure unit tests for the settings acceptance schema and XTEST held-key option.

No X server, Xephyr, or display is touched. Run with:
    python3 -I tools/test_flamewm_settings_acceptance_schema.py
"""

from __future__ import annotations

import subprocess
import sys
import shutil
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import flamewm_settings_acceptance as acc  # noqa: E402
import flamewm_xtest as xt  # noqa: E402


# Independent copy of the canonical matrix. The harness must match it exactly.
EXPECTED_MATRIX = (
    ("SS00", "source/git preservation"),
    ("SS01", "seven pages open"),
    ("SS02", "seven pages x 760x500/820x540/960x640"),
    ("SS03", "accent"),
    ("SS04", "icon theme"),
    ("SS05", "wallpaper"),
    ("SS06", "sticky notes/opacities"),
    ("SS07", "taskbar"),
    ("SS08", "displays 1/2/4/5+ outputs and modes"),
    ("SS09", "15s rollback"),
    ("SS10", "scale"),
    ("SS11", "fonts"),
    ("SS12", "hotkeys"),
    ("SS13", "about"),
    ("SS14", "external signal + auto-revert"),
    ("SS15", "traversal/disabled states"),
    ("SS16", "cargo/CSS/architecture gates"),
    ("SS17", "private XTEST+screenshot matrix"),
    ("SS18", "installed-prefix release"),
    ("SS19", "stability/perf/PID cleanup"),
)

# Wording that the legacy runner used for the wrong canonical IDs.
LEGACY_WRONG_LABELS = {
    "SS03": "Displays one output",
    "SS06": "hotkey",
    "SS19": "PID cleanup",
}

LEGACY_IDS = ("NATIVE_ONE_OUTPUT", "NATIVE_HOTKEY_INPUT", "NATIVE_PID_CLEANUP")
STATUSES = ("PASS", "FAIL", "NOT_RUN", "ENVIRONMENT_BLOCKED")


def _labels(matrix) -> dict[str, str]:
    return dict(matrix)


class CanonicalMatrixTests(unittest.TestCase):
    def test_matrix_has_twenty_ordered_ids(self) -> None:
        ids = [sid for sid, _ in acc.CANONICAL_MATRIX]
        self.assertEqual(ids, [f"SS{n:02d}" for n in range(20)])

    def test_canonical_labels_exact(self) -> None:
        self.assertEqual(tuple(acc.CANONICAL_MATRIX), EXPECTED_MATRIX)

    def test_ss03_is_accent_not_one_output(self) -> None:
        label = _labels(acc.CANONICAL_MATRIX)["SS03"]
        self.assertNotEqual(label, LEGACY_WRONG_LABELS["SS03"])
        self.assertEqual(label, "accent")

    def test_ss06_is_sticky_notes_not_hotkey(self) -> None:
        label = _labels(acc.CANONICAL_MATRIX)["SS06"]
        self.assertNotEqual(label, LEGACY_WRONG_LABELS["SS06"])
        self.assertEqual(label, "sticky notes/opacities")

    def test_ss19_is_stability_perf_not_pid_cleanup(self) -> None:
        label = _labels(acc.CANONICAL_MATRIX)["SS19"]
        self.assertNotEqual(label, LEGACY_WRONG_LABELS["SS19"])
        self.assertEqual(label, "stability/perf/PID cleanup")

    def test_no_canonical_label_reuses_legacy_wrong_wording(self) -> None:
        labels = _labels(acc.CANONICAL_MATRIX)
        for sid, wrong in LEGACY_WRONG_LABELS.items():
            self.assertNotEqual(labels[sid], wrong, f"{sid} relabeled as {wrong!r}")

    def test_sizes_include_760x500(self) -> None:
        self.assertIn((760, 500), acc.SIZES)
        self.assertIn((960, 640), acc.SIZES)


class LegacyLabelTests(unittest.TestCase):
    def test_legacy_ids_are_descriptive(self) -> None:
        ids = tuple(lid for lid, _, _ in acc.LEGACY_SUBSCENARIOS)
        self.assertEqual(ids, LEGACY_IDS + ("NATIVE_PERSISTENCE_RESTART",))

    def test_legacy_results_are_labelled_legacy_harness(self) -> None:
        entry = acc.result_entry("NATIVE_ONE_OUTPUT", "PASS")
        self.assertEqual(entry["report_label"], "LEGACY_HARNESS_NATIVE_ONE_OUTPUT")
        self.assertTrue(entry["title"].startswith("LEGACY_HARNESS_NATIVE_ONE_OUTPUT"))

    def test_canonical_results_keep_canonical_report_label(self) -> None:
        entry = acc.result_entry("SS00", "PASS")
        self.assertEqual(entry["report_label"], "SS00")


class StatusVocabularyTests(unittest.TestCase):
    def test_status_vocabulary_is_exactly_four_distinct_values(self) -> None:
        self.assertEqual(
            (acc.STATUS_PASS, acc.STATUS_FAIL, acc.STATUS_NOT_RUN, acc.STATUS_ENV_BLOCKED),
            STATUSES,
        )
        self.assertEqual(len(set(STATUSES)), 4)

    def test_unimplemented_canonical_is_not_run_not_environment_blocked(self) -> None:
        document = acc.compose_document({})
        by_id = {s["id"]: s for s in document["scenarios"]}
        self.assertEqual(by_id["SS04"]["status"], "NOT_RUN")

    def test_partial_legacy_pass_never_passes_canonical(self) -> None:
        document = acc.compose_document({"NATIVE_HOTKEY_INPUT": acc.result_entry("NATIVE_HOTKEY_INPUT", "PASS")})
        by_id = {s["id"]: s for s in document["scenarios"]}
        self.assertEqual(by_id["SS12"]["status"], "NOT_RUN")
        self.assertIn("NATIVE_HOTKEY_INPUT", by_id["SS12"]["reason"])

    def test_legacy_fail_propagates_to_canonical(self) -> None:
        document = acc.compose_document({"NATIVE_PID_CLEANUP": acc.result_entry("NATIVE_PID_CLEANUP", "FAIL", "left pids")})
        by_id = {s["id"]: s for s in document["scenarios"]}
        self.assertEqual(by_id["SS19"]["status"], "FAIL")
        self.assertEqual(document["exit_code"], 1)


class VerdictTests(unittest.TestCase):
    def test_exit_zero_never_means_ready(self) -> None:
        everything_pass = {sid: acc.result_entry(sid, "PASS") for sid, _ in acc.CANONICAL_MATRIX}
        document = acc.compose_document(everything_pass)
        self.assertEqual(document["summary"]["PASS"], 20)
        self.assertEqual(document["exit_code"], 0)
        self.assertNotEqual(document["verdict"], "READY")
        self.assertNotIn("READY", document["verdict"])

    def test_incomplete_run_exit_zero_not_ready(self) -> None:
        document = acc.compose_document({"SS00": acc.result_entry("SS00", "PASS")})
        self.assertEqual(document["exit_code"], 0)
        self.assertEqual(document["verdict"], "INCOMPLETE")

    def test_any_fail_exit_one(self) -> None:
        document = acc.compose_document({"SS01": acc.result_entry("SS01", "FAIL", "page died")})
        self.assertEqual(document["exit_code"], 1)
        self.assertEqual(document["verdict"], "FAIL")

    def test_summary_counts_cover_all_four_statuses(self) -> None:
        document = acc.compose_document({"SS00": acc.result_entry("SS00", "PASS")})
        self.assertEqual(set(document["summary"]), set(STATUSES))
        self.assertEqual(sum(document["summary"].values()), 20)

    def test_unimplemented_semantic_gates_remain_not_run(self) -> None:
        document = acc.compose_document({"SS01": acc.result_entry("SS01", "PASS"),
                                         "SS02": acc.result_entry("SS02", "PASS")})
        by_id = {scenario["id"]: scenario for scenario in document["scenarios"]}
        for sid in ("SS03", "SS04", "SS05", "SS06", "SS07", "SS08", "SS09", "SS10",
                    "SS11", "SS12", "SS13", "SS14", "SS15", "SS16", "SS17", "SS18", "SS19"):
            with self.subTest(sid=sid):
                self.assertEqual(by_id[sid]["status"], "NOT_RUN")
        self.assertEqual(document["verdict"], "INCOMPLETE")


class WindowGeometryGateTests(unittest.TestCase):
    def test_window_tree_uses_settings_client_not_decorated_frame(self) -> None:
        tree = '''\
xwininfo: Window id: 0x3b8 (the root window) (has no name)

  Root window id: 0x3b8 (the root window) (has no name)
     1 child:
     0x400076 (has no name): () 760x531+0+0  +0+0
        1 child:
        0x400077 "FlameWM Settings": ("flamewm-settings" "FlameWM") 760x500+0+31  +0+31
'''
        geometry = acc.parse_window_tree(tree)
        self.assertEqual(geometry, {
            "name": "FlameWM Settings", "width": 760, "height": 500, "x": 0, "y": 31
        })

    def test_window_tree_without_named_settings_client_is_unknown(self) -> None:
        tree = '  0x400076 (has no name): () 760x531+0+0\n'
        self.assertIsNone(acc.parse_window_tree(tree))

    def test_exact_size_needs_live_nonblank_capture_and_measured_client(self) -> None:
        valid = {"alive": True, "captured": True, "nonblank": True,
                 "window": {"width": 760, "height": 500}}
        self.assertTrue(acc.exact_window_size(valid, (760, 500)))
        for field, value in (("alive", False), ("captured", False), ("nonblank", False),
                             ("nonblank", None), ("window", None),
                             ("window", {"width": 1280, "height": 800}),
                             ("window", {"width": 760, "height": 540})):
            with self.subTest(field=field, value=value):
                mutated = dict(valid)
                mutated[field] = value
                self.assertFalse(acc.exact_window_size(mutated, (760, 500)))


class DesktopOpacityFixtureTests(unittest.TestCase):
    def test_fixture_changes_value_within_authoritative_range(self) -> None:
        for current in (0, 20, 21, 60):
            with self.subTest(current=current):
                next_value = acc.next_desktop_opacity(current)
                self.assertNotEqual(next_value, current)
                self.assertGreaterEqual(next_value, 0)
                self.assertLessEqual(next_value, 60)

    def test_fixture_rejects_out_of_range_snapshot(self) -> None:
        for current in (-1, 61, 63, 100):
            with self.subTest(current=current):
                with self.assertRaises(ValueError):
                    acc.next_desktop_opacity(current)


class HotkeyLeaseScenarioTests(unittest.TestCase):
    def test_complete_observations_required_for_ss12_pass(self) -> None:
        observations = {
            "lease_begin_end_same_owner": True,
            "binding_stored": True,
            "action_effect": True,
            "grab_restored_start_behavior": True,
            "owner_loss_restored": True,
            "clear_reset_persisted": True,
        }
        self.assertEqual(acc.assess_ss12(observations)[0], "PASS")

    def test_missing_owner_loss_or_persistence_stays_not_run(self) -> None:
        observations = {
            "lease_begin_end_same_owner": True,
            "binding_stored": True,
            "action_effect": True,
            "grab_restored_start_behavior": True,
            "owner_loss_restored": None,
            "clear_reset_persisted": None,
        }
        status, reason = acc.assess_ss12(observations)
        self.assertEqual(status, "NOT_RUN")
        self.assertIn("owner_loss_restored", reason)
        self.assertIn("clear_reset_persisted", reason)

    def test_measured_missing_action_effect_fails(self) -> None:
        observations = {key: True for key in (
            "lease_begin_end_same_owner", "binding_stored", "action_effect",
            "grab_restored_start_behavior", "owner_loss_restored", "clear_reset_persisted")}
        observations["action_effect"] = False
        self.assertEqual(acc.assess_ss12(observations)[0], "FAIL")

    def test_malformed_or_missing_snapshot_cannot_prove_binding(self) -> None:
        self.assertIsNone(acc.gvariant_field_string("garbage", "ToggleStartMenu"))
        self.assertIsNone(acc.gvariant_field_string("{'revision': <uint64 4>}", "ToggleStartMenu"))

    def test_lease_call_parser_requires_same_observed_sender_for_driver(self) -> None:
        log = ("method call sender=:1.7 -> dest=:1.0; interface=com.arkflame.FlameWM1.Shortcuts; member=BeginCapture\n"
               "method call sender=:1.7 -> dest=:1.0; interface=com.arkflame.FlameWM1.Shortcuts; member=EndCapture\n")
        calls = acc.capture_lease_calls(log)
        self.assertEqual(calls, [("BeginCapture", ":1.7"), ("EndCapture", ":1.7")])


class AddedNativeScenarioContractTests(unittest.TestCase):
    def test_ss05_choose_coordinate_is_derived_from_measured_client_origin(self) -> None:
        point = tuple(int(part) for part in acc.parse_args([
            "--run-dir", "/tmp/unused", "--settings-bin", "settings", "--wm-bin", "wm"
        ]).ss05_click.split(","))
        self.assertEqual(point, (755, 195))
        self.assertEqual(acc.ss05_choose_screen_point(
            {"x": 0, "y": 31, "width": 960, "height": 640}, point), (755, 226))
        self.assertEqual(acc.ss05_choose_screen_point(
            {"x": 17, "y": 43, "width": 960, "height": 640}, point), (772, 238))

    def test_ss14_reads_real_monitor_filename_signals_and_dynamic_row_pixel(self) -> None:
        fixture = ("method call sender=:1.8 -> dest=:1.0; interface=com.arkflame.FlameWM1.Settings; member=Apply\n"
                   "signal sender=:1.0 -> destination=:1.8; interface=com.arkflame.FlameWM1.Settings; member=SettingsChanged\n"
                   "signal sender=:1.0 -> destination=:1.8; interface=com.arkflame.FlameWM1.Settings; member=SettingsSnapshotChanged\n")
        self.assertTrue(acc.settings_signal_observations(fixture)["apply_call"])
        self.assertTrue(acc.settings_signal_observations(fixture)["settings_changed"])
        self.assertTrue(acc.settings_signal_observations(fixture)["snapshot_changed"])
        self.assertEqual(acc.ss14_accent_screen_point({"x": 0, "y": 31}, (100, 92)), (100, 123))
        self.assertEqual(acc.expected_selected_row_pixel("#2878D7"), (30, 50, 72))
        self.assertFalse(acc.ss03_selected_row_matches((34, 61, 76), "#2878D7"))

    def test_ss15_escape_requires_named_client_and_snapshot_clear_restore_not_active_title(self) -> None:
        geometry = {"name": "flamewm-settings", "width": 820, "height": 540, "x": 0, "y": 31}
        self.assertTrue(acc.ss15_escape_state_ok(True, geometry, (0, 0, 0), "Super", "", "Super"))
        self.assertFalse(acc.ss15_escape_state_ok(True, None, (0, 0, 0), "Super", "", "Super"))
        self.assertFalse(acc.ss15_escape_state_ok(True, geometry, (0, 0, 0), "Super", "Super", "Super"))

    def test_ss03_navigation_diagnostic_does_not_mask_missing_snapshot_effect(self) -> None:
        observations = {key: True for key in (
            "blue_snapshot", "teal_snapshot", "selected_visuals",
            "reset_persisted_after_restart")}
        observations["blue_snapshot"] = False
        # Even proven navigation click delivery cannot turn the missing state mutation into PASS.
        diagnostic = {"navigation_changed_and_returned": True}
        self.assertTrue(diagnostic["navigation_changed_and_returned"])
        self.assertEqual(acc.assess_ss03(observations)[0], "FAIL")

    def test_ss03_visual_report_moves_pointer_before_sampling_and_assembles_exit(self) -> None:
        calls = []

        def move(*args, **kwargs):
            calls.append(("move", kwargs.get("move")))
            return 0, "XTEST --move 900,600"

        def capture(*args, **kwargs):
            calls.append(("capture", args[2]))
            return {"shot": "/tmp/accent.png", "ok": True}

        with patch.object(acc, "send_input", side_effect=move), \
                patch.object(acc, "capture_accent_visual", side_effect=capture):
            report = acc.inspect_accent_after_click(
                object(), Path("/tmp"), "blue", {"x": 0, "y": 31}, (685, 151), "#3DAEE9", 0)

        self.assertEqual(calls, [("move", "900,600"), ("capture", "blue")])
        self.assertEqual(report["move_exit"], 0)
        self.assertTrue(report["passed"])

    def test_ss03_visual_report_fails_when_click_or_pointer_move_fails(self) -> None:
        visual = {"shot": "/tmp/accent.png", "ok": True}
        with patch.object(acc, "send_input", return_value=(3, "move failed")), \
                patch.object(acc, "capture_accent_visual", return_value=visual):
            report = acc.inspect_accent_after_click(
                object(), Path("/tmp"), "blue", {"x": 0, "y": 31}, (685, 151), "#3DAEE9", 0)
        self.assertFalse(report["passed"])
        with patch.object(acc, "send_input", return_value=(0, "move ok")), \
                patch.object(acc, "capture_accent_visual", return_value=visual):
            report = acc.inspect_accent_after_click(
                object(), Path("/tmp"), "blue", {"x": 0, "y": 31}, (685, 151), "#3DAEE9", 1)
        self.assertFalse(report["passed"])

    def test_ss03_requires_two_palette_values_visual_selection_and_restart(self) -> None:
        complete = {key: True for key in (
            "blue_snapshot", "teal_snapshot", "selected_visuals",
            "reset_persisted_after_restart")}
        self.assertEqual(acc.assess_ss03(complete)[0], "PASS")
        for missing in complete:
            partial = dict(complete)
            partial[missing] = None
            with self.subTest(missing=missing):
                self.assertEqual(acc.assess_ss03(partial)[0], "NOT_RUN")

    def test_ss03_measured_wrong_persisted_value_is_failure(self) -> None:
        complete = {key: True for key in (
            "blue_snapshot", "teal_snapshot", "selected_visuals",
            "reset_persisted_after_restart")}
        complete["reset_persisted_after_restart"] = False
        self.assertEqual(acc.assess_ss03(complete)[0], "FAIL")

    def test_ss15_does_not_pass_with_snapshot_only_or_missing_escape(self) -> None:
        observations = {key: True for key in (
            "single_output_boundary_observed", "disabled_output_prev_inert",
            "disabled_output_next_inert", "disabled_mode_ends_inert",
            "settings_survived_keyboard_escape")}
        self.assertEqual(acc.assess_ss15(observations)[0], "PASS")
        observations["settings_survived_keyboard_escape"] = None
        self.assertEqual(acc.assess_ss15(observations)[0], "NOT_RUN")
        observations["disabled_output_next_inert"] = False
        self.assertEqual(acc.assess_ss15(observations)[0], "FAIL")

    def test_ss19_requires_idle_repeat_resources_update_and_teardown(self) -> None:
        observations = {key: True for key in (
            "repeated_pages_opened_and_closed", "idle_cpu_bounded", "fd_growth_bounded",
            "memory_growth_bounded", "external_update_responsive", "owned_processes_clean")}
        self.assertEqual(acc.assess_ss19(observations)[0], "PASS")
        observations["external_update_responsive"] = None
        self.assertEqual(acc.assess_ss19(observations)[0], "NOT_RUN")

    def test_display_mode_count_uses_first_authoritative_output(self) -> None:
        self.assertEqual(acc.display_mode_count(
            "({'outputCount': <uint32 1>, 'outputs': <[{'id': <'x'>, 'modes': <[]>}]>},)"), 0)
        self.assertEqual(acc.display_mode_count(
            "({'outputCount': <uint32 1>, 'outputs': <[{'id': <'x'>, 'modes': <[{'id': <uint64 1>}, {'id': <uint64 2>}]>}]>},)"), 2)
        self.assertIsNone(acc.display_mode_count("not a snapshot"))

    def test_display_mode_count_ignores_mode_dictionary_key_order(self) -> None:
        modes = ", ".join(
            f"{{'height': <uint32 {720 + index}>, 'id': <uint64 {index}>}}"
            for index in range(16)
        )
        snapshot = "({'outputCount': <uint32 1>, 'outputs': <[{'id': <'x'>, 'modes': <[" + modes + "]>}]>},)"
        self.assertEqual(acc.display_mode_count(snapshot), 16)

    def test_display_mode_count_counts_mode_objects_not_nested_dictionaries(self) -> None:
        snapshot = "({'outputs': <[{'id': <'x'>, 'modes': <[{'height': <uint32 720>, 'meta': <{'id': <uint64 1>}>}]>}]>},)"
        self.assertEqual(acc.display_mode_count(snapshot), 1)

    def test_pixel_change_measures_only_requested_full_screen_crop(self) -> None:
        if acc.Image is None:
            self.skipTest("Pillow is unavailable for PNG fixture creation")
        with tempfile.TemporaryDirectory(prefix="flamewm-pixel-test-") as directory:
            before = Path(directory) / "before.png"
            outside = Path(directory) / "outside.png"
            inside = Path(directory) / "inside.png"
            cropped_before = Path(directory) / "cropped-before.png"
            cropped_inside = Path(directory) / "cropped-inside.png"
            base = acc.Image.new("RGB", (20, 20), "black")
            changed_outside = base.copy()
            changed_outside.putpixel((2, 2), (255, 0, 0))
            changed_inside = base.copy()
            changed_inside.putpixel((12, 12), (255, 0, 0))
            base.save(before)
            changed_outside.save(outside)
            changed_inside.save(inside)
            base.crop((10, 10, 15, 15)).save(cropped_before)
            changed_inside.crop((10, 10, 15, 15)).save(cropped_inside)
            backends = [("PIL", True)]
            if shutil.which("convert") and shutil.which("compare"):
                backends.append(("ImageMagick", False))
            for name, use_pil in backends:
                with self.subTest(backend=name), patch.object(acc, "HAVE_PIL", use_pil):
                    unchanged = acc.pixel_change(str(before), str(outside), (10, 10, 5, 5))
                    visible = acc.pixel_change(str(before), str(inside), (10, 10, 5, 5))
                    self.assertEqual(unchanged.get("fraction"), 0)
                    self.assertEqual(unchanged.get("total_pixels"), 25)
                    self.assertEqual(visible.get("fraction"), 1 / 25)
                    cropped = acc.pixel_change(str(cropped_before), str(cropped_inside), (0, 0, 5, 5))
                    self.assertEqual(cropped.get("fraction"), 1 / 25)


class ExternalSettingsScenarioTests(unittest.TestCase):
    def test_one_settings_mutation_does_not_pass_all_ss14_paths(self) -> None:
        observations = {
            "settings_accent_source_revision": True,
            "settings_signals_observed": True,
            "settings_visible_without_input": True,
            "panels_external_change": None,
            "shortcuts_external_change": None,
            "display_rollback_signal": None,
            "signal_loss_recovery": None,
        }
        status, reason = acc.assess_ss14(observations)
        self.assertEqual(status, "NOT_RUN")
        self.assertIn("panels_external_change", reason)
        self.assertIn("display_rollback_signal", reason)

    def test_measured_stale_source_revision_fails(self) -> None:
        observations = {key: True for key in (
            "settings_accent_source_revision", "settings_signals_observed",
            "settings_visible_without_input", "panels_external_change",
            "shortcuts_external_change", "display_rollback_signal", "signal_loss_recovery")}
        observations["settings_accent_source_revision"] = False
        self.assertEqual(acc.assess_ss14(observations)[0], "FAIL")

    def test_missing_source_or_visible_observation_stays_not_run(self) -> None:
        observations = {key: None for key in (
            "settings_accent_source_revision", "settings_signals_observed",
            "settings_visible_without_input", "panels_external_change",
            "shortcuts_external_change", "display_rollback_signal", "signal_loss_recovery")}
        self.assertEqual(acc.assess_ss14(observations)[0], "NOT_RUN")

    def test_gvariant_snapshot_parser_rejects_stale_or_malformed_fields(self) -> None:
        snapshot = "({'accent': <'#2878D7'>, 'revision': <uint64 14>},)"
        self.assertEqual(acc.gvariant_field_string(snapshot, "accent"), "#2878D7")
        self.assertEqual(acc.gvariant_field_u64(snapshot, "revision"), 14)
        stale = "({'accent': <'#2878D7'>},)"
        self.assertIsNone(acc.gvariant_field_u64(stale, "revision"))
        self.assertIsNone(acc.gvariant_field_string("garbage", "accent"))

    def test_signal_parser_requires_both_settings_change_signals(self) -> None:
        log = ("method call member=Apply\nsignal member=SettingsChanged\n"
               "signal member=SettingsSnapshotChanged\n")
        self.assertEqual(acc.settings_signal_observations(log), {
            "apply_call": True, "settings_changed": True, "snapshot_changed": True,
        })
        self.assertFalse(acc.settings_signal_observations("method call member=Apply")['settings_changed'])

    def test_accent_pixel_target_is_parsed_as_rgb(self) -> None:
        self.assertEqual(acc.accent_rgb("#2878D7"), (40, 120, 215))
        self.assertIsNone(acc.accent_rgb("bad"))
        self.assertEqual(acc.parse_pixel_rgb("srgb(40, 120, 215)"), (40, 120, 215))
        self.assertIsNone(acc.parse_pixel_rgb("not-a-color"))

    def test_external_settings_source_and_signal_senders_are_recorded(self) -> None:
        log = ("method call sender=:1.4 -> dest=:1.0; interface=com.arkflame.FlameWM1.Settings; member=Apply\n"
               "signal sender=:1.0 -> destination=:1.4; interface=com.arkflame.FlameWM1.Settings; member=SettingsChanged\n"
               "signal sender=:1.0 -> destination=:1.4; interface=com.arkflame.FlameWM1.Settings; member=SettingsSnapshotChanged\n")
        events = acc.settings_bus_events(log)
        self.assertEqual([event["member"] for event in events],
                         ["Apply", "SettingsChanged", "SettingsSnapshotChanged"])
        self.assertEqual(events[0]["sender"], ":1.4")
        self.assertEqual(events[1]["sender"], ":1.0")


class AppearanceDesktopTaskbarScenarioTests(unittest.TestCase):
    def test_icon_theme_snapshot_alone_does_not_pass_without_real_consumer(self) -> None:
        observations = {"supported_theme_snapshots": True, "unknown_theme_snapshot": True,
                        "real_icon_fallback_consumer": None, "theme_persistence_after_restart": None}
        status, reason = acc.assess_ss04(observations)
        self.assertEqual(status, "NOT_RUN")
        self.assertIn("real_icon_fallback_consumer", reason)

    def test_stale_icon_theme_snapshot_fails(self) -> None:
        observations = {"supported_theme_snapshots": False, "unknown_theme_snapshot": None,
                        "real_icon_fallback_consumer": None, "theme_persistence_after_restart": None}
        self.assertEqual(acc.assess_ss04(observations)[0], "FAIL")

    def test_desktop_snapshot_and_restart_without_scene_consumer_is_not_run(self) -> None:
        observations = {"desktop_values_snapshot": True, "settings_persisted_after_wm_restart": True,
                        "desktop_visual_consumer": None}
        status, reason = acc.assess_ss06(observations)
        self.assertEqual(status, "NOT_RUN")
        self.assertIn("desktop_visual_consumer", reason)

    def test_stale_desktop_values_fail(self) -> None:
        observations = {"desktop_values_snapshot": False, "settings_persisted_after_wm_restart": None,
                        "desktop_visual_consumer": None}
        self.assertEqual(acc.assess_ss06(observations)[0], "FAIL")

    def test_taskbar_panel_snapshot_without_shell_or_restart_is_not_run(self) -> None:
        observations = {"settings_snapshot_values": True, "panel_edge_and_size_snapshot": True,
                        "taskbar_visual_consumer": None, "settings_persistence_after_restart": None}
        status, reason = acc.assess_ss07(observations)
        self.assertEqual(status, "NOT_RUN")
        self.assertIn("taskbar_visual_consumer", reason)

    def test_taskbar_panel_mismatch_fails(self) -> None:
        observations = {"settings_snapshot_values": True, "panel_edge_and_size_snapshot": False,
                        "taskbar_visual_consumer": None, "settings_persistence_after_restart": None}
        self.assertEqual(acc.assess_ss07(observations)[0], "FAIL")

    def test_panel_snapshot_parser_rejects_missing_or_stale_fields(self) -> None:
        snapshot = ("({'panelCount': <uint32 1>, 'panels': <[{'edge': <'top'>, "
                    "'output': <'Xephyr-0'>, 'size': <uint32 53>, 'visible': <true>}]>, "
                    "'revision': <uint64 6>, 'tasks': <[]>},)")
        parsed = acc.parse_panels_snapshot(snapshot)
        self.assertEqual(parsed["revision"], 6)
        self.assertEqual(parsed["panels"][0]["output"], "Xephyr-0")
        self.assertEqual(parsed["panels"][0]["edge"], "top")
        self.assertEqual(parsed["panels"][0]["size"], 53)
        self.assertIsNone(acc.parse_panels_snapshot("{'panels': <[]>}"))


class RealConsumerObservableTests(unittest.TestCase):
    """Pure checks of the real-capture consumer observables (synthetic images, no X server)."""

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="ss-consumer-"))
        self.addCleanup(shutil.rmtree, self.tmp, True)

    def _shot(self, name: str, draw) -> Path:
        from PIL import Image
        image = Image.new("RGB", (1280, 800), (0, 0, 0))
        draw(image)
        path = self.tmp / name
        image.save(path)
        return path

    def test_ss06_requires_snap_preview_consumer(self) -> None:
        observations = {"desktop_values_snapshot": True, "settings_persisted_after_wm_restart": True,
                        "desktop_visual_consumer": True, "snap_preview_consumer": None}
        status, reason = acc.assess_ss06(observations)
        self.assertEqual(status, "NOT_RUN")
        self.assertIn("snap_preview_consumer", reason)
        observations["snap_preview_consumer"] = True
        self.assertEqual(acc.assess_ss06(observations)[0], "PASS")

    def test_rgb_close_rejects_far_and_missing(self) -> None:
        self.assertTrue(acc.rgb_close((35, 62, 90), (27, 53, 80), 16))
        self.assertFalse(acc.rgb_close((35, 62, 90), (0, 0, 0), 16))
        self.assertFalse(acc.rgb_close(None, (0, 0, 0), 16))

    def test_taskbar_matches_needs_edge_height_and_color(self) -> None:
        measure = {"edge": "bottom", "extent": 53, "inside": (35, 62, 90)}
        self.assertTrue(acc.taskbar_matches(measure, "bottom", 53, "#284F77", 67))
        self.assertFalse(acc.taskbar_matches(measure, "top", 53, "#284F77", 67))
        self.assertFalse(acc.taskbar_matches(measure, "bottom", 44, "#284F77", 67))
        self.assertFalse(acc.taskbar_matches(measure, "bottom", 53, "#4A3828", 67))
        self.assertFalse(acc.taskbar_matches(None, "bottom", 53, "#284F77", 67))

    def test_taskbar_measure_unchanged_capture_has_equal_band_and_change_differs(self) -> None:
        def bar(color, height):
            return lambda image: image.paste(color, (0, 800 - height, 1280, 800))
        shots = {"a": self._shot("a.png", bar((24, 26, 28), 44)), "b": self._shot("b.png", bar((24, 26, 28), 44)),
                 "c": self._shot("c.png", bar((35, 62, 90), 53))}
        measures = {}
        for key, path in shots.items():
            def fake_capture(ctx, out, source=path):
                shutil.copy(source, out)
                return True, "copy"
            ctx = SimpleNamespace(run=self.tmp)
            (self.tmp / "shots").mkdir(exist_ok=True)
            with patch("flamewm_settings_acceptance.capture", side_effect=fake_capture):
                measures[key] = acc.taskbar_measure(ctx, "m-" + key)
        self.assertEqual(measures["a"]["band_digest"], measures["b"]["band_digest"])
        self.assertNotEqual(measures["a"]["band_digest"], measures["c"]["band_digest"])
        self.assertEqual((measures["c"]["edge"], measures["c"]["extent"]), ("bottom", 53))

    def test_ss06_selection_red_measures_fill_not_noise(self) -> None:
        def fill(red):
            return lambda image: image.paste((red, 14, 14), (5, 5, 70, 80))
        base = self._shot("base.png", lambda image: None)
        low, high = self._shot("low.png", fill(48)), self._shot("high.png", fill(120))
        self.assertEqual(acc.ss06_selection_red(base, low), 48)
        self.assertEqual(acc.ss06_selection_red(base, high), 120)
        self.assertIsNone(acc.ss06_selection_red(base, base))  # unchanged capture must not count
        self.assertIsNone(acc.ss06_selection_red(None, low))

    def test_ss06_note_visibility_tracks_note_color(self) -> None:
        x, y, w, h = acc.SS06_NOTE_RECT
        shown = self._shot("shown.png", lambda image: image.paste(acc.SS06_NOTE_COLOR, (x, y, x + w, y + h)))
        hidden = self._shot("hidden.png", lambda image: None)
        self.assertTrue(acc.ss06_note_visible(shown))
        self.assertFalse(acc.ss06_note_visible(hidden))
        self.assertIsNone(acc.ss06_note_visible(None))

    def test_gtk_consumer_latest_reads_last_sample_and_tolerates_garbage(self) -> None:
        log = self.tmp / "c.jsonl"
        self.assertIsNone(acc.gtk_consumer_latest(log))
        log.write_text('{"name": "Yaru"}\n{"name": "breeze-dark"}\n', encoding="utf-8")
        self.assertEqual(acc.gtk_consumer_latest(log)["name"], "breeze-dark")
        log.write_text("not json\n", encoding="utf-8")
        self.assertIsNone(acc.gtk_consumer_latest(log))
        log.write_text('{"name": "Yaru"}\n', encoding="utf-8")
        self.assertEqual(acc.gtk_consumer_wait(log, "breeze-dark", timeout=0.3)["name"], "Yaru")

    def test_new_binary_args_default_to_debug_build(self) -> None:
        args = acc.parse_args(["--run-dir", str(self.tmp / "r")])
        self.assertTrue(args.shell_bin.endswith(".target/debug/flamewm-shell"))
        self.assertTrue(args.desktop_bin.endswith(".target/debug/flamewm-desktop"))


class XtestHeldKeyTests(unittest.TestCase):
    class FakeBackend:
        KEYCODES = {"shift": 10, "F2": 11}

        def __init__(self, reject=None):
            self.reject = reject
            self.events: list = []

        def keycode(self, keysym: str) -> int:
            return self.KEYCODES.get(keysym, 0)

        def fake_key(self, code: int, down: bool) -> bool:
            self.events.append(("down" if down else "up", code))
            return not (self.reject == ("down" if down else "up"))

    def test_parse_hold_valid(self) -> None:
        self.assertEqual(xt.parse_hold("shift:500"), ("shift", 500))

    def test_parse_hold_rejects_bad_input(self) -> None:
        for bad in ("shift", ":500", "shift:", "shift:abc", "shift:-1", "shift:99999999"):
            with self.subTest(bad=bad):
                with self.assertRaises(ValueError):
                    xt.parse_hold(bad)

    def test_held_key_is_down_then_sleep_then_up(self) -> None:
        backend = self.FakeBackend()
        sleeps: list[float] = []
        xt.held_key_events(backend, "shift", 250, sleep=sleeps.append)
        self.assertEqual(backend.events, [("down", 10), ("up", 10)])
        self.assertEqual(sleeps, [0.25])

    def test_held_key_releases_when_sleep_raises(self) -> None:
        backend = self.FakeBackend()

        def boom(_seconds: float) -> None:
            raise KeyboardInterrupt

        with self.assertRaises(KeyboardInterrupt):
            xt.held_key_events(backend, "shift", 100, sleep=boom)
        self.assertEqual(backend.events, [("down", 10), ("up", 10)])

    def test_unmapped_held_key_sends_no_events(self) -> None:
        backend = self.FakeBackend()
        sleeps: list[float] = []
        with self.assertRaises(xt.UnmappedKeysym):
            xt.held_key_events(backend, "NoSuchKey", 100, sleep=sleeps.append)
        self.assertEqual(backend.events, [])
        self.assertEqual(sleeps, [])

    def test_rejected_press_skips_sleep_and_release(self) -> None:
        backend = self.FakeBackend(reject="down")
        sleeps: list[float] = []
        with self.assertRaises(RuntimeError):
            xt.held_key_events(backend, "shift", 100, sleep=sleeps.append)
        self.assertEqual(sleeps, [])
        self.assertEqual(backend.events, [("down", 10)])

    def test_separate_down_and_up_are_single_events(self) -> None:
        backend = self.FakeBackend()
        xt.press_key(backend, "F2", down=True)
        xt.press_key(backend, "F2", down=False)
        self.assertEqual(backend.events, [("down", 11), ("up", 11)])


class ScaleFontsAboutDecisionTests(unittest.TestCase):
    def test_ss10_needs_snapshot_change_and_restart_persistence(self) -> None:
        observations = {"scale_changed_snapshot": True, "mode_list_unchanged": True,
                        "scale_persisted_after_restart": True}
        self.assertEqual(acc.assess_ss10(observations)[0], "PASS")
        observations["scale_persisted_after_restart"] = None
        status, reason = acc.assess_ss10(observations)
        self.assertEqual(status, "NOT_RUN")
        self.assertIn("scale_persisted_after_restart", reason)
        observations["scale_changed_snapshot"] = False
        self.assertEqual(acc.assess_ss10(observations)[0], "FAIL")

    def test_ss10_mode_change_after_click_fails_and_unmeasured_mode_list_stays_not_run(self) -> None:
        observations = {"scale_changed_snapshot": True, "mode_list_unchanged": False,
                        "scale_persisted_after_restart": True}
        status, reason = acc.assess_ss10(observations)
        self.assertEqual(status, "FAIL")
        self.assertIn("mode_list_unchanged", reason)
        observations["mode_list_unchanged"] = None
        status, reason = acc.assess_ss10(observations)
        self.assertEqual(status, "NOT_RUN")
        self.assertIn("mode_list_unchanged", reason)

    def test_ss10_scale_applied_rejects_wrong_value_and_failed_click(self) -> None:
        self.assertTrue(acc.ss10_scale_applied(0, {"Xephyr-0": 125}, "Xephyr-0"))
        self.assertFalse(acc.ss10_scale_applied(0, {"Xephyr-0": 100}, "Xephyr-0"))
        self.assertFalse(acc.ss10_scale_applied(0, {"Xephyr-0": 150}, "Xephyr-0"))
        self.assertFalse(acc.ss10_scale_applied(1, {"Xephyr-0": 125}, "Xephyr-0"))
        self.assertFalse(acc.ss10_scale_applied(0, None, "Xephyr-0"))

    def test_display_output_scales_maps_each_output_id(self) -> None:
        snapshot = ("({'outputCount': <uint32 2>, 'outputs': <["
                    "{'id': <'Xephyr-0'>, 'modes': <[{'id': <uint64 3>, 'width': <int32 1>}]>, 'scale': <uint32 100>}, "
                    "{'id': <'Xephyr-1'>, 'modes': <[]>, 'scale': <uint32 125>}]>},)")
        self.assertEqual(acc.display_output_scales(snapshot), {"Xephyr-0": 100, "Xephyr-1": 125})

    def test_display_output_scales_rejects_malformed_snapshots(self) -> None:
        self.assertIsNone(acc.display_output_scales("not a snapshot"))
        self.assertIsNone(acc.display_output_scales("({'outputs': <[]>},)"))
        self.assertIsNone(acc.display_output_scales("({'outputs': <[{'id': <'x'>, 'modes': <[]>}]>},)"))

    def test_top_level_blocks_skips_braces_inside_quoted_text(self) -> None:
        self.assertEqual(acc.top_level_blocks("{'a': <'}'>}, {'b': <1>}"), ["{'a': <'}'>}", "{'b': <1>}"])

    def test_ss11_family_and_size_are_measured_against_the_previous_snapshot(self) -> None:
        self.assertTrue(acc.ss11_family_applied(0, "IBM Plex Sans", "Noto Sans"))
        self.assertFalse(acc.ss11_family_applied(0, "IBM Plex Sans", "IBM Plex Sans"))
        self.assertFalse(acc.ss11_family_applied(1, "IBM Plex Sans", "Noto Sans"))
        self.assertTrue(acc.ss11_size_applied(0, 0, 1, "Noto Sans", "Noto Sans"))
        self.assertFalse(acc.ss11_size_applied(0, 0, 2, "Noto Sans", "Noto Sans"))
        self.assertFalse(acc.ss11_size_applied(0, 0, 1, "Noto Sans", "DejaVu Sans"))
        self.assertFalse(acc.ss11_size_applied(0, 0, 0, "Noto Sans", "Noto Sans"))

    def test_ss11_persistence_requires_family_and_size(self) -> None:
        persisted = "({'fontFamily': <'Noto Sans'>, 'fontSizeOffset': <int64 1>},)"
        self.assertTrue(acc.ss11_fonts_persisted(persisted, "Noto Sans", 1))
        self.assertFalse(acc.ss11_fonts_persisted(persisted, "Noto Sans", 0))
        self.assertFalse(acc.ss11_fonts_persisted(persisted, "IBM Plex Sans", 1))
        self.assertEqual(acc.gvariant_field_int("{'fontSizeOffset': <int64 -2>}", "fontSizeOffset"), -2)
        self.assertIsNone(acc.gvariant_field_uint("{'fontSizeOffset': <int64 -2>}", "fontSizeOffset"))

    def test_ss11_needs_both_controls_and_persistence(self) -> None:
        observations = {"family_changed_snapshot": True, "size_changed_snapshot": True,
                        "fonts_persisted_after_restart": True}
        self.assertEqual(acc.assess_ss11(observations)[0], "PASS")
        observations["size_changed_snapshot"] = None
        self.assertEqual(acc.assess_ss11(observations)[0], "NOT_RUN")
        observations["size_changed_snapshot"] = False
        self.assertEqual(acc.assess_ss11(observations)[0], "FAIL")

    def test_ss12_reset_must_return_platform_default(self) -> None:
        self.assertTrue(acc.ss12_reset_restored(0, "Super_L"))
        self.assertFalse(acc.ss12_reset_restored(0, ""))
        self.assertFalse(acc.ss12_reset_restored(0, "Super"))
        self.assertFalse(acc.ss12_reset_restored(1, "Super_L"))

    def test_ss13_version_needs_cargo_exit_zero_and_the_single_test_result_line(self) -> None:
        passed = "running 1 test\ntest controller::tests::about_version_text_is_exact_flamewm_version ... ok\n" \
                 "test result: ok. 1 passed; 0 failed; 0 ignored\n"
        self.assertTrue(acc.ss13_version_test_observed(0, passed))
        self.assertFalse(acc.ss13_version_test_observed(101, passed))
        self.assertFalse(acc.ss13_version_test_observed(0, "test result: ok. 0 passed; 0 failed"))
        self.assertFalse(acc.ss13_version_test_observed(1, "error[E0432]: unresolved import"))

    def test_ss13_ocr_matcher_exact_digits_and_negative_controls(self) -> None:
        match = acc.ss13_version_ocr_matches
        self.assertTrue(match("Version 0.0.9", "0.0.9"))
        self.assertTrue(match("Versi0n 0.0.9\n", "0.0.9"))
        self.assertTrue(match("Versian 0.0.9", "0.0.9"))
        self.assertFalse(match("Version 0.0.9", "0.0.8"))
        self.assertFalse(match("Version 0.0.90", "0.0.9"))
        self.assertFalse(match("Version 0.0.9.1", "0.0.9"))
        self.assertFalse(match("0.0.9", "0.0.9"))
        self.assertFalse(match("Native desktop settings for FlameWM", "0.0.9"))
        self.assertFalse(match("A lightweight desktop by ArkFlame Studios", "0.0.9"))
        self.assertFalse(match("Version 0.0.8", "0.0.9"))

    def test_ss13_needs_three_renders_and_version(self) -> None:
        observations = {"about_render_760x500": True, "about_render_820x540": True,
                        "about_render_960x640": True, "version_matches_flamewm_version": True,
                        "version_live_ocr_matches": True}
        self.assertEqual(acc.assess_ss13(observations)[0], "PASS")
        observations["version_live_ocr_matches"] = None
        self.assertEqual(acc.assess_ss13(observations)[0], "NOT_RUN")
        observations["version_live_ocr_matches"] = False
        self.assertEqual(acc.assess_ss13(observations)[0], "FAIL")
        observations["version_live_ocr_matches"] = True
        observations["version_matches_flamewm_version"] = None
        status, reason = acc.assess_ss13(observations)
        self.assertEqual(status, "NOT_RUN")
        self.assertIn("version_matches_flamewm_version", reason)
        observations["about_render_960x640"] = False
        self.assertEqual(acc.assess_ss13(observations)[0], "FAIL")

    def test_new_scenarios_are_runner_ids_so_blocked_runs_report_them(self) -> None:
        results: dict[str, dict] = {}
        acc.block_runner_ids(results, "no free display in :90..:99")
        for sid in ("SS10", "SS11", "SS13"):
            with self.subTest(sid=sid):
                self.assertEqual(results[sid]["status"], "ENVIRONMENT_BLOCKED")

    def test_client_points_for_new_controls_are_bounded_to_the_settings_client(self) -> None:
        run = Path(tempfile.mkdtemp(prefix="flamewm-ss-args-")) / "run"
        self.addCleanup(shutil.rmtree, run.parent, True)
        for flag, value in (("--ss10-scale-click", "960,10"), ("--ss11-family-click", "10,600"),
                            ("--ss11-size-click", "820,20"), ("--ss10-scale-click", "10"),
                            ("--ss10-scrolled-click", "960,10"), ("--ss10-scrolled-click", "10")):
            with self.subTest(flag=flag, value=value):
                args = acc.parse_args(["--run-dir", str(run), flag, value])
                with self.assertRaises(SystemExit):
                    acc.validate(args)
        for steps in ("0", "101"):
            with self.subTest(wheel_steps=steps):
                args = acc.parse_args(["--run-dir", str(run), "--ss10-wheel-steps", steps])
                with self.assertRaises(SystemExit):
                    acc.validate(args)
        scrolled = acc.parse_args(["--run-dir", str(run), "--ss10-wheel-steps", "5", "--ss10-scrolled-click", "600,300"])
        self.assertEqual(acc.validate(scrolled), run.resolve())
        valid = acc.parse_args(["--run-dir", str(run), "--ss10-scale-click", "600,300",
                                "--ss11-family-click", "100,200", "--ss11-size-click", "300,200"])
        self.assertEqual(acc.validate(valid), run.resolve())


class FakeNativeRunnerTests(unittest.TestCase):
    """Drive run_ss10/run_ss11/run_ss13 with a faked display, D-Bus and screenshot layer."""

    BEFORE_DISPLAYS = "({'outputCount': <uint32 1>, 'outputs': <[{'id': <'Xephyr-0'>, 'modes': <[]>, 'scale': <uint32 100>}]>},)"
    CHANGED_DISPLAYS = BEFORE_DISPLAYS.replace("uint32 100", "uint32 125")
    STUCK_DISPLAYS = BEFORE_DISPLAYS
    MODE_LIST_ONE = "'modes': <[{'id': <'1'>, 'width': <uint32 1280>}]>"
    MODE_LIST_TWO = "'modes': <[{'id': <'1'>, 'width': <uint32 1280>}, {'id': <'2'>, 'width': <uint32 1024>}]>"
    MODED_DISPLAYS = BEFORE_DISPLAYS.replace("'modes': <[]>", MODE_LIST_ONE)
    MODED_CHANGED = MODED_DISPLAYS.replace("uint32 100", "uint32 125")
    MODED_TWO_MODES_AT_125 = MODED_CHANGED.replace(MODE_LIST_ONE, MODE_LIST_TWO)
    BEFORE_FONTS = "({'fontFamily': <'IBM Plex Sans'>, 'fontSizeOffset': <int64 0>, 'revision': <uint64 3>},)"
    FAMILY_FONTS = "({'fontFamily': <'Noto Sans'>, 'fontSizeOffset': <int64 0>, 'revision': <uint64 4>},)"
    SIZED_FONTS = "({'fontFamily': <'Noto Sans'>, 'fontSizeOffset': <int64 1>, 'revision': <uint64 5>},)"

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp(prefix="flamewm-ss-fake-"))
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.ctx = acc.Ctx(self.tmp)
        self.ctx.display = ":97"
        self.ctx.app_env = {}
        self.fake_owned = acc.Owned("fake", SimpleNamespace(poll=lambda: None, pid=4242))
        self.spawn = self._patch("flamewm_settings_acceptance.Ctx.spawn", return_value=self.fake_owned)
        self._patch("flamewm_settings_acceptance.Ctx.stop")
        self._patch("flamewm_settings_acceptance.time.sleep")
        self._patch("flamewm_settings_acceptance.shutil.which", return_value="/usr/bin/gdbus")
        self.geometry = self._patch("flamewm_settings_acceptance.wait_for_settings_window",
                                    return_value={"x": 0, "y": 0, "width": 820, "height": 540})
        self.send_input = self._patch("flamewm_settings_acceptance.send_input", return_value=(0, "xtest"))
        self.restart = self._patch("flamewm_settings_acceptance.restart_owned_wm", return_value=True)
        self.live = self._patch("flamewm_settings_acceptance.ss13_live_version",
                                side_effect=lambda shot, size, *a: {"size": "x", "observed": True})
        # Never let a unit test shell out to cargo; SS13 tests set the verdict they need.
        self.version_test = self._patch("flamewm_settings_acceptance.run_about_version_test",
                                        return_value=(True, {"command": "cargo test", "exit_code": 0,
                                                             "result_line": acc.ABOUT_VERSION_PASSED}))

    def _patch(self, target: str, **kwargs):
        patcher = patch(target, **kwargs)
        mock = patcher.start()
        self.addCleanup(patcher.stop)
        return mock

    def _args(self, *extra: str):
        return acc.parse_args(["--run-dir", str(self.tmp / "run"), *extra])

    def test_ss10_passes_only_with_click_snapshot_change_and_restart_persistence(self) -> None:
        self._patch("flamewm_settings_acceptance.dbus_displays_snapshot",
                    side_effect=[(self.BEFORE_DISPLAYS, "q"), (self.CHANGED_DISPLAYS, "q"), (self.CHANGED_DISPLAYS, "q")])
        self.geometry.return_value = {"x": 0, "y": 0, "width": 960, "height": 640}
        entry = acc.run_ss10(self.ctx, self._args("--ss10-scale-click", "600,300"))
        self.assertEqual(entry["status"], "PASS", entry["reason"])
        self.restart.assert_called_once()
        # Geometry is faked at origin (0,0), so the client-relative click maps to the same screen point.
        self.assertEqual(self.send_input.call_args.args[1], "600,300")

    def test_ss10_with_listed_modes_is_not_run_and_never_clicks(self) -> None:
        modes = self.BEFORE_DISPLAYS.replace("'modes': <[]>", "'modes': <[{'id': <'1'>, 'width': <uint32 1280>}]>")
        self._patch("flamewm_settings_acceptance.dbus_displays_snapshot", return_value=(modes, "q"))
        self.geometry.return_value = {"x": 0, "y": 0, "width": 960, "height": 640}
        entry = acc.run_ss10(self.ctx, self._args("--ss10-scale-click", "600,300"))
        self.assertEqual(entry["status"], "NOT_RUN", entry["reason"])
        self.assertIn("below the fold", entry["details"]["reason"])
        self.send_input.assert_not_called()

    def test_ss10_listed_modes_scrolled_then_125_with_unchanged_modes_passes(self) -> None:
        self._patch("flamewm_settings_acceptance.dbus_displays_snapshot",
                    side_effect=[(self.MODED_DISPLAYS, "q"), (self.MODED_CHANGED, "q"), (self.MODED_CHANGED, "q")])
        self.geometry.return_value = {"x": 0, "y": 0, "width": 960, "height": 640}
        entry = acc.run_ss10(self.ctx, self._args("--ss10-wheel-steps", "3", "--ss10-scrolled-click", "600,300"))
        self.assertEqual(entry["status"], "PASS", entry["reason"])
        scroll, click = self.send_input.call_args_list
        self.assertEqual(scroll.kwargs, {"move": "480,320", "wheel": "down:3"})
        self.assertEqual(click.args[1], "600,300")
        self.assertEqual(entry["details"]["scroll"], {"direction": "down", "steps": 3, "pointer": "480,320", "exit_code": 0})
        self.assertEqual(entry["details"]["after"]["click"], "600,300")
        self.restart.assert_called_once()

    def test_ss10_listed_modes_without_wheel_steps_is_not_run_and_never_scrolls_or_clicks(self) -> None:
        self.geometry.return_value = {"x": 0, "y": 0, "width": 960, "height": 640}
        self._patch("flamewm_settings_acceptance.dbus_displays_snapshot", return_value=(self.MODED_DISPLAYS, "q"))
        entry = acc.run_ss10(self.ctx, self._args("--ss10-scrolled-click", "600,300"))
        self.assertEqual(entry["status"], "NOT_RUN", entry["reason"])
        self.assertIn("--ss10-wheel-steps", entry["details"]["reason"])
        self.send_input.assert_not_called()

    def test_ss10_listed_modes_without_scrolled_click_is_not_run_and_never_scrolls_or_clicks(self) -> None:
        self.geometry.return_value = {"x": 0, "y": 0, "width": 960, "height": 640}
        self._patch("flamewm_settings_acceptance.dbus_displays_snapshot", return_value=(self.MODED_DISPLAYS, "q"))
        entry = acc.run_ss10(self.ctx, self._args("--ss10-wheel-steps", "3"))
        self.assertEqual(entry["status"], "NOT_RUN", entry["reason"])
        self.assertIn("--ss10-scrolled-click", entry["details"]["reason"])
        self.send_input.assert_not_called()

    def test_ss10_failed_wheel_input_is_not_run_and_skips_the_click(self) -> None:
        self.geometry.return_value = {"x": 0, "y": 0, "width": 960, "height": 640}
        self._patch("flamewm_settings_acceptance.dbus_displays_snapshot", return_value=(self.MODED_DISPLAYS, "q"))
        self.send_input.return_value = (1, "xtest")
        entry = acc.run_ss10(self.ctx, self._args("--ss10-wheel-steps", "3", "--ss10-scrolled-click", "600,300"))
        self.assertEqual(entry["status"], "NOT_RUN", entry["reason"])
        self.assertEqual(entry["details"]["scroll"]["exit_code"], 1)
        self.assertEqual(self.send_input.call_count, 1)
        self.assertEqual(self.send_input.call_args.kwargs.get("wheel"), "down:3")
        self.assertIsNone(self.send_input.call_args.args[1])
        self.restart.assert_not_called()

    def test_ss10_listed_modes_change_after_scrolled_click_fails(self) -> None:
        self.geometry.return_value = {"x": 0, "y": 0, "width": 960, "height": 640}
        self._patch("flamewm_settings_acceptance.dbus_displays_snapshot",
                    side_effect=[(self.MODED_DISPLAYS, "q"), (self.MODED_TWO_MODES_AT_125, "q"),
                                 (self.MODED_TWO_MODES_AT_125, "q")])
        entry = acc.run_ss10(self.ctx, self._args("--ss10-wheel-steps", "3", "--ss10-scrolled-click", "600,300"))
        self.assertEqual(entry["status"], "FAIL", entry["reason"])
        self.assertIn("mode_list_unchanged", entry["reason"])

    def test_ss10_wrong_scale_after_click_fails_and_skips_restart(self) -> None:
        self._patch("flamewm_settings_acceptance.dbus_displays_snapshot",
                    side_effect=[(self.BEFORE_DISPLAYS, "q"), (self.STUCK_DISPLAYS, "q")])
        self.geometry.return_value = {"x": 0, "y": 0, "width": 960, "height": 640}
        entry = acc.run_ss10(self.ctx, self._args("--ss10-scale-click", "600,300"))
        self.assertEqual(entry["status"], "FAIL", entry["reason"])
        self.restart.assert_not_called()

    def test_ss10_wrong_scale_after_restart_fails(self) -> None:
        self._patch("flamewm_settings_acceptance.dbus_displays_snapshot",
                    side_effect=[(self.BEFORE_DISPLAYS, "q"), (self.CHANGED_DISPLAYS, "q"), (self.STUCK_DISPLAYS, "q")])
        self.geometry.return_value = {"x": 0, "y": 0, "width": 960, "height": 640}
        entry = acc.run_ss10(self.ctx, self._args("--ss10-scale-click", "600,300"))
        self.assertEqual(entry["status"], "FAIL", entry["reason"])

    def test_ss10_without_click_coordinate_is_not_run_and_spawns_nothing(self) -> None:
        args = self._args()
        args.ss10_scale_click = None
        entry = acc.run_ss10(self.ctx, args)
        self.assertEqual(entry["status"], "NOT_RUN")
        self.assertIn("--ss10-scale-click", entry["reason"])
        self.spawn.assert_not_called()

    def test_ss11_passes_only_when_both_controls_and_restart_persist(self) -> None:
        self._patch("flamewm_settings_acceptance.dbus_settings_snapshot",
                    side_effect=[(self.BEFORE_FONTS, "q"), (self.FAMILY_FONTS, "q"),
                                 (self.SIZED_FONTS, "q"), (self.SIZED_FONTS, "q")])
        entry = acc.run_ss11(self.ctx, self._args("--ss11-family-click", "100,200", "--ss11-size-click", "300,200"))
        self.assertEqual(entry["status"], "PASS", entry["reason"])
        self.assertEqual(self.send_input.call_count, 2)

    def test_ss11_size_click_without_effect_fails(self) -> None:
        self._patch("flamewm_settings_acceptance.dbus_settings_snapshot",
                    side_effect=[(self.BEFORE_FONTS, "q"), (self.FAMILY_FONTS, "q"), (self.FAMILY_FONTS, "q")])
        entry = acc.run_ss11(self.ctx, self._args("--ss11-family-click", "100,200", "--ss11-size-click", "300,200"))
        self.assertEqual(entry["status"], "FAIL", entry["reason"])
        self.restart.assert_not_called()

    def test_ss11_persisted_family_mismatch_after_restart_fails(self) -> None:
        self._patch("flamewm_settings_acceptance.dbus_settings_snapshot",
                    side_effect=[(self.BEFORE_FONTS, "q"), (self.FAMILY_FONTS, "q"),
                                 (self.SIZED_FONTS, "q"), (self.BEFORE_FONTS, "q")])
        entry = acc.run_ss11(self.ctx, self._args("--ss11-family-click", "100,200", "--ss11-size-click", "300,200"))
        self.assertEqual(entry["status"], "FAIL", entry["reason"])

    def test_ss11_without_both_coordinates_is_not_run(self) -> None:
        args = self._args("--ss11-family-click", "100,200")
        args.ss11_size_click = None
        entry = acc.run_ss11(self.ctx, args)
        self.assertEqual(entry["status"], "NOT_RUN")
        self.spawn.assert_not_called()

    def _shot(self, width: int, height: int, nonblank: bool = True, window: tuple[int, int] | None = None) -> dict:
        measured = window or (width, height)
        return {"page": "about", "alive": True, "captured": True, "nonblank": nonblank, "shot": f"{width}x{height}.png",
                "window": {"width": measured[0], "height": measured[1]}, "commands": ["import"]}

    def test_ss13_passes_with_three_renders_and_passing_about_version_test(self) -> None:
        self._patch("flamewm_settings_acceptance.page_shot",
                    side_effect=lambda ctx, args, page, label, window_size=None: self._shot(*window_size))
        entry = acc.run_ss13(self.ctx, self._args())
        self.assertEqual(entry["status"], "PASS", entry["reason"])
        self.assertIs(entry["details"]["observations"]["version_matches_flamewm_version"], True)
        self.assertIn(acc.ABOUT_VERSION_TEST, entry["details"]["paths_exercised"][-1])

    def test_ss13_failing_about_version_test_fails(self) -> None:
        self._patch("flamewm_settings_acceptance.page_shot",
                    side_effect=lambda ctx, args, page, label, window_size=None: self._shot(*window_size))
        self.version_test.return_value = (False, {"command": "cargo test", "exit_code": 101, "result_line": None})
        entry = acc.run_ss13(self.ctx, self._args())
        self.assertEqual(entry["status"], "FAIL", entry["reason"])
        self.assertIn("version_matches_flamewm_version", entry["reason"])
        self.assertEqual(entry["details"]["version_test"]["exit_code"], 101)

    def test_ss13_cargo_absent_is_not_run_not_pass(self) -> None:
        self._patch("flamewm_settings_acceptance.page_shot",
                    side_effect=lambda ctx, args, page, label, window_size=None: self._shot(*window_size))
        self.version_test.return_value = (None, {"command": "cargo test", "reason": "cargo not on PATH"})
        entry = acc.run_ss13(self.ctx, self._args())
        self.assertEqual(entry["status"], "NOT_RUN")
        self.assertIn("version_matches_flamewm_version", entry["reason"])
        self.assertIsNone(entry["details"]["observations"]["version_matches_flamewm_version"])
        self.assertEqual(len(entry["details"]["renders"]), 3)

    def test_ss13_live_ocr_failure_fails_and_missing_tesseract_is_not_run(self) -> None:
        self._patch("flamewm_settings_acceptance.page_shot",
                    side_effect=lambda ctx, args, page, label, window_size=None: self._shot(*window_size))
        self.live.side_effect = lambda shot, size, *a: {"size": "x", "observed": False}
        entry = acc.run_ss13(self.ctx, self._args())
        self.assertEqual(entry["status"], "FAIL")
        self.assertIn("version_live_ocr_matches", entry["reason"])
        self.live.side_effect = lambda shot, size, *a: {"size": "x", "observed": None, "refused": "geometry"}
        self.assertEqual(acc.run_ss13(self.ctx, self._args())["status"], "NOT_RUN")
        self._patch("flamewm_settings_acceptance.shutil.which", return_value=None)
        entry = acc.run_ss13(self.ctx, self._args())
        self.assertEqual(entry["status"], "NOT_RUN")
        self.assertIn("tesseract", entry["details"]["live_ocr_reason"])

    def test_ss13_does_not_read_a_settings_snapshot_version_key(self) -> None:
        self._patch("flamewm_settings_acceptance.page_shot",
                    side_effect=lambda ctx, args, page, label, window_size=None: self._shot(*window_size))
        snapshot = self._patch("flamewm_settings_acceptance.dbus_settings_snapshot")
        acc.run_ss13(self.ctx, self._args())
        snapshot.assert_not_called()

    def test_ss13_blank_or_mismatched_render_fails(self) -> None:
        renders = {(760, 500): self._shot(760, 500, nonblank=False),
                   (820, 540): self._shot(820, 540),
                   (960, 640): self._shot(960, 640)}
        self._patch("flamewm_settings_acceptance.page_shot",
                    side_effect=lambda ctx, args, page, label, window_size=None: renders[window_size])
        entry = acc.run_ss13(self.ctx, self._args())
        self.assertEqual(entry["status"], "FAIL", entry["reason"])
        self.assertIn("about_render_760x500", entry["reason"])
        renders[(760, 500)] = self._shot(760, 500)
        renders[(960, 640)] = self._shot(960, 640, window=(820, 540))
        entry = acc.run_ss13(self.ctx, self._args())
        self.assertEqual(entry["status"], "FAIL", entry["reason"])
        self.assertIn("about_render_960x640", entry["reason"])


class Ss12DefaultClickGeometryTests(unittest.TestCase):
    def _run(self, geometry: dict | None, extra: list[str] | None = None):
        tmp = Path(tempfile.mkdtemp(prefix="flamewm-ss12-geom-"))
        self.addCleanup(shutil.rmtree, tmp, True)
        ctx = acc.Ctx(tmp)
        ctx.display = ":97"
        proc = SimpleNamespace(poll=lambda: None)
        owned = SimpleNamespace(proc=proc, pid=1, name="x")
        sent: list = []
        snapshot = "({'ToggleStartMenu': <'Super_L'>},)"
        with patch("flamewm_settings_acceptance.shutil.which", return_value="/usr/bin/x"), \
                patch("flamewm_settings_acceptance.dbus_shortcuts_snapshot", return_value=(snapshot, "gdbus")), \
                patch("flamewm_settings_acceptance.Ctx.spawn", return_value=owned), \
                patch("flamewm_settings_acceptance.Ctx.stop"), \
                patch("flamewm_settings_acceptance.stop_dbus_monitor"), \
                patch("flamewm_settings_acceptance.time.sleep"), \
                patch("flamewm_settings_acceptance.wait_for_settings_window", return_value=geometry), \
                patch("flamewm_settings_acceptance.keyboard_preflight", return_value=None), \
                patch("flamewm_settings_acceptance.send_input",
                      side_effect=lambda *a, **k: sent.append(a) or (0, "x")):
            entry = acc.run_ss12(ctx, acc.parse_args(["--run-dir", str(tmp / "run"), *(extra or [])]))
        return entry, sent

    def test_default_click_refused_when_client_is_not_the_measured_fixture(self) -> None:
        entry, sent = self._run({"name": "System Settings", "width": 960, "height": 640, "x": 0, "y": 31})
        self.assertEqual(entry["status"], "NOT_RUN")
        self.assertIn("default clicks refused", entry["reason"])
        self.assertEqual(sent, [])

    def test_default_click_refused_without_a_measured_window(self) -> None:
        entry, sent = self._run(None)
        self.assertEqual(entry["status"], "NOT_RUN")
        self.assertEqual(sent, [])

    def test_measured_constants_resolve_to_screen_points_for_the_fixture(self) -> None:
        geometry = {"name": "System Settings", "width": 820, "height": 540, "x": 0, "y": 31}
        ctx = SimpleNamespace()
        self.assertEqual(acc.settings_relative_point(ctx, geometry, acc.SS12_CAPTURE_CLIENT), "593,180")
        self.assertEqual(acc.settings_relative_point(ctx, geometry, acc.SS12_RESET_CLIENT), "733,180")


class Ss16StaticGateTests(unittest.TestCase):
    def test_all_zero_passes(self) -> None:
        status, _ = acc.assess_ss16({name: 0 for name, _ in acc.STATIC_GATES})
        self.assertEqual(status, "PASS")

    def test_one_gate_exiting_one_fails_and_names_it(self) -> None:
        codes = {name: 0 for name, _ in acc.STATIC_GATES}
        codes["architecture_guard"] = 1
        status, reason = acc.assess_ss16(codes)
        self.assertEqual(status, "FAIL")
        self.assertIn("architecture_guard=1", reason)

    def test_unrun_gate_is_not_run_never_pass(self) -> None:
        codes = {name: 0 for name, _ in acc.STATIC_GATES}
        codes["architecture_guard"] = None
        self.assertEqual(acc.assess_ss16(codes)[0], "NOT_RUN")
        self.assertEqual(acc.assess_ss16({})[0], "NOT_RUN")

    def test_runner_maps_real_exit_codes(self) -> None:
        results = iter([0, 1, 0, 0])
        with patch("flamewm_settings_acceptance.subprocess.run",
                   side_effect=lambda *a, **k: SimpleNamespace(returncode=next(results))), \
                patch("flamewm_settings_acceptance.git_snapshot", return_value="same"):
            entry = acc.run_ss16()
        self.assertEqual(entry["status"], "FAIL")
        self.assertEqual(entry["details"]["exit_codes"]["architecture_guard"], 1)


class Ss18InstalledMappingTests(unittest.TestCase):
    def _map(self, document: dict | None) -> dict:
        tmp = Path(tempfile.mkdtemp(prefix="flamewm-ss18-"))
        self.addCleanup(shutil.rmtree, tmp, True)
        path = tmp / "installed-result.json"
        if document is not None:
            import json
            path.write_text(json.dumps(document), encoding="utf-8")
        return acc.map_ss18(str(path))

    def _doc(self, **over) -> dict:
        doc = {"schema": "flamewm-settings-installed-acceptance-v1", "ss18_status": "PASS",
               "behavioral_scenarios": {"SS03": "PASS", "SS12": "PASS"}, "command": ["x"]}
        doc.update(over)
        return doc

    def test_absent_path_is_not_run(self) -> None:
        self.assertEqual(acc.map_ss18(None)["status"], "NOT_RUN")

    def test_launch_smoke_only_is_never_pass(self) -> None:
        doc = self._doc()
        del doc["behavioral_scenarios"]
        self.assertEqual(self._map(doc)["status"], "NOT_RUN")

    def test_behavioral_pass_maps_to_pass(self) -> None:
        self.assertEqual(self._map(self._doc())["status"], "PASS")

    def test_behavioral_fail_maps_to_fail(self) -> None:
        doc = self._doc(behavioral_scenarios={"SS03": "PASS", "SS12": "FAIL"}, ss18_status="FAIL")
        self.assertEqual(self._map(doc)["status"], "FAIL")

    def test_missing_behavioral_pass_is_not_run(self) -> None:
        doc = self._doc(behavioral_scenarios={"SS03": "PASS", "SS12": "NOT_RUN"}, ss18_status="NOT_RUN")
        self.assertEqual(self._map(doc)["status"], "NOT_RUN")

    def test_unknown_schema_is_not_run(self) -> None:
        self.assertEqual(self._map(self._doc(schema="x"))["status"], "NOT_RUN")


class AboutVersionTestRunnerTests(unittest.TestCase):
    """run_about_version_test shells out only to cargo; subprocess and PATH lookup are faked here."""

    def test_cargo_absent_is_not_run_and_never_invokes_cargo(self) -> None:
        with patch("flamewm_settings_acceptance.shutil.which", return_value=None), \
                patch("flamewm_settings_acceptance.subprocess.run") as run:
            observed, record = acc.run_about_version_test()
        self.assertIsNone(observed)
        run.assert_not_called()
        self.assertIn(f"cargo test -p flamewm-settings {acc.ABOUT_VERSION_TEST}", record["command"])
        self.assertIn("cargo not on PATH", record["reason"])

    def test_passing_exact_test_runs_in_root_with_two_jobs(self) -> None:
        completed = SimpleNamespace(returncode=0, stdout="test result: ok. 1 passed; 0 failed\n", stderr="")
        with patch("flamewm_settings_acceptance.shutil.which", return_value="/usr/bin/cargo"), \
                patch("flamewm_settings_acceptance.subprocess.run", return_value=completed) as run:
            observed, record = acc.run_about_version_test()
        self.assertIs(observed, True)
        self.assertEqual(run.call_args.args[0], ["cargo", "test", "-p", "flamewm-settings", acc.ABOUT_VERSION_TEST])
        self.assertEqual(run.call_args.kwargs["cwd"], acc.ROOT)
        self.assertEqual(run.call_args.kwargs["env"]["CARGO_BUILD_JOBS"], "2")
        self.assertEqual(record["exit_code"], 0)
        self.assertEqual(record["result_line"], "test result: ok. 1 passed")

    def test_failed_or_unmatched_test_is_measured_false(self) -> None:
        cases = ((101, "test result: FAILED. 0 passed; 1 failed\n"), (0, "test result: ok. 0 passed; 0 failed\n"))
        for returncode, stdout in cases:
            with self.subTest(returncode=returncode, stdout=stdout), \
                    patch("flamewm_settings_acceptance.shutil.which", return_value="/usr/bin/cargo"), \
                    patch("flamewm_settings_acceptance.subprocess.run",
                          return_value=SimpleNamespace(returncode=returncode, stdout=stdout, stderr="")):
                observed, record = acc.run_about_version_test()
                self.assertIs(observed, False)
                self.assertEqual(record["exit_code"], returncode)
                self.assertIsNone(record["result_line"])

    def test_timeout_is_not_run_not_false(self) -> None:
        with patch("flamewm_settings_acceptance.shutil.which", return_value="/usr/bin/cargo"), \
                patch("flamewm_settings_acceptance.subprocess.run",
                      side_effect=subprocess.TimeoutExpired(cmd="cargo", timeout=1800)):
            observed, record = acc.run_about_version_test()
        self.assertIsNone(observed)
        self.assertEqual(record["reason"], "cargo test timed out")


class WindowTreeScreenCoordinateTests(unittest.TestCase):
    TREE = ('''xwininfo: Window id: 0x230 (the root window) (has no name)
     3 children:
     0x400001 (has no name): ()  820x571+%d+%d  +0+0
        13 children:
        0x40000d (has no name): ()  38x25+782+6  +783+7
        0x800001 "System Settings": ("flamewm-settings" "FlameWM Settings")  820x540+0+31  +1+32
''')

    def test_client_position_includes_moved_frame(self) -> None:
        before = acc.parse_window_tree(self.TREE % (0, 0))
        after = acc.parse_window_tree(self.TREE % (1, 66))
        self.assertEqual((before["x"], before["y"]), (0, 31))
        self.assertEqual((after["x"], after["y"]), (1, 97))  # negative control: relative 0,31 would be stale

    def test_unnamed_windows_are_never_selected(self) -> None:
        self.assertIsNone(acc.parse_window_tree(self.TREE.replace("System Settings", "Terminal")))


class Ss09SourceAndNativeTests(unittest.TestCase):
    OK = {name: "ok" for names in acc.SS09_SOURCE_TESTS.values() for name in names}

    def test_all_named_tests_ok_is_true(self) -> None:
        observed = acc.ss09_source_observations(0, self.OK)
        self.assertTrue(all(value is True for value in observed.values()))

    def test_missing_failed_or_nonzero_is_false_and_not_run_is_none(self) -> None:
        keep = "services::display::tests::keep_clears_pending_and_never_auto_reverts"
        without = {name: word for name, word in self.OK.items() if name != keep}
        self.assertFalse(acc.ss09_source_observations(0, without)["source_keep_prevents_revert"])
        failed = dict(self.OK, **{keep: "FAILED"})
        self.assertFalse(acc.ss09_source_observations(101, failed)["source_keep_prevents_revert"])
        self.assertTrue(all(value is None for value in acc.ss09_source_observations(None, {}).values()))

    def test_cargo_line_parser(self) -> None:
        text = "test a::b ... ok\ntest c::d ... FAILED\nrandom\n"
        self.assertEqual(acc.parse_cargo_test_lines(text), {"a::b": "ok", "c::d": "FAILED"})

    def test_assess_pass_labels_source_evidence_and_fails_on_native_false(self) -> None:
        observations = {key: True for key in acc.SS09_SOURCE_TESTS}
        status, reason = acc.assess_ss09(observations)
        self.assertEqual(status, "PASS")
        self.assertIn("SOURCE-TEST", reason)
        observations["native_timeout_revert"] = False
        self.assertEqual(acc.assess_ss09(observations)[0], "FAIL")
        observations = dict.fromkeys(acc.SS09_SOURCE_TESTS)
        self.assertEqual(acc.assess_ss09(observations)[0], "NOT_RUN")

    def test_native_window_rejects_early_late_and_unchanged(self) -> None:
        self.assertTrue(acc.ss09_native_observed(True, True, 15.2, True))
        self.assertFalse(acc.ss09_native_observed(True, True, 3.0, True))
        self.assertFalse(acc.ss09_native_observed(True, True, 30.0, True))
        self.assertFalse(acc.ss09_native_observed(False, True, 15.2, True))
        self.assertFalse(acc.ss09_native_observed(True, True, None, True))
        self.assertFalse(acc.ss09_native_observed(True, True, 15.2, False))


class Ss08Tests(unittest.TestCase):
    def test_one_output_is_never_pass(self) -> None:
        status, reason = acc.assess_ss08("PASS", {"Xephyr(owned)": 1, "Xvfb": 1}, False, 16)
        self.assertEqual(status, "ENVIRONMENT_BLOCKED")
        self.assertIn("Xephyr(owned)=1", reason)

    def test_provider_with_outputs_is_not_run_not_blocked(self) -> None:
        self.assertEqual(acc.assess_ss08("PASS", {"X": 2}, False, 3)[0], "NOT_RUN")

    def test_failed_one_output_fails(self) -> None:
        self.assertEqual(acc.assess_ss08("FAIL", {"X": 1}, False, 3)[0], "FAIL")

    def test_connected_counter_ignores_disconnected(self) -> None:
        text = "a connected 1x1\nb disconnected\nc connected primary\n"
        self.assertEqual(acc.count_connected(text), 2)


class Ss17Tests(unittest.TestCase):
    def test_canary_check_needs_named_passing_check(self) -> None:
        good = '{"scenario":"J07","pass":true,"checks":[{"name":"t06-real-move","pass":true}]}'
        self.assertTrue(acc.canary_check_passed("noise\n" + good, "t06-real-move"))
        self.assertFalse(acc.canary_check_passed(good, "t07-real-resize-8"))
        bad = good.replace('"pass":true}]', '"pass":false}]')
        self.assertFalse(acc.canary_check_passed(bad, "t06-real-move"))
        self.assertFalse(acc.canary_check_passed("not json", "t06-real-move"))

    def test_hover_needs_difference_and_stable_control(self) -> None:
        self.assertTrue(acc.ss17_hover_observed(0.0, 0.2))
        self.assertFalse(acc.ss17_hover_observed(0.0, 0.0))
        self.assertFalse(acc.ss17_hover_observed(0.05, 0.2))
        self.assertIsNone(acc.ss17_hover_observed(None, 0.2))

    def test_scale_scroll_needs_pass_and_wheel_exit_zero(self) -> None:
        ok = {"status": "PASS", "details": {"scroll": {"exit_code": 0}}}
        self.assertTrue(acc.ss17_scale_scroll_observed(ok))
        self.assertFalse(acc.ss17_scale_scroll_observed({"status": "FAIL", "details": ok["details"]}))
        self.assertIsNone(acc.ss17_scale_scroll_observed({"status": "PASS", "details": {}}))

    def test_matrix_pass_only_when_every_row_true(self) -> None:
        rows = {key: True for key in acc.SS17_REQUIRED}
        self.assertEqual(acc.assess_ss17(rows)[0], "PASS")
        rows["j07_move"] = None
        status, reason = acc.assess_ss17(rows)
        self.assertEqual(status, "NOT_RUN")
        self.assertIn("j07_move", reason)
        rows["j07_move"] = False
        self.assertEqual(acc.assess_ss17(rows)[0], "FAIL")


class Ss06SnapPreviewTests(unittest.TestCase):
    def test_real_difference_passes(self) -> None:
        self.assertTrue(acc.ss06_snap_preview_observed(False, True, 0.9, 0.0, True, False))

    def test_negative_controls(self) -> None:
        cases = {"preview in control": (True, True, 0.9, 0.0, True, False),
                 "no preview at edge": (False, False, 0.9, 0.0, True, False),
                 "no pixel change": (False, True, 0.0, 0.0, True, False),
                 "noisy control": (False, True, 0.9, 0.5, True, False),
                 "not committed": (False, True, 0.9, 0.0, False, False),
                 "preview left behind": (False, True, 0.9, 0.0, True, True)}
        for name, case in cases.items():
            with self.subTest(name):
                self.assertFalse(acc.ss06_snap_preview_observed(*case))
        self.assertIsNone(acc.ss06_snap_preview_observed(False, True, None, 0.0, True, False))


class Ss18IngestTests(unittest.TestCase):
    def _args(self, **kw) -> SimpleNamespace:
        base = {"ss18_installed_result": None, "ss18_prefix": None}
        base.update(kw)
        return SimpleNamespace(**base)

    def test_no_inputs_is_not_run(self) -> None:
        self.assertEqual(acc.ss18_result(self._args(), Path("/nonexistent"))["status"], "NOT_RUN")

    def test_prefix_invokes_installed_gate_and_ingests_its_json(self) -> None:
        tmp = Path(tempfile.mkdtemp(prefix="ss18-"))
        self.addCleanup(shutil.rmtree, tmp, True)
        document = {"schema": "flamewm-settings-installed-acceptance-v1", "ss18_status": "PASS",
                    "behavioral_scenarios": {"SS01": "PASS", "SS03": "PASS"}, "command": ["x"], "prefix": "p"}

        def fake_run(argv, **kwargs):
            run_dir = Path(argv[argv.index("--run-dir") + 1])
            run_dir.mkdir(parents=True)
            (run_dir / "installed-result.json").write_text(__import__("json").dumps(document))
            return SimpleNamespace(returncode=0)

        with patch("flamewm_settings_acceptance.subprocess.run", side_effect=fake_run) as run:
            entry = acc.ss18_result(self._args(ss18_prefix="/staged"), tmp)
        self.assertEqual(entry["status"], "PASS")
        self.assertIn("settings-installed-acceptance", run.call_args.args[0][0])
        self.assertNotIn("FLAMEWM_SS_INNER", run.call_args.kwargs["env"])

    def test_explicit_result_overrides_prefix(self) -> None:
        with patch("flamewm_settings_acceptance.subprocess.run") as run:
            entry = acc.ss18_result(self._args(ss18_prefix="/staged", ss18_installed_result="/nope.json"), Path("/x"))
        run.assert_not_called()
        self.assertEqual(entry["status"], "NOT_RUN")

    def test_prefix_failure_negative_control(self) -> None:
        tmp = Path(tempfile.mkdtemp(prefix="ss18-"))
        self.addCleanup(shutil.rmtree, tmp, True)
        with patch("flamewm_settings_acceptance.subprocess.run", return_value=SimpleNamespace(returncode=3)):
            entry = acc.ss18_result(self._args(ss18_prefix="/staged"), tmp)
        self.assertEqual(entry["status"], "NOT_RUN")


class KeyboardPreflightTests(unittest.TestCase):
    """Stuck-modifier preflight for keyboard-driven scenarios; query and release are faked, never run."""

    def _ctx(self) -> SimpleNamespace:
        return SimpleNamespace(display=":97", token="a" * 64, marker=Path("/nonexistent"), host_env={},
                               spawn=self._no_spawn)

    @staticmethod
    def _no_spawn(*args, **kwargs):
        raise AssertionError("scenario must not start clients when a modifier is stuck")

    def test_clean_keymap_proceeds_unchanged(self) -> None:
        details: dict = {"commands": []}
        with patch.object(acc, "query_modifiers", return_value=("00" * 32, [], "q")) as query, \
                patch.object(acc, "xtest_control") as control:
            self.assertIsNone(acc.keyboard_preflight(self._ctx(), details))
        control.assert_not_called()
        self.assertEqual(query.call_count, 1)
        self.assertEqual(details["held_modifiers_before"], [])
        self.assertNotIn("released_leaked_modifiers", details)

    def test_held_then_released_proceeds_with_visible_leak_evidence(self) -> None:
        details: dict = {"commands": []}
        queries = [("ff" * 32, ["Shift_L"], "q1"), ("00" * 32, [], "q2")]
        with patch.object(acc, "query_modifiers", side_effect=queries), \
                patch.object(acc, "xtest_control", return_value=(SimpleNamespace(returncode=0, stdout=""), "r")) as control:
            self.assertIsNone(acc.keyboard_preflight(self._ctx(), details))
        control.assert_called_once_with(self._ctx(), "--release-modifiers")
        self.assertEqual(details["held_modifiers_before"], ["Shift_L"])
        self.assertEqual(details["held_modifiers_after_release"], [])
        self.assertEqual(details["released_leaked_modifiers"], ["Shift_L"])

    def test_still_held_after_release_is_environment_not_run_reason(self) -> None:
        details: dict = {"commands": []}
        queries = [("ff" * 32, ["Shift_L", "Super_L"], "q1"), ("ff" * 32, ["Shift_L", "Super_L"], "q2")]
        with patch.object(acc, "query_modifiers", side_effect=queries), \
                patch.object(acc, "xtest_control", return_value=(SimpleNamespace(returncode=0, stdout=""), "r")):
            reason = acc.keyboard_preflight(self._ctx(), details)
        self.assertEqual(reason, "environment: stuck modifier Shift_L, Super_L on :97")
        self.assertNotIn("released_leaked_modifiers", details)

    def test_unreadable_keymap_is_environment_not_run_reason(self) -> None:
        details: dict = {"commands": []}
        with patch.object(acc, "query_modifiers", return_value=(None, None, "q")):
            reason = acc.keyboard_preflight(self._ctx(), details)
        self.assertEqual(reason, "environment: keymap unreadable on :97")

    def test_ss12_stuck_modifier_is_not_run_without_input_or_snapshot(self) -> None:
        ctx = self._ctx()
        args = SimpleNamespace(ss12_chord="Super+F8", settle_seconds=0.1, ss12_click=None, ss12_reset_click=None)
        reason = "environment: stuck modifier Shift_L on :97"
        with patch.object(acc.shutil, "which", return_value="/usr/bin/x"), \
                patch.object(acc, "keyboard_preflight", return_value=reason), \
                patch.object(acc, "dbus_shortcuts_snapshot", side_effect=AssertionError("no snapshot")), \
                patch.object(acc, "send_input", side_effect=AssertionError("no input")):
            entry = acc.run_ss12(ctx, args)
        self.assertEqual(entry["status"], "NOT_RUN")
        self.assertEqual(entry["reason"], reason)

    def test_ss15_stuck_modifier_is_not_run_without_input(self) -> None:
        ctx = self._ctx()
        with patch.object(acc, "keyboard_preflight", return_value="environment: stuck modifier Super_R on :97"), \
                patch.object(acc, "send_input", side_effect=AssertionError("no input")):
            entry = acc.run_ss15(ctx, SimpleNamespace(settle_seconds=0.1, settings_bin="/bin/true"))
        self.assertEqual(entry["status"], "NOT_RUN")
        self.assertIn("stuck modifier Super_R", entry["reason"])


class SS05ManagedWallpaperTests(unittest.TestCase):
    def test_store_copy_with_identical_bytes_is_accepted(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = Path(tmp)
            fixture = run / "fixtures" / "wallpaper.png"
            fixture.parent.mkdir(parents=True)
            fixture.write_bytes(b"png-bytes")
            store = run / "xdg-data" / "flamewm" / "wallpapers"
            store.mkdir(parents=True)
            copy = store / "desktop-0123.png"
            copy.write_bytes(b"png-bytes")
            self.assertTrue(acc.ss05_wallpaper_matches_fixture(str(copy), fixture, run))
            self.assertTrue(acc.ss05_wallpaper_matches_fixture(str(fixture), fixture, run))

    def test_store_copy_with_different_bytes_or_outside_store_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            run = Path(tmp)
            fixture = run / "fixtures" / "wallpaper.png"
            fixture.parent.mkdir(parents=True)
            fixture.write_bytes(b"png-bytes")
            store = run / "xdg-data" / "flamewm" / "wallpapers"
            store.mkdir(parents=True)
            wrong = store / "desktop-wrong.png"
            wrong.write_bytes(b"other-bytes")
            outside = run / "elsewhere.png"
            outside.write_bytes(b"png-bytes")
            self.assertFalse(acc.ss05_wallpaper_matches_fixture(str(wrong), fixture, run))
            self.assertFalse(acc.ss05_wallpaper_matches_fixture(str(outside), fixture, run))
            self.assertFalse(acc.ss05_wallpaper_matches_fixture("", fixture, run))
            self.assertFalse(acc.ss05_wallpaper_matches_fixture(None, fixture, run))


if __name__ == "__main__":
    unittest.main()
