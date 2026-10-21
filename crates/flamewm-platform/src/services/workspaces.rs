use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use flamewm_api::ports::WorkspacePort;
use flamewm_api::workspace::{
    MAX_WORKSPACES, WORKSPACE_COUNT_KEY, WorkspacePlan, WorkspaceSnapshot,
};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

use crate::storage::write_atomic_text;
use crate::workspace::WorkspacePlanner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Sibling of the settings file that stores the desired workspace count. It is a separate file
/// because the settings store rewrites only its in-memory unknown keys, so a count stored there
/// would be dropped by the next settings save from the same session.
#[must_use]
pub fn count_store_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name("workspaces.conf")
}

/// Persisted workspace count, or `None` when the file is missing, malformed, or outside 1..=MAX.
#[must_use]
pub fn load_persisted_count(path: &Path) -> Option<usize> {
    let text = fs::read_to_string(path).ok()?;
    let (_, value) = text
        .lines()
        .filter_map(|line| line.trim().split_once('='))
        .find(|(key, _)| key.trim() == WORKSPACE_COUNT_KEY)?;
    let count: usize = value.trim().parse().ok()?;
    (1..=MAX_WORKSPACES).contains(&count).then_some(count)
}

fn persist_count(path: &Path, count: usize) -> FlameResult<()> {
    write_atomic_text(path, &format!("{WORKSPACE_COUNT_KEY}={count}\n")).map_err(io_error)
}

fn io_error(error: io::Error) -> FlameError {
    FlameError::new(ErrorCode::IoFailure, error.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceService {
    count_path: PathBuf,
}

impl WorkspaceService {
    #[must_use]
    pub fn new(count_path: PathBuf) -> Self {
        Self { count_path }
    }

    #[must_use]
    pub fn load_count(&self) -> Option<usize> {
        load_persisted_count(&self.count_path)
    }

    pub fn snapshot<P: WorkspacePort>(&self, port: &P) -> FlameResult<WorkspaceSnapshot> {
        let snapshot = port.workspace_snapshot()?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn activate<P: WorkspacePort>(
        &self,
        port: &mut P,
        index: usize,
        expected_revision: u64,
    ) -> FlameResult<()> {
        let snapshot = self.snapshot(port)?;
        require_revision(&snapshot, expected_revision)?;
        if index >= snapshot.count {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "workspace index is invalid",
            ));
        }
        port.activate_workspace(index, expected_revision)
    }

    pub fn insert_after<P: WorkspacePort>(
        &self,
        port: &mut P,
        index: usize,
        expected_revision: u64,
    ) -> FlameResult<()> {
        let snapshot = self.snapshot(port)?;
        require_revision(&snapshot, expected_revision)?;
        let plan = WorkspacePlanner::insert_after(&snapshot, index)?;
        self.apply_with_count(port, &snapshot, &plan)
    }

    pub fn remove<P: WorkspacePort>(
        &self,
        port: &mut P,
        index: usize,
        expected_revision: u64,
    ) -> FlameResult<()> {
        let snapshot = self.snapshot(port)?;
        require_revision(&snapshot, expected_revision)?;
        let plan = WorkspacePlanner::remove(&snapshot, index)?;
        self.apply_with_count(port, &snapshot, &plan)
    }

    /// Persists the new count before the engine applies the plan, so a persist failure leaves
    /// windows untouched. If the engine rejects the plan, the previous count is restored.
    fn apply_with_count<P: WorkspacePort>(
        &self,
        port: &mut P,
        snapshot: &WorkspaceSnapshot,
        plan: &WorkspacePlan,
    ) -> FlameResult<()> {
        persist_count(&self.count_path, plan.new_count)?;
        if let Err(error) = port.apply_workspace_plan(plan) {
            let _ = persist_count(&self.count_path, snapshot.count);
            return Err(error);
        }
        Ok(())
    }
}

fn require_revision(snapshot: &WorkspaceSnapshot, expected: u64) -> FlameResult<()> {
    if snapshot.revision == expected {
        Ok(())
    } else {
        Err(FlameError::new(
            ErrorCode::StaleRevision,
            "workspace revision is stale",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use flamewm_api::WindowRef;
    use flamewm_api::workspace::{WorkspacePlan, WorkspaceSnapshot};

    use super::*;

    #[derive(Default)]
    struct FakePort {
        snapshot: Option<WorkspaceSnapshot>,
        applied: Vec<WorkspacePlan>,
        fail_apply: bool,
    }

    impl FakePort {
        fn with_count(count: usize) -> Self {
            Self {
                snapshot: Some(snapshot(count, 0)),
                ..Self::default()
            }
        }
    }

    impl WorkspacePort for FakePort {
        fn workspace_snapshot(&self) -> FlameResult<WorkspaceSnapshot> {
            Ok(self.snapshot.clone().expect("snapshot"))
        }
        fn activate_workspace(&mut self, _: usize, _: u64) -> FlameResult<()> {
            Ok(())
        }
        fn move_window_to_workspace(&mut self, _: WindowRef, _: usize) -> FlameResult<()> {
            Ok(())
        }
        fn switch_workspace_with_window(
            &mut self,
            _: WindowRef,
            _: usize,
            _: u64,
        ) -> FlameResult<()> {
            Ok(())
        }
        fn apply_workspace_plan(&mut self, plan: &WorkspacePlan) -> FlameResult<()> {
            if self.fail_apply {
                return Err(FlameError::new(ErrorCode::EngineRejected, "rejected"));
            }
            self.applied.push(plan.clone());
            self.snapshot = Some(snapshot(plan.new_count, plan.new_revision));
            Ok(())
        }
    }

    fn snapshot(count: usize, revision: u64) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            revision,
            count,
            active_index: 0,
            last_index: None,
            names: Vec::new(),
        }
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flamewm-ws-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn insert_persists_count_and_restart_loads_it() {
        let settings = scratch_dir("insert").join("settings.toml");
        let service = WorkspaceService::new(count_store_path(&settings));
        let mut port = FakePort::with_count(4);
        service.insert_after(&mut port, 0, 0).expect("insert");
        assert_eq!(port.applied.len(), 1);
        assert_eq!(load_persisted_count(&count_store_path(&settings)), Some(5));
        assert_eq!(
            fs::read(count_store_path(&settings)).unwrap(),
            b"workspaces.count=5\n"
        );
        let restarted = WorkspaceService::new(count_store_path(&settings));
        assert_eq!(restarted.load_count(), Some(5));
    }

    #[test]
    fn remove_persists_count() {
        let settings = scratch_dir("remove").join("settings.toml");
        let service = WorkspaceService::new(count_store_path(&settings));
        let mut port = FakePort::with_count(4);
        service.remove(&mut port, 1, 0).expect("remove");
        assert_eq!(load_persisted_count(&count_store_path(&settings)), Some(3));
    }

    #[test]
    fn stale_revision_is_rejected_before_any_write() {
        let settings = scratch_dir("stale").join("settings.toml");
        let service = WorkspaceService::new(count_store_path(&settings));
        let mut port = FakePort::with_count(4);
        let error = service.insert_after(&mut port, 0, 7).expect_err("stale");
        assert_eq!(error.code, ErrorCode::StaleRevision);
        assert!(port.applied.is_empty());
        assert!(!count_store_path(&settings).exists());
    }

    #[test]
    fn persist_failure_applies_nothing() {
        let blocker = scratch_dir("unwritable").join("not-a-dir");
        fs::write(&blocker, "file").expect("blocker file");
        let service = WorkspaceService::new(count_store_path(&blocker.join("settings.toml")));
        let mut port = FakePort::with_count(4);
        assert!(service.insert_after(&mut port, 0, 0).is_err());
        assert!(port.applied.is_empty());
        assert_eq!(port.snapshot.as_ref().map(|s| s.count), Some(4));
    }

    #[test]
    fn engine_failure_rolls_persisted_count_back() {
        let settings = scratch_dir("rollback").join("settings.toml");
        let service = WorkspaceService::new(count_store_path(&settings));
        let mut port = FakePort::with_count(4);
        port.fail_apply = true;
        assert!(service.insert_after(&mut port, 0, 0).is_err());
        assert_eq!(load_persisted_count(&count_store_path(&settings)), Some(4));
    }

    #[test]
    fn invalid_or_out_of_range_stored_value_is_ignored() {
        let dir = scratch_dir("invalid");
        let path = dir.join("workspaces.conf");
        for text in [
            "workspaces.count=0\n",
            "workspaces.count=19\n",
            "workspaces.count=abc\n",
            "garbage\n",
        ] {
            fs::write(&path, text).expect("write");
            assert_eq!(load_persisted_count(&path), None, "{text:?}");
        }
        assert_eq!(load_persisted_count(&dir.join("missing.conf")), None);
    }
}
