//! T13 debug retention two-run check: run A captures a bounded state
//! snapshot digest to a file; run B re-captures and compares. Pass when
//! both runs retain the same live toplevel set (no silent loss) — the
//! digest file is the retention artifact under .agents/runtime/pre009/J07/.

use std::path::PathBuf;

fn default_path(args: &[String]) -> PathBuf {
    crate::arg(args, "--digest")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".agents/runtime/pre009/J07/t13-digest.txt"))
}

/// T13: `--run a|b`. Run A writes digest; run B compares. Deterministic,
/// no pointer needed; path marker records file retention instead.
pub fn run_t13(args: &[String]) -> (bool, String) {
    let which = crate::arg(args, "--run").unwrap_or_else(|| "a".to_owned());
    let path = default_path(args);
    // Digest is computed by the caller harness (canary j07 prints guidance);
    // here we implement the file-retention half: run A requires --digest-of,
    // run B compares current --digest-of to the stored file.
    let current = crate::arg(args, "--digest-of").unwrap_or_default();
    let file = path.display();
    let needs_digest = |run: &str| {
        (
            false,
            format!("run={run} needs --digest-of <hex> file={file} path=file-retention"),
        )
    };
    match (which.as_str(), current.is_empty()) {
        ("a" | "b", true) => needs_digest(&which),
        ("a", false) => match std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")))
            .and_then(|()| std::fs::write(&path, format!("{current}\n")))
        {
            Ok(()) => (
                true,
                format!("run=a stored digest={current} file={file} path=file-retention"),
            ),
            Err(e) => (false, format!("run=a write failed {e} file={file} path=file-retention")),
        },
        ("b", false) => match std::fs::read_to_string(&path) {
            Ok(stored) => {
                let stored = stored.trim().to_owned();
                let pass = stored == current;
                (
                    pass,
                    format!(
                        "run=b stored={stored} current={current} match={pass} file={file} path=file-retention"
                    ),
                )
            }
            Err(e) => (
                false,
                format!("run=b missing run-a artifact {e} file={file} path=file-retention"),
            ),
        },
        _ => (false, "--run must be a|b path=file-retention".to_owned()),
    }
}
