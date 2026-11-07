//! On-disk cache of compiled UI documents, keyed by a hash of every compile input (see
//! `compile.rs`). A missing, unreadable, or invalid entry is never an error: the caller compiles
//! from source instead. Entries are capped in size and count.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use flamewm_render_core::{decode, encode, CompiledDocument};

/// Largest encoded document that is cached. Larger documents recompile on every load.
const MAX_ENTRY_BYTES: usize = 4 << 20;
/// Entries kept per cache directory. The least recently written are pruned after each store.
const MAX_ENTRIES: usize = 32;
const ENTRY_EXTENSION: &str = "fwc";

/// `$XDG_CACHE_HOME/flamewm/render`, else `$HOME/.cache/flamewm/render`. `None` without `$HOME`.
pub fn default_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(base.join("flamewm").join("render"))
}

/// Returns the cached document for `key`, or `None` on any miss or invalid entry.
pub fn load(dir: &Path, key: u64) -> Option<CompiledDocument> {
    let bytes = fs::read(entry_path(dir, key)).ok()?;
    let document = decode(&bytes).ok()?;
    document.validate().ok()?;
    Some(document)
}

/// Best-effort store: a failed write only costs a recompile on the next load.
pub fn store(dir: &Path, key: u64, document: &CompiledDocument) {
    let Ok(bytes) = encode(document) else {
        return;
    };
    if bytes.len() > MAX_ENTRY_BYTES || fs::create_dir_all(dir).is_err() {
        return;
    }
    // Per-process temp name so concurrent writers never share a partial file.
    let temp = dir.join(format!("{key:016x}.{}.tmp", std::process::id()));
    let path = entry_path(dir, key);
    if fs::write(&temp, &bytes)
        .and_then(|()| fs::rename(&temp, &path))
        .is_err()
    {
        let _ = fs::remove_file(&temp);
        return;
    }
    prune(dir);
}

fn entry_path(dir: &Path, key: u64) -> PathBuf {
    dir.join(format!("{key:016x}.{ENTRY_EXTENSION}"))
}

fn prune(dir: &Path) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(SystemTime, PathBuf)> = read
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == ENTRY_EXTENSION))
        .filter_map(|path| Some((fs::metadata(&path).ok()?.modified().ok()?, path)))
        .collect();
    if entries.len() <= MAX_ENTRIES {
        return;
    }
    entries.sort();
    for (_, path) in &entries[..entries.len() - MAX_ENTRIES] {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile_str;
    use crate::CompileOptions;

    fn fixture_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flamewm-cache-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn document(width: u32) -> CompiledDocument {
        compile_str(
            &format!("<body><div style='width:{width}px'></div></body>"),
            "",
            CompileOptions::default(),
        )
        .unwrap()
        .document
    }

    #[test]
    fn stored_document_loads_back_identically() {
        let dir = fixture_dir("roundtrip");
        let doc = document(10);
        store(&dir, 7, &doc);
        let loaded = load(&dir, 7).expect("stored entry loads");
        assert_eq!(encode(&loaded).unwrap(), encode(&doc).unwrap());
    }

    #[test]
    fn missing_and_corrupt_entries_are_misses() {
        let dir = fixture_dir("miss");
        assert!(load(&dir, 1).is_none(), "absent entry is a miss");
        fs::create_dir_all(&dir).unwrap();
        fs::write(entry_path(&dir, 2), b"not a document").unwrap();
        assert!(load(&dir, 2).is_none(), "corrupt entry is a miss");
        fs::write(entry_path(&dir, 3), &encode(&document(4)).unwrap()[..20]).unwrap();
        assert!(load(&dir, 3).is_none(), "truncated entry is a miss");
    }

    #[test]
    fn entry_count_is_bounded() {
        let dir = fixture_dir("bounded");
        let doc = document(1);
        for key in 0..(MAX_ENTRIES as u64 + 8) {
            store(&dir, key, &doc);
        }
        let count = fs::read_dir(&dir).unwrap().count();
        assert!(count <= MAX_ENTRIES, "{count} entries kept");
        assert!(
            load(&dir, MAX_ENTRIES as u64 + 7).is_some(),
            "newest entry survives"
        );
    }
}
