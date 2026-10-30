//! On-demand model download. A model is a directory `<models>/<name>/` holding every file of its
//! manifest. Each file's bytes go to `<file>.part`, are verified against the pinned size and
//! SHA-256, fsynced, and only then renamed to `<file>`. Files that already verify are skipped, so
//! an interrupted download resumes per file. A manifest without a pinned URL or checksum, or with
//! an unsafe file path, is refused before any network access.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use crate::sha256::Sha256;
use crate::speech::{ModelFile, ModelManifest, ModelStore, UNPINNED};
use crate::{AssistantError, CancelToken};

/// Written into a model directory after every file verified. Lists `size<TAB>path` per file.
const COMPLETE_MARKER: &str = ".complete";

/// Fetches `url` into `dest`, appending to an existing `dest` when the server allows resume.
pub trait Fetcher {
    fn fetch(
        &mut self,
        url: &str,
        dest: &Path,
        expected: u64,
        progress: &mut dyn FnMut(u64, u64),
        cancel: &CancelToken,
    ) -> Result<(), AssistantError>;
}

/// Downloads with a `curl` subprocess (optional dependency). Redirects and transfers are
/// limited to `proto`; production uses `=https`.
#[derive(Clone, Debug)]
pub struct CurlFetcher {
    pub program: String,
    pub proto: String,
}

impl Default for CurlFetcher {
    fn default() -> Self {
        Self { program: "curl".into(), proto: "=https".into() }
    }
}

impl CurlFetcher {
    pub fn with_program_and_proto(program: &str, proto: &str) -> Self {
        Self { program: program.into(), proto: proto.into() }
    }
}

impl Fetcher for CurlFetcher {
    fn fetch(
        &mut self,
        url: &str,
        dest: &Path,
        expected: u64,
        progress: &mut dyn FnMut(u64, u64),
        cancel: &CancelToken,
    ) -> Result<(), AssistantError> {
        let mut child = Command::new(&self.program)
            .args(["--fail", "--silent", "--show-error", "--location", "--proto", &self.proto])
            .args(["--proto-redir", &self.proto, "--tlsv1.2", "-C", "-", "-o"])
            .arg(dest)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => AssistantError::NotConfigured(
                    "curl is required to download models (optional dependency); install curl and retry".into(),
                ),
                _ => AssistantError::Io(format!("curl: {error}")),
            })?;
        let mut stderr = child.stderr.take();
        loop {
            if let Some(status) = child.try_wait()? {
                let mut message = String::new();
                if let Some(pipe) = stderr.as_mut() {
                    let _ = pipe.read_to_string(&mut message);
                }
                let done = file_len(dest);
                progress(done, expected);
                if status.success() {
                    return Ok(());
                }
                return Err(AssistantError::Io(format!(
                    "download failed ({status}): {}",
                    message.trim()
                )));
            }
            if cancel.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AssistantError::Cancelled);
            }
            progress(file_len(dest), expected);
            thread::sleep(Duration::from_millis(100));
        }
    }
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |meta| meta.len())
}

/// `<path>.part`, next to the final file.
fn part_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// Models stored as `<dir>/<name>/<file path>`. The directory is `$XDG_DATA_HOME/flamewm/assistant/models`
/// (see [`default_models_dir`]).
pub struct DiskModelStore<F: Fetcher> {
    dir: PathBuf,
    fetcher: F,
    cancel: CancelToken,
}

impl<F: Fetcher> DiskModelStore<F> {
    pub fn new(dir: PathBuf, fetcher: F) -> Self {
        Self { dir, fetcher, cancel: CancelToken::new() }
    }

    /// Ties downloads to a token so the UI can abort them. Partial files are kept for resume.
    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = cancel;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The directory that holds a model's files.
    pub fn model_dir(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Downloads and verifies one file. `base` is the verified byte count of earlier files, so
    /// progress reported for the whole model is monotonic.
    fn fetch_file(
        &mut self,
        model: &str,
        file: &ModelFile,
        final_path: &Path,
        base: u64,
        total: u64,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<(), AssistantError> {
        let part = part_of(final_path);
        if file_len(&part) > file.size_bytes {
            fs::remove_file(&part)?;
        }
        let mut relay = |done: u64, _: u64| progress(base + done, total);
        let fetched = self.fetcher.fetch(&file.url, &part, file.size_bytes, &mut relay, &self.cancel);
        if let Err(error) = fetched {
            // A complete-looking partial is still verified below; anything else stays for resume.
            if file_len(&part) < file.size_bytes {
                return Err(error);
            }
        }
        if let Err(error) = verify_file(&part, model, file) {
            let _ = fs::remove_file(&part);
            return Err(error);
        }
        File::open(&part)?.sync_all()?;
        fs::rename(&part, final_path)?;
        Ok(())
    }
}

/// Rejects manifests that cannot be fetched safely. Runs before any file or network work.
pub fn check_pinned(manifest: &ModelManifest) -> Result<(), AssistantError> {
    let name_ok = !manifest.name.is_empty()
        && manifest.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !name_ok {
        return Err(AssistantError::Config(format!("invalid model name: {:?}", manifest.name)));
    }
    if manifest.files.is_empty() {
        return Err(AssistantError::Config(format!("model {} lists no files", manifest.name)));
    }
    let mut seen = HashSet::new();
    for file in &manifest.files {
        if !is_safe_relative(&file.path) {
            return Err(AssistantError::Config(format!(
                "model {} has an unsafe file path: {:?}",
                manifest.name, file.path
            )));
        }
        if !seen.insert(file.path.as_str()) {
            return Err(AssistantError::Config(format!("model {} lists {} twice", manifest.name, file.path)));
        }
        if file.url.trim().is_empty() || file.url == UNPINNED {
            return Err(AssistantError::NotConfigured(format!(
                "model {} file {} has no pinned download URL; no model was fetched",
                manifest.name, file.path
            )));
        }
        if file.sha256.is_empty() || file.sha256 == UNPINNED {
            return Err(AssistantError::ChecksumUnavailable(format!(
                "model {} file {} has no pinned SHA-256; refusing to download an unverified model",
                manifest.name, file.path
            )));
        }
        if file.sha256.len() != 64 || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(AssistantError::Config(format!(
                "model {} file {} SHA-256 is malformed",
                manifest.name, file.path
            )));
        }
        if file.size_bytes == 0 {
            return Err(AssistantError::Config(format!(
                "model {} file {} has no pinned size",
                manifest.name, file.path
            )));
        }
    }
    Ok(())
}

/// True for a relative path of plain components: no `..`, `.`, root, prefix, or backslash.
fn is_safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains('\0')
        && Path::new(path).components().all(|c| matches!(c, Component::Normal(_)))
}

fn sha256_of(path: &Path) -> Result<String, AssistantError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hasher.hex())
}

/// Checks size (when pinned) and SHA-256 of one file against its pinned values.
pub fn verify_file(path: &Path, model: &str, file: &ModelFile) -> Result<(), AssistantError> {
    let len = fs::metadata(path)?.len();
    if file.size_bytes > 0 && len != file.size_bytes {
        return Err(AssistantError::ChecksumMismatch(format!(
            "model {model} file {} is {len} bytes, expected {}",
            file.path, file.size_bytes
        )));
    }
    if sha256_of(path)? != file.sha256.to_ascii_lowercase() {
        return Err(AssistantError::ChecksumMismatch(format!(
            "model {model} file {} SHA-256 does not match",
            file.path
        )));
    }
    Ok(())
}

/// Single-file form of [`verify_file`]: the manifest must list exactly one file.
pub fn verify(path: &Path, manifest: &ModelManifest) -> Result<(), AssistantError> {
    match manifest.files.as_slice() {
        [file] => verify_file(path, &manifest.name, file),
        _ => Err(AssistantError::Config(format!(
            "model {} has {} files; verify checks a single-file manifest",
            manifest.name,
            manifest.files.len()
        ))),
    }
}

impl<F: Fetcher> ModelStore for DiskModelStore<F> {
    fn fetch(
        &mut self,
        manifest: &ModelManifest,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<PathBuf, AssistantError> {
        check_pinned(manifest)?;
        let model_dir = self.model_dir(&manifest.name);
        fs::create_dir_all(&model_dir)?;
        // The marker describes the last complete fetch; it is rewritten only once all files verify.
        let _ = fs::remove_file(model_dir.join(COMPLETE_MARKER));
        let total = manifest.total_bytes();
        let mut done = 0u64;
        for file in &manifest.files {
            let final_path = model_dir.join(&file.path);
            if final_path.is_file() {
                if verify_file(&final_path, &manifest.name, file).is_ok() {
                    done += file.size_bytes;
                    progress(done, total);
                    continue;
                }
                fs::remove_file(&final_path)?;
            }
            if let Some(parent) = final_path.parent() {
                fs::create_dir_all(parent)?;
            }
            self.fetch_file(&manifest.name, file, &final_path, done, total, progress)?;
            done += file.size_bytes;
        }
        write_marker(&model_dir, manifest)?;
        Ok(model_dir)
    }

    fn installed(&self, name: &str) -> Option<PathBuf> {
        let model_dir = self.model_dir(name);
        let marker = fs::read_to_string(model_dir.join(COMPLETE_MARKER)).ok()?;
        let complete = marker.lines().all(|line| {
            let Some((size, path)) = line.split_once('\t') else { return false };
            let Ok(size) = size.parse::<u64>() else { return false };
            fs::metadata(model_dir.join(path)).is_ok_and(|meta| meta.is_file() && meta.len() == size)
        });
        (complete && !marker.is_empty()).then_some(model_dir)
    }
}

fn write_marker(model_dir: &Path, manifest: &ModelManifest) -> Result<(), AssistantError> {
    let body: String = manifest.files.iter().map(|f| format!("{}\t{}\n", f.size_bytes, f.path)).collect();
    let tmp = model_dir.join(format!("{COMPLETE_MARKER}.part"));
    fs::write(&tmp, body)?;
    File::open(&tmp)?.sync_all()?;
    fs::rename(&tmp, model_dir.join(COMPLETE_MARKER))?;
    Ok(())
}

/// Unpacks a zstd-compressed tar archive into `dest` with `tar --zstd`. Call only on an archive
/// that passed [`verify_file`]. Needs GNU tar and the `zstd` program; see docs/INSTALL.md.
pub fn extract_zstd_tar(archive: &Path, dest: &Path) -> Result<(), AssistantError> {
    extract_zstd_tar_with("tar", archive, dest)
}

/// [`extract_zstd_tar`] with the tar program given explicitly, for tests and alternate installs.
pub fn extract_zstd_tar_with(program: &str, archive: &Path, dest: &Path) -> Result<(), AssistantError> {
    fs::create_dir_all(dest)?;
    let output = Command::new(program)
        .args(["--zstd", "--no-same-owner", "-xf"])
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => AssistantError::NotConfigured(format!(
                "{program} is required to unpack speech model archives (optional dependency); install tar and zstd"
            )),
            _ => AssistantError::Io(format!("{program}: {error}")),
        })?;
    if output.status.success() {
        return Ok(());
    }
    Err(AssistantError::Io(format!(
        "{program} --zstd failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// `$XDG_DATA_HOME/flamewm/assistant/models`, falling back to `~/.local/share`.
pub fn default_models_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::home_dir().map(|home| home.join(".local/share")))?;
    Some(base.join("flamewm").join("assistant").join("models"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_dir;
    use std::collections::HashMap;
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    /// Deterministic bytes that exercise several SHA-256 blocks. `seed` varies the content.
    fn body_seeded(len: usize, seed: usize) -> Vec<u8> {
        (0..len).map(|i| (i.wrapping_mul(31).wrapping_add(i >> 7).wrapping_add(seed) % 253) as u8).collect()
    }

    fn body(len: usize) -> Vec<u8> {
        body_seeded(len, 0)
    }

    fn sha_of(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hasher.hex()
    }

    fn file(path: &str, url: &str, data: &[u8]) -> ModelFile {
        ModelFile { path: path.into(), url: url.into(), sha256: sha_of(data), size_bytes: data.len() as u64 }
    }

    /// Loopback HTTP server. Serves `files` by request path, honours one `Range: bytes=N-` per
    /// connection, answers 404 for unknown paths, and returns the base URL plus the request heads.
    fn serve_files(files: Vec<(String, Vec<u8>)>, connections: usize) -> (String, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let handle = thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..connections {
                let (mut conn, _) = listener.accept().expect("accept");
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = conn.read(&mut tmp).expect("read");
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let head = String::from_utf8_lossy(&buf).into_owned();
                let request_path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let Some((_, data)) = files.iter().find(|(path, _)| *path == request_path) else {
                    let _ = conn.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    seen.push(head);
                    continue;
                };
                let start = head
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_owned))
                    .and_then(|r| r.trim_end_matches('-').split('-').next().and_then(|n| n.parse::<usize>().ok()))
                    .unwrap_or(0);
                let slice = &data[start.min(data.len())..];
                let status = if start > 0 { "206 Partial Content" } else { "200 OK" };
                let range = if start > 0 {
                    format!("Content-Range: bytes {start}-{}/{}\r\n", data.len() - 1, data.len())
                } else {
                    String::new()
                };
                let header = format!(
                    "HTTP/1.1 {status}\r\n{range}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    slice.len()
                );
                let _ = conn.write_all(header.as_bytes());
                let _ = conn.write_all(slice);
                seen.push(head);
            }
            seen
        });
        (base, handle)
    }

    /// Single-file server at `/model.bin`. Returns that URL.
    fn serve_body(body: Vec<u8>, connections: usize) -> (String, JoinHandle<Vec<String>>) {
        let (base, handle) = serve_files(vec![("/model.bin".into(), body)], connections);
        (format!("{base}/model.bin"), handle)
    }

    struct PanickingFetcher;
    impl Fetcher for PanickingFetcher {
        fn fetch(&mut self, _: &str, _: &Path, _: u64, _: &mut dyn FnMut(u64, u64), _: &CancelToken) -> Result<(), AssistantError> {
            panic!("network must not be touched");
        }
    }

    /// Serves bytes from memory keyed by URL. `tamper` names one URL that gets wrong bytes.
    struct MapFetcher {
        files: HashMap<String, Vec<u8>>,
        tamper: Option<String>,
        requested: Vec<String>,
    }
    impl Fetcher for MapFetcher {
        fn fetch(
            &mut self,
            url: &str,
            dest: &Path,
            _: u64,
            progress: &mut dyn FnMut(u64, u64),
            _: &CancelToken,
        ) -> Result<(), AssistantError> {
            self.requested.push(url.to_owned());
            let data = if self.tamper.as_deref() == Some(url) { b"tampered".to_vec() } else { self.files[url].clone() };
            fs::write(dest, &data)?;
            progress(data.len() as u64, data.len() as u64);
            Ok(())
        }
    }

    #[test]
    fn downloads_verifies_and_renames_from_loopback_server() {
        let data = body(300_000);
        let (url, server) = serve_body(data.clone(), 1);
        let dir = test_dir("download-basic");
        let m = ModelManifest::single("stt-test", "model.bin", &url, &sha_of(&data), data.len() as u64);
        let mut store = DiskModelStore::new(dir.clone(), CurlFetcher::with_program_and_proto("curl", "=http"));
        let mut last = (0, 0);
        let path = store.fetch(&m, &mut |done, total| last = (done, total)).expect("fetch");
        server.join().expect("server");
        assert_eq!(path, dir.join("stt-test"));
        assert_eq!(fs::read(path.join("model.bin")).expect("read"), data);
        assert_eq!(last, (data.len() as u64, data.len() as u64));
        assert!(!path.join("model.bin.part").exists());
        assert_eq!(store.installed("stt-test"), Some(path));
    }

    #[test]
    fn multi_file_download_fetches_each_file_and_aggregates_progress() {
        let files = [("/model.bin", body(300_000)), ("/config.json", body_seeded(4_634, 7)), ("/sub/tok.json", body_seeded(9_000, 9))];
        let served: Vec<(String, Vec<u8>)> = files.iter().map(|(p, d)| ((*p).to_owned(), d.clone())).collect();
        let (base, server) = serve_files(served, files.len());
        let dir = test_dir("download-multi");
        let manifest = ModelManifest {
            name: "tts-multi".into(),
            files: files
                .iter()
                .map(|(p, d)| file(p.trim_start_matches('/'), &format!("{base}{p}"), d))
                .collect(),
        };
        let total = manifest.total_bytes();
        let mut store = DiskModelStore::new(dir.clone(), CurlFetcher::with_program_and_proto("curl", "=http"));
        let mut reports = Vec::new();
        let model_dir = store.fetch(&manifest, &mut |done, t| reports.push((done, t))).expect("fetch");
        let heads = server.join().expect("server");
        assert_eq!(heads.len(), 3);
        assert_eq!(model_dir, dir.join("tts-multi"));
        for (path, data) in &files {
            assert_eq!(fs::read(model_dir.join(path.trim_start_matches('/'))).expect("read"), *data);
            assert!(!model_dir.join(format!("{}.part", path.trim_start_matches('/'))).exists());
        }
        assert!(reports.windows(2).all(|w| w[0].0 <= w[1].0), "progress is monotonic: {reports:?}");
        assert!(reports.iter().all(|&(done, t)| t == total && done <= total));
        assert_eq!(reports.last(), Some(&(total, total)));
        assert_eq!(store.installed("tts-multi"), Some(model_dir.clone()));
        // Removing any listed file makes the model not installed.
        fs::remove_file(model_dir.join("sub/tok.json")).expect("remove");
        assert_eq!(store.installed("tts-multi"), None);
    }

    #[test]
    fn partial_resume_skips_verified_files_and_resumes_the_partial_one() {
        let first = body_seeded(5_000, 1);
        let second = body(200_000);
        // Only the second file is served: a request for the first would be a 404 and fail the test.
        let (base, server) = serve_files(vec![("/tok.json".into(), second.clone())], 1);
        let dir = test_dir("download-multi-resume");
        let model_dir = dir.join("tts-resume");
        fs::create_dir_all(&model_dir).expect("model dir");
        fs::write(model_dir.join("config.json"), &first).expect("seed verified file");
        fs::write(model_dir.join("tok.json.part"), &second[..80_000]).expect("seed partial");
        let manifest = ModelManifest {
            name: "tts-resume".into(),
            files: vec![
                file("config.json", "https://example.invalid/config.json", &first),
                file("tok.json", &format!("{base}/tok.json"), &second),
            ],
        };
        let mut store = DiskModelStore::new(dir.clone(), CurlFetcher::with_program_and_proto("curl", "=http"));
        let mut first_report = None;
        store.fetch(&manifest, &mut |done, _| {
            first_report.get_or_insert(done);
        })
        .expect("resume fetch");
        let heads = server.join().expect("server");
        assert_eq!(heads.len(), 1);
        assert!(heads[0].to_ascii_lowercase().contains("range: bytes=80000-"), "{}", heads[0]);
        assert_eq!(fs::read(model_dir.join("tok.json")).expect("read"), second);
        assert_eq!(first_report, Some(first.len() as u64), "verified file is counted before the download starts");
        assert_eq!(store.installed("tts-resume"), Some(model_dir));
    }

    #[test]
    fn checksum_mismatch_on_one_file_installs_nothing_and_keeps_the_verified_file() {
        let good_a = body_seeded(64, 1);
        let good_b = body_seeded(128, 2);
        let base = "https://example.invalid";
        let manifest = ModelManifest {
            name: "tts-bad".into(),
            files: vec![
                file("a.bin", &format!("{base}/a.bin"), &good_a),
                file("b.bin", &format!("{base}/b.bin"), &good_b),
            ],
        };
        let mut fetcher = MapFetcher {
            files: HashMap::from([
                (format!("{base}/a.bin"), good_a.clone()),
                (format!("{base}/b.bin"), good_b.clone()),
            ]),
            tamper: Some(format!("{base}/b.bin")),
            requested: Vec::new(),
        };
        let dir = test_dir("download-mismatch-multi");
        let mut store = DiskModelStore::new(dir.clone(), MapFetcherRef(&mut fetcher));
        let result = store.fetch(&manifest, &mut |_, _| {});
        assert!(matches!(result, Err(AssistantError::ChecksumMismatch(message)) if message.contains("b.bin")));
        let model_dir = dir.join("tts-bad");
        assert!(!model_dir.join("b.bin.part").exists());
        assert!(!model_dir.join("b.bin").exists());
        assert_eq!(fs::read(model_dir.join("a.bin")).expect("verified file kept"), good_a);
        assert_eq!(store.installed("tts-bad"), None);
        drop(store);
        assert_eq!(fetcher.requested.len(), 2);
    }

    /// Lets a test keep the fake's request log after the store has consumed it.
    struct MapFetcherRef<'a>(&'a mut MapFetcher);
    impl Fetcher for MapFetcherRef<'_> {
        fn fetch(&mut self, url: &str, dest: &Path, expected: u64, progress: &mut dyn FnMut(u64, u64), cancel: &CancelToken) -> Result<(), AssistantError> {
            self.0.fetch(url, dest, expected, progress, cancel)
        }
    }

    #[test]
    fn unsafe_file_paths_are_refused_before_any_network_access() {
        let dir = test_dir("download-unsafe-path");
        for path in ["../evil.bin", "/abs.bin", "a/../../b.bin", "./c.bin", "d\\e.bin", ""] {
            let manifest = ModelManifest {
                name: "tts-unsafe".into(),
                files: vec![ModelFile {
                    path: path.into(),
                    url: "https://example.invalid/x".into(),
                    sha256: "a".repeat(64),
                    size_bytes: 10,
                }],
            };
            let mut store = DiskModelStore::new(dir.clone(), PanickingFetcher);
            assert!(matches!(store.fetch(&manifest, &mut |_, _| {}), Err(AssistantError::Config(_))), "{path:?}");
        }
        assert!(!dir.join("tts-unsafe").exists());
    }

    #[test]
    fn unpinned_manifests_refuse_before_any_network_access() {
        let dir = test_dir("download-unpinned");
        let mut store = DiskModelStore::new(dir.clone(), PanickingFetcher);
        let no_url = ModelManifest::single("tts", "m.bin", UNPINNED, UNPINNED, 10);
        assert!(matches!(store.fetch(&no_url, &mut |_, _| {}), Err(AssistantError::NotConfigured(_))));
        let no_sum = ModelManifest::single("tts", "m.bin", "https://example.invalid/m.bin", UNPINNED, 10);
        assert!(matches!(store.fetch(&no_sum, &mut |_, _| {}), Err(AssistantError::ChecksumUnavailable(_))));
        assert!(!dir.exists() || fs::read_dir(&dir).expect("dir").next().is_none());
    }

    #[test]
    fn checksum_mismatch_removes_partial_and_installs_nothing() {
        struct WrongBytes;
        impl Fetcher for WrongBytes {
            fn fetch(&mut self, _: &str, dest: &Path, _: u64, _: &mut dyn FnMut(u64, u64), _: &CancelToken) -> Result<(), AssistantError> {
                fs::write(dest, b"tampered")?;
                Ok(())
            }
        }
        let dir = test_dir("download-mismatch");
        let good = body(64);
        let m = ModelManifest::single("tts-bad", "model.bin", "https://example.invalid/m.bin", &sha_of(&good), 64);
        let mut store = DiskModelStore::new(dir.clone(), WrongBytes);
        assert!(matches!(store.fetch(&m, &mut |_, _| {}), Err(AssistantError::ChecksumMismatch(_))));
        assert!(!dir.join("tts-bad/model.bin.part").exists());
        assert_eq!(store.installed("tts-bad"), None);
    }

    #[test]
    fn cancelled_download_keeps_partial_for_resume() {
        struct Stall;
        impl Fetcher for Stall {
            fn fetch(&mut self, _: &str, dest: &Path, _: u64, _: &mut dyn FnMut(u64, u64), cancel: &CancelToken) -> Result<(), AssistantError> {
                fs::write(dest, b"abc")?;
                cancel.cancel();
                Err(AssistantError::Cancelled)
            }
        }
        let dir = test_dir("download-cancel");
        let data = body(10);
        let m = ModelManifest::single("stt-cancel", "model.bin", "https://example.invalid/m.bin", &sha_of(&data), 10);
        let mut store = DiskModelStore::new(dir.clone(), Stall);
        assert_eq!(store.fetch(&m, &mut |_, _| {}), Err(AssistantError::Cancelled));
        assert_eq!(fs::read(dir.join("stt-cancel/model.bin.part")).expect("partial kept"), b"abc");
    }

    #[test]
    fn production_fetcher_refuses_plain_http() {
        let dir = test_dir("download-https-only");
        let data = body(8);
        let (url, _server) = serve_body(data.clone(), 0);
        let m = ModelManifest::single("stt-plain", "model.bin", &url, &sha_of(&data), 8);
        let mut store = DiskModelStore::new(dir.clone(), CurlFetcher::default());
        assert!(matches!(store.fetch(&m, &mut |_, _| {}), Err(AssistantError::Io(_))));
        assert_eq!(store.installed("stt-plain"), None);
    }

    #[test]
    fn missing_curl_is_reported_as_optional_dependency() {
        let dir = test_dir("download-no-curl");
        let data = body(8);
        let (url, _server) = serve_body(data.clone(), 0);
        let m = ModelManifest::single("stt-nocurl", "model.bin", &url, &sha_of(&data), 8);
        let mut store = DiskModelStore::new(dir, CurlFetcher::with_program_and_proto("flamewm-no-such-curl", "=http"));
        match store.fetch(&m, &mut |_, _| {}) {
            Err(AssistantError::NotConfigured(message)) => assert!(message.contains("curl")),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn missing_tar_is_reported_as_optional_dependency() {
        let dir = test_dir("extract-no-tar");
        let archive = dir.join("a.tar.zst");
        fs::write(&archive, b"not used").expect("archive");
        match extract_zstd_tar_with("flamewm-no-such-tar", &archive, &dir.join("out")) {
            Err(AssistantError::NotConfigured(message)) => assert!(message.contains("tar")),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn zstd_tar_round_trips_when_tar_is_available() {
        let dir = test_dir("extract-roundtrip");
        let src = dir.join("src");
        fs::create_dir_all(src.join("inner")).expect("src");
        fs::write(src.join("inner/model.fermion"), body(4_000)).expect("payload");
        let archive = dir.join("pack.tar.zst");
        let created = Command::new("tar")
            .args(["--zstd", "-cf"])
            .arg(&archive)
            .arg("-C")
            .arg(&src)
            .arg(".")
            .output();
        let Ok(created) = created else {
            eprintln!("skipping: tar not installed");
            return;
        };
        if !created.status.success() {
            eprintln!("skipping: tar --zstd unsupported: {}", String::from_utf8_lossy(&created.stderr));
            return;
        }
        let out = dir.join("out");
        extract_zstd_tar(&archive, &out).expect("extract");
        assert_eq!(fs::read(out.join("inner/model.fermion")).expect("read"), body(4_000));
    }

    #[test]
    fn corrupt_archive_reports_tar_failure_without_panicking() {
        let dir = test_dir("extract-corrupt");
        let archive = dir.join("bad.tar.zst");
        let mut file = File::create(&archive).expect("archive");
        file.write_all(b"definitely not zstd").expect("write");
        drop(file);
        match extract_zstd_tar(&archive, &dir.join("out")) {
            Err(AssistantError::Io(message)) => assert!(message.contains("--zstd failed"), "{message}"),
            Err(AssistantError::NotConfigured(_)) => eprintln!("skipping: tar not installed"),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
