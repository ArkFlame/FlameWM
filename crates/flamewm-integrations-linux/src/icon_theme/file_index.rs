//! Icon file indexing from theme directory listings.

use super::*;

/// Generation-scoped shared lookup snapshot.
///
/// Built once per `(theme_names, data_dirs, generation)` and shared by all
/// resolver workers. Holds search roots, parsed `index.theme` per theme in
/// the inheritance chain, per-theme roots, a real `base-name -> existing
/// files` index (each declared dir read once at build), pixmaps recorded at
/// build, and a bounded `name -> paths` memo so repeated lookups are
/// memory-only. Interior memo uses a `Mutex`; snapshots are replaced (never
/// mutated) on generation bumps.
/// One existing icon file, 8 bytes. The base name is the [`IconFileIndex`]
/// key; the directory (with its theme and size rank) is interned once in
/// `IconFileIndex::dirs`, and the path is rebuilt on demand.
#[derive(Clone, Copy, Debug)]
pub(super) struct IndexedIconFile {
    /// Index into `IconFileIndex::dirs` (and `dir_meta`).
    pub(super) dir: u32,
    /// Index into `IconFileIndex::exts` (extension as spelled on disk).
    pub(super) ext: u16,
    /// File name is not valid UTF-8: `dirs[dir]` then holds the full path.
    pub(super) raw_name: bool,
    /// Index into `IconLookupIndex::EXTENSIONS` (probe order).
    pub(super) format_rank: u8,
}

/// Theme chain position and directory section of one interned directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct IconDirMeta {
    pub(super) theme_rank: u32,
    pub(super) directory_rank: u32,
}

/// One base name of [`IconFileIndex`]: its bytes in `name_bytes` and its
/// files in `entries`. The end of each range is the next record's start, so
/// the list ends with a sentinel record.
#[derive(Clone, Copy, Debug)]
pub(super) struct IndexedIconName {
    pub(super) name_start: u32,
    pub(super) entry_start: u32,
}

/// Real name -> existing-files index. Each file is an 8-byte record; base
/// names, directories and extensions are stored once. Base names share one
/// string and are looked up by binary search, so the index makes no
/// allocation per name.
#[derive(Debug, Default)]
pub struct IconFileIndex {
    pub(super) name_bytes: String,
    /// Sorted by base name, plus one sentinel past the last name.
    pub(super) names: Vec<IndexedIconName>,
    pub(super) entries: Vec<IndexedIconFile>,
    pub(super) dirs: Vec<PathBuf>,
    pub(super) dir_meta: Vec<IconDirMeta>,
    pub(super) exts: Vec<String>,
}

impl IconFileIndex {
    pub(super) fn name_count(&self) -> usize {
        self.names.len().saturating_sub(1)
    }

    pub(super) fn name(&self, index: usize) -> &str {
        let start = self.names[index].name_start as usize;
        let end = self.names[index + 1].name_start as usize;
        &self.name_bytes[start..end]
    }

    pub(super) fn get(&self, base: &str) -> Option<&[IndexedIconFile]> {
        let (mut lo, mut hi) = (0, self.name_count());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.name(mid) < base {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == self.name_count() || self.name(lo) != base {
            return None;
        }
        let start = self.names[lo].entry_start as usize;
        let end = self.names[lo + 1].entry_start as usize;
        self.entries.get(start..end)
    }

    pub(super) fn meta(&self, file: IndexedIconFile) -> IconDirMeta {
        self.dir_meta[file.dir as usize]
    }

    pub(super) fn ext(&self, file: IndexedIconFile) -> &str {
        &self.exts[file.ext as usize]
    }

    /// Rebuilds the on-disk path of `file` whose base name is `base`.
    pub(super) fn path(&self, base: &str, file: IndexedIconFile) -> PathBuf {
        let dir = &self.dirs[file.dir as usize];
        if file.raw_name {
            return dir.clone();
        }
        dir.join(format!("{base}.{}", self.ext(file)))
    }
}

/// A recorded file before grouping: its name id and rel-dir id (the rel id
/// dedupes the same theme dir seen under another search root).
#[derive(Clone, Copy, Debug)]
pub(super) struct PendingIconFile {
    pub(super) name: u32,
    pub(super) rel: u32,
    pub(super) file: IndexedIconFile,
}

/// Build-time accumulator for [`IconFileIndex`]. Records are kept flat (one
/// small record per file) and grouped by name once in `finish`.
#[derive(Default)]
pub(super) struct IconFileBuilder {
    pub(super) name_ids: HashMap<Box<str>, u32>,
    pub(super) recs: Vec<PendingIconFile>,
    pub(super) rel_ids: HashMap<PathBuf, u32>,
    pub(super) dirs: Vec<PathBuf>,
    pub(super) dir_meta: Vec<IconDirMeta>,
    /// Interned directory ids per path (one path can hold several metas).
    pub(super) dir_ids: HashMap<PathBuf, Vec<(IconDirMeta, u32)>>,
    pub(super) exts: Vec<String>,
    pub(super) ext_ids: HashMap<String, u16>,
}

impl IconFileBuilder {
    /// Records one supported icon file in `dir`; `rel` is `dir` relative to
    /// its theme root.
    pub(super) fn push(
        &mut self,
        theme_rank: usize,
        directory_rank: usize,
        rel: &Path,
        dir: &Path,
        file_name: &OsStr,
    ) {
        let lossy = file_name.to_string_lossy();
        let Some((base, ext)) = split_icon_filename(&lossy) else {
            return;
        };
        // Only supported extensions are indexed (both for indexed and walked themes).
        let Some(format_rank) = IconLookupIndex::EXTENSIONS.iter().position(|e| *e == ext) else {
            return;
        };
        let meta = IconDirMeta {
            theme_rank: u32::try_from(theme_rank).unwrap_or(u32::MAX),
            directory_rank: u32::try_from(directory_rank).unwrap_or(u32::MAX),
        };
        let raw_name = file_name.to_str().is_none();
        let rel_id = if let Some(&id) = self.rel_ids.get(rel) {
            id
        } else {
            let id = self.rel_ids.len() as u32;
            self.rel_ids.insert(rel.to_path_buf(), id);
            id
        };
        let name_id = if let Some(&id) = self.name_ids.get(base.as_str()) {
            id
        } else {
            let id = self.name_ids.len() as u32;
            self.name_ids.insert(base.as_str().into(), id);
            id
        };
        let raw_path;
        let dir = if raw_name {
            raw_path = dir.join(file_name);
            raw_path.as_path()
        } else {
            dir
        };
        let dir_id = self.intern_dir(dir, meta);
        let ext_orig = &lossy[lossy.len() - ext.len()..];
        let ext_id = if let Some(&id) = self.ext_ids.get(ext_orig) {
            id
        } else {
            let id = self.exts.len() as u16;
            self.exts.push(ext_orig.to_owned());
            self.ext_ids.insert(ext_orig.to_owned(), id);
            id
        };
        self.recs.push(PendingIconFile {
            name: name_id,
            rel: rel_id,
            file: IndexedIconFile {
                dir: dir_id,
                ext: ext_id,
                raw_name,
                format_rank: format_rank as u8,
            },
        });
    }

    pub(super) fn intern_dir(&mut self, path: &Path, meta: IconDirMeta) -> u32 {
        if let Some(found) = self
            .dir_ids
            .get(path)
            .and_then(|ids| ids.iter().find(|(known, _)| *known == meta))
        {
            return found.1;
        }
        let id = self.dirs.len() as u32;
        self.dirs.push(path.to_path_buf());
        self.dir_meta.push(meta);
        self.dir_ids
            .entry(path.to_path_buf())
            .or_default()
            .push((meta, id));
        id
    }

    pub(super) fn finish(self) -> IconFileIndex {
        let IconFileBuilder {
            name_ids,
            mut recs,
            dirs,
            dir_meta,
            exts,
            ..
        } = self;
        let mut sorted_names: Vec<(Box<str>, u32)> = name_ids.into_iter().collect();
        sorted_names.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        let mut rank = vec![0u32; sorted_names.len()];
        for (pos, (_, id)) in sorted_names.iter().enumerate() {
            rank[*id as usize] = pos as u32;
        }
        // Stable: files of one name keep their build order (lookup ties rely on it).
        recs.sort_by_key(|rec| rank[rec.name as usize]);
        let name_total: usize = sorted_names.iter().map(|(name, _)| name.len()).sum();
        let mut name_bytes = String::with_capacity(name_total);
        let mut names = Vec::with_capacity(sorted_names.len() + 1);
        let mut entries: Vec<IndexedIconFile> = Vec::with_capacity(recs.len());
        let mut kept: Vec<PendingIconFile> = Vec::new();
        let mut next = 0;
        for (pos, (name, _)) in sorted_names.iter().enumerate() {
            names.push(IndexedIconName {
                name_start: name_bytes.len() as u32,
                entry_start: entries.len() as u32,
            });
            name_bytes.push_str(name);
            kept.clear();
            while next < recs.len() && rank[recs[next].name as usize] == pos as u32 {
                let rec = recs[next];
                next += 1;
                // Same theme dir under another search root: keep the first.
                // Raw names dedupe on the exact on-disk name, never the lossy base.
                let duplicate = kept.iter().any(|old| {
                    let (old_dir, new_dir) = (old.file.dir as usize, rec.file.dir as usize);
                    old.rel == rec.rel
                        && dir_meta[old_dir].theme_rank == dir_meta[new_dir].theme_rank
                        && old.file.format_rank == rec.file.format_rank
                        && old.file.raw_name == rec.file.raw_name
                        && (!rec.file.raw_name
                            || dirs[old_dir].file_name() == dirs[new_dir].file_name())
                });
                if !duplicate {
                    kept.push(rec);
                    entries.push(rec.file);
                }
            }
        }
        names.push(IndexedIconName {
            name_start: name_bytes.len() as u32,
            entry_start: entries.len() as u32,
        });
        IconFileIndex {
            name_bytes,
            names,
            entries,
            dirs,
            dir_meta,
            exts,
        }
    }
}
