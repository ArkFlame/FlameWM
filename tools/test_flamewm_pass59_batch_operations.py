"""Pass 59 source/worker contracts. These do not substitute for Rust runtime tests."""
import unittest
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]

_EXPL_BASE = ROOT / 'crates/flamewm-explorer/src'


def _explorer_crate():
    """Explorer was split into submodules; owners span the crate (tests excluded)."""
    return '\n'.join(p.read_text() for p in sorted(_EXPL_BASE.rglob('*.rs'))
                     if 'tests' not in p.relative_to(_EXPL_BASE).parts)


MAIN=_explorer_crate()
LIB=MAIN
TESTS='\n'.join(p.read_text(errors='ignore') for p in (ROOT/'crates/flamewm-explorer/src/tests').rglob('*.rs'))

class BatchOps(unittest.TestCase):
    def test_true_multi_source_batch_worker(self):
        for text in ('Transfer {sources:Vec<PathBuf>,directory:PathBuf,move_it:bool,',
                     'Trash {paths:Vec<PathBuf>}',
                     'transfer_batch_with(&sources,&directory,move_it,conflict,&cancel,reporter)',
                     'for path in &paths',
                     'report.record(path,removed)',
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
        self.assertIn('outcome.record(source, Err(error));',MAIN)  # per-source outcome lives in jobs.rs (MAIN is the whole crate)
        self.assertIn('self.failed_sources.push(source.to_path_buf())',LIB)
        self.assertIn('RenameFlags::NOREPLACE',LIB)
        self.assertIn('create_new(true)',LIB)
        self.assertIn('not a regular file',LIB)
        self.assertIn('fn batch_copy_continues_after_conflict_without_overwriting', TESTS)

    def test_keyboard_multiselection_and_rescan_reconciliation(self):
        for text in ('select_range_to','toggle_selection','select_all_filtered','ordered_selection',
                     'selected_paths.retain'):
            self.assertIn(text,LIB)
        self.assertIn('fn rescan_reconciles_selection_of_deleted_files', TESTS)
        for text in ('"a" if ctrl','"space"|" " if ctrl','"up"=>self.step_selection(-1,shift)',
                     '"down"=>self.step_selection(1,shift)'):
            self.assertIn(text,MAIN)

    def test_status_reports_partial_completion_and_refreshes(self):
        self.assertIn('report.failed>0',MAIN)
        self.assertIn('if report.completed>0 {self.scan();}',MAIN)
        self.assertIn('self.activity=Some(summary.clone());',MAIN)
        self.assertIn('File operation partially completed',MAIN)

if __name__=='__main__':unittest.main()
