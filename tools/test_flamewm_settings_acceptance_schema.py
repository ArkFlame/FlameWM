"""Pure unit tests for the settings acceptance schema and XTEST held-key option.

No X server, Xephyr, or display is touched. Run with:
    python3 -I tools/test_flamewm_settings_acceptance_schema.py
"""

from __future__ import annotations

import sys
import shutil
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path

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


if __name__ == "__main__":
    unittest.main()
