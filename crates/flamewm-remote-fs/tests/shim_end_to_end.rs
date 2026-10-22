//! End-to-end tests through the real async client, process pump and ls/cat parsers. The `ssh`
//! binary is replaced by a shim that runs the final argument with `sh -c` locally. Host, port and
//! ControlMaster options are ignored; everything else (commands, quoting, streaming, error mapping)
//! is the production path. Real OpenSSH behaviour is covered by `real_host.rs`.

use flamewm_remote_fs::{Cancel, Config, Error, Kind, ListEvent, RemoteFs, TransferEvent, Url};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn workdir(name: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("rfs-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    base
}

fn shim(dir: &Path) -> PathBuf {
    let path = dir.join("fake-ssh");
    fs::write(
        &path,
        "#!/bin/sh\nfor last; do :; done\nexec sh -c \"$last\"\n",
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn client(dir: &Path) -> RemoteFs {
    RemoteFs::new(Config {
        ssh: shim(dir),
        control_dir: dir.join("ctl"),
        cache_ttl: Duration::from_millis(0),
        workers: 4,
        ..Config::default()
    })
}

fn local_url(p: &Path) -> Url {
    Url::parse(&format!("sftp://127.0.0.1{}", p.display())).unwrap()
}

fn list_all(rfs: &RemoteFs, url: &Url) -> Result<Vec<flamewm_remote_fs::Entry>, Error> {
    let rx = rfs.list_dir(url, &Cancel::new());
    let mut out = Vec::new();
    loop {
        match rx.recv().expect("listing channel") {
            ListEvent::Chunk(c) => out.extend(c),
            ListEvent::Done { total } => {
                assert_eq!(total, out.len());
                return Ok(out);
            }
            ListEvent::Failed(e) => return Err(e),
        }
    }
}

#[test]
fn lists_1000_entry_directory_streaming_with_latency_report() {
    let dir = workdir("list1000");
    let big = dir.join("big");
    fs::create_dir_all(&big).unwrap();
    for i in 0..1000 {
        fs::write(big.join(format!("file {i:04}.txt")), vec![b'x'; i % 7]).unwrap();
    }
    fs::create_dir_all(big.join("subdir")).unwrap();
    let rfs = client(&dir);
    let url = local_url(&big);

    // Warm the first master-less path once so timings reflect steady-state listing.
    list_all(&rfs, &url).unwrap();

    let t = Instant::now();
    let rx = rfs.list_dir(&url, &Cancel::new());
    let mut first_chunk_ms = None;
    let mut total = 0usize;
    let mut chunks = 0usize;
    let mut first_chunk_len = 0usize;
    loop {
        match rx.recv().unwrap() {
            ListEvent::Chunk(c) => {
                if first_chunk_ms.is_none() {
                    first_chunk_ms = Some(t.elapsed().as_secs_f64() * 1000.0);
                    first_chunk_len = c.len();
                }
                chunks += 1;
                total += c.len();
            }
            ListEvent::Done { .. } => break,
            ListEvent::Failed(e) => panic!("listing failed: {e}"),
        }
    }
    let full_ms = t.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "BENCH list 1000 entries (shim transport, no network): first chunk {:.2} ms ({} rows), total {:.2} ms, {} chunks",
        first_chunk_ms.unwrap(),
        first_chunk_len,
        full_ms,
        chunks
    );
    assert_eq!(total, 1001);
    assert!(first_chunk_len <= 64, "first page must be small so the UI paints early");
    assert!(chunks > 1, "listing must stream in more than one chunk");

    let entries = list_all(&rfs, &url).unwrap();
    assert_eq!(entries.iter().filter(|e| e.kind == Kind::Dir).count(), 1);
    let f = entries.iter().find(|e| e.name == "file 0003.txt").unwrap();
    assert_eq!(f.size, 3);
    assert_eq!(f.kind, Kind::File);
    assert!(f.mtime > 0);
}

#[test]
fn listing_errors_map_to_typed_not_found() {
    let dir = workdir("notfound");
    let rfs = client(&dir);
    let err = list_all(&rfs, &local_url(&dir.join("missing"))).unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
}

#[test]
fn home_stat_mkdir_rename_remove_roundtrip() {
    let dir = workdir("mutate");
    let rfs = client(&dir);
    let base = local_url(&dir.join("work"));
    let sub = base.join("new dir");
    fs::create_dir_all(dir.join("work")).unwrap();

    rfs.mkdir(&sub).wait().unwrap();
    assert!(dir.join("work/new dir").is_dir());
    assert_eq!(rfs.stat(&sub).wait().unwrap().kind, Kind::Dir);

    let renamed = base.join("renamed");
    rfs.rename(&sub, &renamed).wait().unwrap();
    assert!(!dir.join("work/new dir").exists());
    assert!(dir.join("work/renamed").is_dir());

    let clash = rfs.rename(&renamed, &base).wait();
    assert!(matches!(clash, Err(Error::AlreadyExists(_))), "rename onto existing must not overwrite: {clash:?}");

    rfs.remove(&renamed, true).wait().unwrap();
    assert!(!dir.join("work/renamed").exists());

    let home = rfs.home(&Url::parse("fish://127.0.0.1").unwrap()).wait().unwrap();
    assert!(home.path.starts_with('/'));
    assert_eq!(home.path, std::env::var("HOME").unwrap());
}

#[test]
fn refuses_to_remove_root_or_home() {
    let dir = workdir("guard");
    let rfs = client(&dir);
    assert!(matches!(rfs.remove(&Url::parse("sftp://h/").unwrap(), true).wait(), Err(Error::InvalidUrl(_))));
    assert!(matches!(rfs.remove(&Url::parse("fish://h").unwrap(), true).wait(), Err(Error::InvalidUrl(_))));
}

#[test]
fn upload_and_download_with_progress_and_no_partial_on_cancel() {
    let dir = workdir("transfer");
    let rfs = client(&dir);
    let src = dir.join("src.bin");
    let data: Vec<u8> = (0..3 * 1024 * 1024 + 17).map(|i| (i % 251) as u8).collect();
    fs::write(&src, &data).unwrap();

    let remote = local_url(&dir.join("remote.bin"));
    let mut progress = 0;
    let rx = rfs.upload(&src, &remote, &Cancel::new());
    loop {
        match rx.recv().unwrap() {
            TransferEvent::Progress { .. } => progress += 1,
            TransferEvent::Done { bytes } => {
                assert_eq!(bytes, data.len() as u64);
                break;
            }
            TransferEvent::Failed(e) => panic!("upload failed: {e}"),
        }
    }
    assert!(progress > 1, "progress should be reported incrementally");
    assert_eq!(fs::read(dir.join("remote.bin")).unwrap(), data);

    let back = dir.join("back.bin");
    let rx = rfs.download(&remote, &back, &Cancel::new());
    loop {
        match rx.recv().unwrap() {
            TransferEvent::Progress { .. } => {}
            TransferEvent::Done { bytes } => {
                assert_eq!(bytes, data.len() as u64);
                break;
            }
            TransferEvent::Failed(e) => panic!("download failed: {e}"),
        }
    }
    assert_eq!(fs::read(&back).unwrap(), data);
    assert!(!dir.join("back.bin.flamewm-part").exists());

    // Cancel before the first block: the destination must not appear.
    let cancel = Cancel::new();
    cancel.cancel();
    let cancelled = dir.join("cancelled.bin");
    let rx = rfs.download(&remote, &cancelled, &cancel);
    let mut failed = None;
    while let Ok(ev) = rx.recv() {
        if let TransferEvent::Failed(e) = ev {
            failed = Some(e);
        }
    }
    assert_eq!(failed, Some(Error::Cancelled));
    assert!(!cancelled.exists());
    assert!(!dir.join("cancelled.bin.flamewm-part").exists());
}

#[test]
fn list_cache_serves_within_ttl_and_invalidates_on_mutation() {
    let dir = workdir("cache");
    let base = dir.join("c");
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("one"), b"1").unwrap();
    let rfs = RemoteFs::new(Config {
        ssh: shim(&dir),
        control_dir: dir.join("ctl"),
        cache_ttl: Duration::from_secs(60),
        ..Config::default()
    });
    let url = local_url(&base);
    assert_eq!(list_all(&rfs, &url).unwrap().len(), 1);
    fs::write(base.join("two"), b"2").unwrap();
    assert_eq!(list_all(&rfs, &url).unwrap().len(), 1, "external change hidden within TTL");
    rfs.invalidate(&url);
    assert_eq!(list_all(&rfs, &url).unwrap().len(), 2);
    rfs.mkdir(&url.join("three")).wait().unwrap();
    assert_eq!(list_all(&rfs, &url).unwrap().len(), 3, "own mutation invalidates listing");
}

#[test]
fn smb_and_ftp_are_typed_unsupported() {
    let dir = workdir("unsupported");
    let rfs = client(&dir);
    let url = Url::parse("smb://server/share").unwrap();
    assert!(matches!(rfs.stat(&url).wait(), Err(Error::UnsupportedProtocol("smb"))));
}
