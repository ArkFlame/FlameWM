"""Pass 63 source-owner gates. Requires Rust Cargo and live X11 for complete validation."""
import unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]

class InternalDragDropContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.model=(ROOT/'crates/flamewm-explorer/src/lib.rs').read_text()
        cls.app=(ROOT/'crates/flamewm-explorer/src/main.rs').read_text()

    def test_uses_existing_pressed_row_and_canonical_ui_modifiers(self):
        for snippet in ('UiActionPhase::Press if e.button==1',
                        'UiActionPhase::Motion', 'UiActionPhase::Release',
                        'struct RowDrag', 'DRAG_THRESHOLD_PX:f32=7.0',
                        'fn finish_row_drag', 'force_copy:e.ctrl_down()'):
            self.assertIn(snippet, self.app)

    def test_validated_drop_targets_and_visible_feedback(self):
        for snippet in ('fn folder_at(&self,x:f32,y:f32)',
                        'FILE_LIST_LEFT', 'entry.directory',
                        'self.drop_folder=self.folder_at(e.x,e.y)',
                        'self.drop_folder.as_deref()==Some(entry.path.as_path())',
                        'Drop cancelled (choose a folder)',
                        'Drag onto a folder to move'):
            self.assertIn(snippet, self.app)

    def test_noreplace_and_symlink_recursive_guards(self):
        for snippet in ('validate_transfer_destination',
                        'fs::canonicalize(directory)', 'fs::canonicalize(source)',
                        'destination_parent.starts_with(source)',
                        'RenameFlags::NOREPLACE', 'transfer_rejects_symlink_alias_to_child_before_writing'):
            self.assertIn(snippet, self.model)

    def test_cross_device_drop_defaults_to_copy_and_cut_remains_move(self):
        for snippet in ('pub fn drop_moves(', 'source_dev==destination_dev',
                        'pub fn transfer_drop_batch(', 'metadata.dev()',
                        'drop_moves(force_copy,src,dst)',
                        'drop_policy_moves_on_same_device_copies_cross_device_and_respects_ctrl'):
            self.assertIn(snippet, self.model)
        self.assertIn('WorkerJob::Drop{sources,directory,force_copy}', self.app)
        self.assertIn('WorkerJob::Transfer{sources,directory,move_it}', self.app)
        self.assertIn('cut_sources:None', self.app)

    def test_existing_mime_work_and_navigation_workers_preserved(self):
        for snippet in ('ChooserJob::Discover', 'chooser_worker(', 'scan_worker(',
                        'transfer_batch(&sources,&directory,move_it,&mut progress)',
                        'self.model.tab_mut().toggle_selection(path)',
                        'self.model.tab_mut().select_range_to_with_mode(path,e.ctrl_down())'):
            self.assertIn(snippet, self.app)

if __name__=='__main__':unittest.main()
