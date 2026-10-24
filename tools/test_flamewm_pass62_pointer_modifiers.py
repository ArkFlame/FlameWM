"""Regression contracts for X11 event-time pointer modifiers and Explorer selection."""
from pathlib import Path
import re
import unittest

ROOT=Path(__file__).resolve().parents[1]
def src(path): return (ROOT/path).read_text()

class PointerSelectionContracts(unittest.TestCase):
    def test_core_event_carries_raw_state_without_new_allocation(self):
        core=src('crates/flamewm-render-core/src/lib.rs')
        backend=src('crates/flamewm-render-x11/src/app/events.rs')
        self.assertRegex(core,r'pub modifiers: u32,')
        self.assertIn('modifiers: u32,\n) -> ActionEvent',backend)
        for native in ('button','motion','key'):
            self.assertRegex(backend,r'(?s)'+re.escape(native)+r'\.time as u64,.{0,95}'+re.escape(native)+r'\.state,')
        self.assertIn('modifiers: 0,',src('crates/flamewm-render-x11/src/surface_controller.rs'))

    def test_ui_facade_forwards_modifiers_and_typed_boolean_semantics(self):
        facade=src('crates/flamewm-ui-x11/src/lib.rs')
        self.assertIn('modifiers: action.modifiers,',facade)
        self.assertIn('pub const fn ctrl_down(&self)',facade)
        self.assertIn('pub const fn shift_down(&self)',facade)
        for fixture in ('crates/flamewm-settings/src/controller.rs','crates/flamewm-settings/src/ui_contract.rs'):
            self.assertIn('modifiers: 0,',src(fixture))

    def test_explorer_has_event_time_mouse_gestures_and_non_destructive_context_click(self):
        app=src('crates/flamewm-explorer/src/main.rs')
        self.assertIn('e.shift_down()',app)
        self.assertIn('e.ctrl_down()',app)
        self.assertIn('self.model.tab_mut().toggle_selection(path)',app)
        self.assertIn('self.model.tab_mut().select_range_to_with_mode(path,e.ctrl_down())',app)
        self.assertLess(app.index('if e.button == 3 {\n                    // Context-click'),app.index('} else if e.button == 1 && e.shift_down()'))
        self.assertIn('self.last_click=None;',app)
        self.assertIn('self.model.tab_mut().clear_selection();',app)

    def test_selection_anchor_preserved_through_repeated_shift_ranges(self):
        model=src('crates/flamewm-explorer/src/lib.rs')
        block=model.split('pub fn select_range_to_with_mode(',1)[1].split('    pub fn select_all_filtered',1)[0]
        self.assertIn('self.anchor.as_deref()',block)
        self.assertIn('if !additive {self.selected_paths.clear();}',block)
        self.assertIn('self.selected_paths.extend(range);',block)
        self.assertNotIn('self.anchor=Some(target)',block)
        self.assertIn('mouse_selection_ctrl_toggle_shift_anchor_and_ctrl_shift_union',src('crates/flamewm-explorer/src/tests/mod.rs'))

if __name__=='__main__':unittest.main()
