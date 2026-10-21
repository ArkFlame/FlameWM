"""Pass 59 source/worker contracts. These do not substitute for Rust runtime tests."""
import unittest
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
MAIN=(ROOT/'crates/flamewm-explorer/src/main.rs').read_text()
LIB=(ROOT/'crates/flamewm-explorer/src/lib.rs').read_text()

class BatchOps(unittest.TestCase):
    def test_true_multi_source_batch_worker(self):
        for text in ('Transfer {sources:Vec<PathBuf>,directory:PathBuf,move_it:bool}',
                     'Trash {paths:Vec<PathBuf>}',
                     'transfer_batch(&sources,&directory,move_it,&mut progress)',
                     'for path in &paths',
                     'report.record(path,outcome)',
                     'WorkerResult::BatchCompleted'):
            self.assertIn(text,MAIN)
        self.assertNotIn('Multiple items selected; this action needs a batch handler',MAIN)

    def test_cut_clipboard_clears_only_after_complete_success(self):
        self.assertIn('clipboard: Option<(Vec<PathBuf>, bool)>',LIB)
        self.assertIn('if let Some(sources)=cut_sources.as_ref()',MAIN)
        self.assertIn('report.failed_sources.clone()',MAIN)
        self.assertIn('let paths=self.selected_files()',MAIN)
        self.assertIn('if self.busy',MAIN)

    def test_copy_move_safe_under_conflict_and_partial_failure(self):
        self.assertIn('pub fn transfer_batch(',LIB)
        self.assertIn('outcome.record(source,result)',LIB)
        self.assertIn('self.failed_sources.push(source.to_path_buf())',LIB)
        self.assertIn('rustix::fs::RenameFlags::NOREPLACE',LIB)
        self.assertIn('create_new(true)',LIB)
        self.assertIn('not a regular file',LIB)
        self.assertIn('batch_copy_continues_after_conflict_without_overwriting',LIB)

    def test_keyboard_multiselection_and_rescan_reconciliation(self):
        for text in ('select_range_to','toggle_selection','select_all_filtered','ordered_selection',
                     'selected_paths.retain','rescan_reconciles_selection_of_deleted_files'):
            self.assertIn(text,LIB)
        for text in ('"a" if ctrl','"space"|" " if ctrl','"up"=>self.step_selection(-1,shift)',
                     '"down"=>self.step_selection(1,shift)'):
            self.assertIn(text,MAIN)

    def test_status_reports_partial_completion_and_refreshes(self):
        self.assertIn('report.failed>0',MAIN)
        self.assertIn('if report.completed>0 {self.scan();}',MAIN)
        self.assertIn('self.activity=Some(summary.clone());',MAIN)
        self.assertIn('File operation partially completed',MAIN)

if __name__=='__main__':unittest.main()
