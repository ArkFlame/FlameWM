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
            "blue_snapshot", "teal_snapshot", "reset_snapshot", "selected_visuals",
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

    def test_ss03_requires_two_palette_values_visual_reset_and_restart(self) -> None:
        complete = {key: True for key in (
            "blue_snapshot", "teal_snapshot", "reset_snapshot", "selected_visuals",
            "reset_persisted_after_restart")}
        self.assertEqual(acc.assess_ss03(complete)[0], "PASS")
        for missing in complete:
            partial = dict(complete)
            partial[missing] = None
            with self.subTest(missing=missing):
                self.assertEqual(acc.assess_ss03(partial)[0], "NOT_RUN")

    def test_ss03_measured_wrong_reset_is_failure(self) -> None:
        complete = {key: True for key in (
            "blue_snapshot", "teal_snapshot", "reset_snapshot", "selected_visuals",
            "reset_persisted_after_restart")}
        complete["reset_snapshot"] = False
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

    def test_ss13_needs_three_renders_and_version(self) -> None:
        observations = {"about_render_760x500": True, "about_render_820x540": True,
                        "about_render_960x640": True, "version_matches_flamewm_version": True}
        self.assertEqual(acc.assess_ss13(observations)[0], "PASS")
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
        codes["static_audit"] = None
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


if __name__ == "__main__":
    unittest.main()
