use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) fn write_atomic_text(path: &Path, text: &str) -> io::Result<()> {
    if path.as_os_str().is_empty() || path.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "atomic text path must name a file",
        ));
    }
    let parent = parent_dir(path);
    fs::create_dir_all(parent)?;
    for _ in 0..16 {
        let temp = temp_path(path);
        match write_atomic_with(
            path,
            &temp,
            |file| file.write_all(text.as_bytes()),
            |parent| {
                if let Ok(directory) = File::open(parent) {
                    let _ = directory.sync_all();
                }
            },
        ) {
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            result => return result,
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a unique atomic-write temporary file",
    ))
}

fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .expect("validated file path")
        .to_os_string();
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    parent_dir(path).join(name)
}

fn write_atomic_with<W, S>(path: &Path, temp: &Path, write: W, sync_parent: S) -> io::Result<()>
where
    W: FnOnce(&mut File) -> io::Result<()>,
    S: FnOnce(&Path),
{
    let parent = parent_dir(path);
    if parent_dir(temp) != parent {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "atomic-write temporary file must be a sibling",
        ));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(temp)?;
    let result = (|| {
        write(&mut file)?;
        file.sync_all()?;
        drop(file);
        fs::rename(temp, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(temp);
        return Err(error);
    }
    sync_parent(parent);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("flamewm-storage-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create fixture dir");
        path
    }

    #[test]
    fn atomic_replace_keeps_exact_bytes_and_confidential_permissions() {
        let dir = temp_dir("permissions");
        let path = dir.join("settings.conf");
        write_atomic_text(&path, "revision=7\naccent=#112233\n").expect("first commit");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("relax old mode");
        write_atomic_text(&path, "revision=8\naccent=#445566\n").expect("replace");

        assert_eq!(fs::read(&path).unwrap(), b"revision=8\naccent=#445566\n");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn failed_write_preserves_old_file_and_removes_partial_temp() {
        let dir = temp_dir("failure");
        let path = dir.join("workspaces.conf");
        let temp = dir.join("workspaces.conf.partial.tmp");
        fs::write(&path, b"workspaces.count=4\n").unwrap();
        let error = write_atomic_with(
            &path,
            &temp,
            |file| {
                file.write_all(b"partial")?;
                Err(io::Error::other("injected failure"))
            },
            |_| panic!("parent must not sync before commit"),
        )
        .expect_err("injected write failure");

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(fs::read(&path).unwrap(), b"workspaces.count=4\n");
        assert!(!temp.exists());
    }

    #[test]
    fn existing_temp_collision_is_preserved_and_parent_sync_follows_commit() {
        let dir = temp_dir("collision");
        let path = dir.join("settings.conf");
        let temp = dir.join("settings.conf.collision.tmp");
        fs::write(&path, b"old\n").unwrap();
        fs::write(&temp, b"keep\n").unwrap();
        let error = write_atomic_with(
            &path,
            &temp,
            |_| panic!("collision must fail before writing"),
            |_| panic!("parent must not sync after a failed commit"),
        )
        .expect_err("temporary collision");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&temp).unwrap(), b"keep\n");
        assert_eq!(fs::read(&path).unwrap(), b"old\n");

        fs::remove_file(&temp).unwrap();
        let synced = AtomicBool::new(false);
        write_atomic_with(
            &path,
            &temp,
            |file| file.write_all(b"new\n"),
            |parent| {
                assert_eq!(parent, dir.as_path());
                assert_eq!(fs::read(&path).unwrap(), b"new\n");
                synced.store(true, Ordering::SeqCst);
            },
        )
        .expect("commit");
        assert!(synced.load(Ordering::SeqCst));
    }
}
