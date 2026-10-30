//! On-demand model download. Bytes go to `<name>.part`, are verified against the manifest
//! size and SHA-256, fsynced, and only then renamed to `<name>`. A manifest without a pinned URL
//! or checksum is refused before any network access.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use crate::sha256::Sha256;
use crate::speech::{ModelManifest, ModelStore, UNPINNED};
use crate::{AssistantError, CancelToken};

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

/// Models stored as `<dir>/<name>`. The directory is `$XDG_DATA_HOME/flamewm/assistant/models`
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

    fn final_path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn part_path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.part"))
    }
}

/// Rejects manifests that cannot be fetched safely. Runs before any file or network work.
pub fn check_pinned(manifest: &ModelManifest) -> Result<(), AssistantError> {
    let name_ok = !manifest.name.is_empty()
        && manifest.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !name_ok {
        return Err(AssistantError::Config(format!("invalid model name: {:?}", manifest.name)));
    }
    if manifest.url.trim().is_empty() || manifest.url == UNPINNED {
        return Err(AssistantError::NotConfigured(format!(
            "model {} has no pinned download URL; no model was fetched",
            manifest.name
        )));
    }
    if manifest.sha256.is_empty() || manifest.sha256 == UNPINNED {
        return Err(AssistantError::ChecksumUnavailable(format!(
            "model {} has no pinned SHA-256; refusing to download an unverified model",
            manifest.name
        )));
    }
    if manifest.sha256.len() != 64 || !manifest.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AssistantError::Config(format!("model {} SHA-256 is malformed", manifest.name)));
    }
    Ok(())
}

/// Checks size (when known) and SHA-256 of a file against the manifest.
pub fn verify(path: &Path, manifest: &ModelManifest) -> Result<(), AssistantError> {
    let len = fs::metadata(path)?.len();
    if manifest.size_bytes > 0 && len != manifest.size_bytes {
        return Err(AssistantError::ChecksumMismatch(format!(
            "model {} is {len} bytes, expected {}",
            manifest.name, manifest.size_bytes
        )));
    }
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
    let actual = hasher.hex();
    if actual != manifest.sha256.to_ascii_lowercase() {
        return Err(AssistantError::ChecksumMismatch(format!("model {} SHA-256 does not match", manifest.name)));
    }
    Ok(())
}

impl<F: Fetcher> ModelStore for DiskModelStore<F> {
    fn fetch(
        &mut self,
        manifest: &ModelManifest,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<PathBuf, AssistantError> {
        check_pinned(manifest)?;
        let final_path = self.final_path(&manifest.name);
        if final_path.is_file() {
            if verify(&final_path, manifest).is_ok() {
                return Ok(final_path);
            }
            fs::remove_file(&final_path)?;
        }
        fs::create_dir_all(&self.dir)?;
        let part = self.part_path(&manifest.name);
        if manifest.size_bytes > 0 && file_len(&part) > manifest.size_bytes {
            fs::remove_file(&part)?;
        }
        if let Err(error) = self.fetcher.fetch(
            &manifest.url,
            &part,
            manifest.size_bytes,
            progress,
            &self.cancel,
        ) {
            // A complete-looking partial is still verified below; anything else stays for resume.
            let complete = manifest.size_bytes > 0 && file_len(&part) >= manifest.size_bytes;
            if !complete {
                return Err(error);
            }
        }
        if let Err(error) = verify(&part, manifest) {
            let _ = fs::remove_file(&part);
            return Err(error);
        }
        File::open(&part)?.sync_all()?;
        fs::rename(&part, &final_path)?;
        Ok(final_path)
    }

    fn installed(&self, name: &str) -> Option<PathBuf> {
        let path = self.final_path(name);
        path.is_file().then_some(path)
    }
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
    use std::io::Write as _;
    use std::net::TcpListener;

    /// Deterministic bytes that exercise several SHA-256 blocks.
    fn body(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i.wrapping_mul(31).wrapping_add(i >> 7) % 253) as u8).collect()
    }

    fn sha_of(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hasher.hex()
    }

    fn manifest(name: &str, url: &str, data: &[u8]) -> ModelManifest {
        ModelManifest {
            name: name.into(),
            url: url.into(),
            sha256: sha_of(data),
            size_bytes: data.len() as u64,
        }
    }

    /// Loopback HTTP server that honours one `Range: bytes=N-` request per connection.
    /// Returns the base URL and the request heads it saw.
    fn serve_body(body: Vec<u8>, connections: usize) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}/model.bin", listener.local_addr().expect("addr"));
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
                let start = head
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_owned))
                    .and_then(|r| r.trim_end_matches('-').split('-').next().and_then(|n| n.parse::<usize>().ok()))
                    .unwrap_or(0);
                let slice = &body[start.min(body.len())..];
                let status = if start > 0 { "206 Partial Content" } else { "200 OK" };
                let range = if start > 0 {
                    format!("Content-Range: bytes {start}-{}/{}\r\n", body.len() - 1, body.len())
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
        (url, handle)
    }

    struct PanickingFetcher;
    impl Fetcher for PanickingFetcher {
        fn fetch(&mut self, _: &str, _: &Path, _: u64, _: &mut dyn FnMut(u64, u64), _: &CancelToken) -> Result<(), AssistantError> {
            panic!("network must not be touched");
        }
    }

    #[test]
    fn downloads_verifies_and_renames_from_loopback_server() {
        let data = body(300_000);
        let (url, server) = serve_body(data.clone(), 1);
        let dir = test_dir("download-basic");
        let m = manifest("stt-test", &url, &data);
        let mut store = DiskModelStore::new(dir.clone(), CurlFetcher::with_program_and_proto("curl", "=http"));
        let mut last = (0, 0);
        let path = store.fetch(&m, &mut |done, total| last = (done, total)).expect("fetch");
        server.join().expect("server");
        assert_eq!(fs::read(&path).expect("read"), data);
        assert_eq!(last, (data.len() as u64, data.len() as u64));
        assert!(!dir.join("stt-test.part").exists());
        assert_eq!(store.installed("stt-test"), Some(path));
    }

    #[test]
    fn resumes_partial_download_with_range_request() {
        let data = body(200_000);
        let (url, server) = serve_body(data.clone(), 1);
        let dir = test_dir("download-resume");
        fs::write(dir.join("stt-resume.part"), &data[..80_000]).expect("seed partial");
        let m = manifest("stt-resume", &url, &data);
        let mut store = DiskModelStore::new(dir.clone(), CurlFetcher::with_program_and_proto("curl", "=http"));
        let path = store.fetch(&m, &mut |_, _| {}).expect("resume fetch");
        let heads = server.join().expect("server");
        assert!(heads[0].to_ascii_lowercase().contains("range: bytes=80000-"), "{}", heads[0]);
        assert_eq!(fs::read(path).expect("read"), data);
    }

    #[test]
    fn unpinned_manifests_refuse_before_any_network_access() {
        let dir = test_dir("download-unpinned");
        let mut store = DiskModelStore::new(dir.clone(), PanickingFetcher);
        let no_url = ModelManifest { name: "tts".into(), url: UNPINNED.into(), sha256: UNPINNED.into(), size_bytes: 0 };
        assert!(matches!(store.fetch(&no_url, &mut |_, _| {}), Err(AssistantError::NotConfigured(_))));
        let no_sum = ModelManifest {
            name: "tts".into(),
            url: "https://example.invalid/m.bin".into(),
            sha256: UNPINNED.into(),
            size_bytes: 10,
        };
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
        let m = manifest("tts-bad", "https://example.invalid/m.bin", &good);
        let mut store = DiskModelStore::new(dir.clone(), WrongBytes);
        assert!(matches!(store.fetch(&m, &mut |_, _| {}), Err(AssistantError::ChecksumMismatch(_))));
        assert!(!dir.join("tts-bad.part").exists());
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
        let m = manifest("stt-cancel", "https://example.invalid/m.bin", &data);
        let mut store = DiskModelStore::new(dir.clone(), Stall);
        assert_eq!(store.fetch(&m, &mut |_, _| {}), Err(AssistantError::Cancelled));
        assert_eq!(fs::read(dir.join("stt-cancel.part")).expect("partial kept"), b"abc");
    }

    #[test]
    fn production_fetcher_refuses_plain_http() {
        let dir = test_dir("download-https-only");
        let data = body(8);
        let (url, _server) = serve_body(data.clone(), 0);
        let m = manifest("stt-plain", &url, &data);
        let mut store = DiskModelStore::new(dir.clone(), CurlFetcher::default());
        assert!(matches!(store.fetch(&m, &mut |_, _| {}), Err(AssistantError::Io(_))));
        assert_eq!(store.installed("stt-plain"), None);
    }

    #[test]
    fn missing_curl_is_reported_as_optional_dependency() {
        let dir = test_dir("download-no-curl");
        let data = body(8);
        let (url, _server) = serve_body(data.clone(), 0);
        let m = manifest("stt-nocurl", &url, &data);
        let mut store = DiskModelStore::new(dir, CurlFetcher::with_program_and_proto("flamewm-no-such-curl", "=http"));
        match store.fetch(&m, &mut |_, _| {}) {
            Err(AssistantError::NotConfigured(message)) => assert!(message.contains("curl")),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
