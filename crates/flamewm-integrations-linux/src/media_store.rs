//! Managed wallpaper media store: selected images are copied into FlameWM's own folder so the
//! desktop and lock wallpapers keep working after the original file is removed.

use std::collections::HashSet;
use std::env;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use flamewm_image_core::{ImageSourceFormat, decode_bytes};

use crate::icons::user_data_home;

/// Largest source file accepted for import (64 MiB).
pub const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const STORE_SUBDIR: &str = "flamewm/wallpapers";
const SHORT_HASH_LEN: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    DesktopWallpaper,
    LockWallpaper,
    /// Lock-screen profile picture.
    Avatar,
    /// Custom Start menu icon (raster formats only; SVG is rejected).
    StartIcon,
}

impl MediaKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::DesktopWallpaper => "desktop",
            Self::LockWallpaper => "lock",
            Self::Avatar => "avatar",
            Self::StartIcon => "start",
        }
    }

    /// Start icons are PNG/WebP/JPEG only; wallpapers and avatars also accept GIF.
    fn accepts(self, format: ImageSourceFormat) -> bool {
        !matches!(self, Self::StartIcon) || !matches!(format, ImageSourceFormat::Gif)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedMedia {
    pub path: PathBuf,
    pub sha256_hex: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug)]
pub enum MediaError {
    Io(io::Error),
    NotRegularFile,
    TooLarge,
    UnsupportedFormat,
    UnsupportedStartIconFormat,
    Decode(String),
    NameCollision(PathBuf),
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "media I/O failed: {error}"),
            Self::NotRegularFile => f.write_str("selected media is not a regular file"),
            Self::TooLarge => write!(f, "media exceeds {MAX_SOURCE_BYTES} bytes"),
            Self::UnsupportedFormat => {
                f.write_str("unsupported image format (PNG, WebP, JPEG or GIF)")
            }
            Self::UnsupportedStartIconFormat => {
                f.write_str("unsupported start icon format (PNG, WebP or JPEG)")
            }
            Self::Decode(message) => write!(f, "image decode failed: {message}"),
            Self::NameCollision(path) => {
                write!(
                    f,
                    "managed media name already holds other content: {}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for MediaError {}

impl From<io::Error> for MediaError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone, Debug)]
pub struct MediaStore {
    root: PathBuf,
}

impl MediaStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// `$XDG_DATA_HOME/flamewm/wallpapers`, falling back to `$HOME/.local/share/...`.
    pub fn default_root() -> Option<PathBuf> {
        user_data_home(
            env::var_os("HOME").as_deref(),
            env::var_os("XDG_DATA_HOME").as_deref(),
        )
        .map(|data| data.join(STORE_SUBDIR))
    }

    /// Copies `src` into the store and returns the managed copy.
    pub fn import(&self, src: &Path, kind: MediaKind) -> Result<ImportedMedia, MediaError> {
        let file = open_source(src)?;
        let bytes = read_source(file)?;
        self.import_bytes(&bytes, kind)
    }

    fn import_bytes(&self, bytes: &[u8], kind: MediaKind) -> Result<ImportedMedia, MediaError> {
        let (format, extension) = sniff(bytes)
            .filter(|(format, _)| kind.accepts(*format))
            .ok_or(match kind {
                MediaKind::StartIcon => MediaError::UnsupportedStartIconFormat,
                _ => MediaError::UnsupportedFormat,
            })?;
        let image = decode_bytes(bytes, format).map_err(MediaError::Decode)?;
        let (width, height) = (image.width, image.height);
        let sha256_hex = sha256_hex(bytes);
        let name = format!(
            "{}-{}.{extension}",
            kind.prefix(),
            &sha256_hex[..SHORT_HASH_LEN]
        );
        let path = self.root.join(name);
        self.persist(&path, bytes, |file, data| file.write_all(data))?;
        Ok(ImportedMedia {
            path,
            sha256_hex,
            width,
            height,
        })
    }

    /// Writes `bytes` to `dest` through a same-directory temp file. `write` is the
    /// content writer, injectable so tests can simulate a failure mid-write.
    fn persist<W>(&self, dest: &Path, bytes: &[u8], write: W) -> Result<(), MediaError>
    where
        W: FnOnce(&mut File, &[u8]) -> io::Result<()>,
    {
        self.ensure_root()?;
        match fs::symlink_metadata(dest) {
            Ok(meta) => {
                return if meta.is_file() && existing_file_matches(dest, bytes)? {
                    Ok(())
                } else {
                    Err(MediaError::NameCollision(dest.to_path_buf()))
                };
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let leaf = dest.file_name().and_then(OsStr::to_str).unwrap_or("media");
        let temp = self
            .root
            .join(format!(".{leaf}.{}.tmp", std::process::id()));
        let _ = fs::remove_file(&temp);
        let staged = (|| -> io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)?;
            write(&mut file, bytes)?;
            file.sync_all()
        })();
        if let Err(error) = staged {
            let _ = fs::remove_file(&temp);
            return Err(error.into());
        }
        // hard_link never replaces an existing name, so a concurrent different file is kept.
        let published = fs::hard_link(&temp, dest);
        let _ = fs::remove_file(&temp);
        match published {
            Ok(()) => {
                let _ = File::open(&self.root).and_then(|dir| dir.sync_all());
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if existing_file_matches(dest, bytes)? {
                    Ok(())
                } else {
                    Err(MediaError::NameCollision(dest.to_path_buf()))
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    fn ensure_root(&self) -> io::Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.root)?;
        fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))
    }

    /// True when `p` is a direct child of the store root with a managed file name.
    pub fn is_managed(&self, p: &Path) -> bool {
        p.parent() == Some(self.root.as_path())
            && p.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(is_managed_name)
    }

    /// Removes managed regular files that are not in `keep`. Symlinks and other files are untouched.
    pub fn gc(&self, keep: &[PathBuf]) -> io::Result<()> {
        let kept: HashSet<&OsStr> = keep
            .iter()
            .filter(|path| path.parent() == Some(self.root.as_path()))
            .filter_map(|path| path.file_name())
            .collect();
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !self.is_managed(&path) || kept.contains(entry.file_name().as_os_str()) {
                continue;
            }
            if entry.file_type()?.is_file() {
                fs::remove_file(&path)?;
            }
        }
        Ok(())
    }
}

/// Compares an existing regular file without buffering its full contents.
fn existing_file_matches(path: &Path, expected: &[u8]) -> io::Result<bool> {
    const CHUNK_BYTES: usize = 16 * 1024;

    let path_meta = fs::symlink_metadata(path)?;
    if !path_meta.is_file() || path_meta.len() != expected.len() as u64 {
        return Ok(false);
    }
    let mut file = File::open(path)?;
    let opened_meta = file.metadata()?;
    let current_meta = fs::symlink_metadata(path)?;
    if !opened_meta.is_file()
        || !current_meta.is_file()
        || opened_meta.dev() != path_meta.dev()
        || opened_meta.ino() != path_meta.ino()
        || current_meta.dev() != opened_meta.dev()
        || current_meta.ino() != opened_meta.ino()
        || opened_meta.len() != expected.len() as u64
    {
        return Ok(false);
    }

    let mut buffer = [0; CHUNK_BYTES];
    for chunk in expected.chunks(CHUNK_BYTES) {
        file.read_exact(&mut buffer[..chunk.len()])?;
        if buffer[..chunk.len()] != *chunk {
            return Ok(false);
        }
    }
    let mut extra = [0];
    Ok(file.read(&mut extra)? == 0)
}

/// Imports a user-selected image and returns the managed absolute path to store in settings.
pub fn import_for_setting(kind: MediaKind, src: &Path) -> Result<String, String> {
    let root = MediaStore::default_root().ok_or("HOME and XDG_DATA_HOME are unset")?;
    let imported = MediaStore::new(root)
        .import(src, kind)
        .map_err(|error| error.to_string())?;
    imported
        .path
        .into_os_string()
        .into_string()
        .map_err(|_| "managed media path is not UTF-8".to_string())
}

/// Opens the canonical target and verifies the handle is the same regular file as the path.
fn open_source(src: &Path) -> Result<File, MediaError> {
    let canonical = fs::canonicalize(src)?;
    let file = File::open(&canonical)?;
    let opened = file.metadata()?;
    let linked = fs::symlink_metadata(&canonical)?;
    if !opened.is_file()
        || !linked.is_file()
        || opened.dev() != linked.dev()
        || opened.ino() != linked.ino()
    {
        return Err(MediaError::NotRegularFile);
    }
    if opened.len() > MAX_SOURCE_BYTES {
        return Err(MediaError::TooLarge);
    }
    Ok(file)
}

/// Reads at most `MAX_SOURCE_BYTES` from `reader`.
pub fn read_source<R: Read>(reader: R) -> Result<Vec<u8>, MediaError> {
    let mut bytes = Vec::new();
    reader.take(MAX_SOURCE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(MediaError::TooLarge);
    }
    Ok(bytes)
}

fn sniff(bytes: &[u8]) -> Option<(ImageSourceFormat, &'static str)> {
    match ImageSourceFormat::from_magic(bytes)? {
        format @ ImageSourceFormat::Png => Some((format, "png")),
        format @ ImageSourceFormat::Webp => Some((format, "webp")),
        format @ ImageSourceFormat::Jpeg => Some((format, "jpg")),
        format @ ImageSourceFormat::Gif => Some((format, "gif")),
        ImageSourceFormat::Ppm | ImageSourceFormat::Svg => None,
    }
}

fn is_managed_name(name: &str) -> bool {
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    let Some((prefix, hash)) = stem.split_once('-') else {
        return false;
    };
    matches!(prefix, "desktop" | "lock" | "avatar" | "start")
        && matches!(extension, "png" | "webp" | "jpg" | "gif")
        && hash.len() == SHORT_HASH_LEN
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Lowercase hex SHA-256 digest (FIPS 180-4); the workspace has no hashing crate.
fn sha256_hex(data: &[u8]) -> String {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut blocks = data.chunks_exact(64);
    for block in &mut blocks {
        sha256_compress(&mut state, block);
    }
    let rest = blocks.remainder();
    let mut tail = [0u8; 128];
    tail[..rest.len()].copy_from_slice(rest);
    tail[rest.len()] = 0x80;
    let tail_len = if rest.len() < 56 { 64 } else { 128 };
    let bit_len = (data.len() as u64).wrapping_mul(8);
    tail[tail_len - 8..tail_len].copy_from_slice(&bit_len.to_be_bytes());
    for block in tail[..tail_len].chunks_exact(64) {
        sha256_compress(&mut state, block);
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}

fn sha256_compress(state: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for (index, word) in block.chunks_exact(4).enumerate() {
        w[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
    }
    for index in 16..64 {
        let s0 =
            w[index - 15].rotate_right(7) ^ w[index - 15].rotate_right(18) ^ (w[index - 15] >> 3);
        let s1 =
            w[index - 2].rotate_right(17) ^ w[index - 2].rotate_right(19) ^ (w[index - 2] >> 10);
        w[index] = w[index - 16]
            .wrapping_add(s0)
            .wrapping_add(w[index - 7])
            .wrapping_add(s1);
    }
    let mut v = *state;
    for index in 0..64 {
        let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
        let choose = (v[4] & v[5]) ^ (!v[4] & v[6]);
        let t1 = v[7]
            .wrapping_add(s1)
            .wrapping_add(choose)
            .wrapping_add(SHA256_K[index])
            .wrapping_add(w[index]);
        let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
        let majority = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
        let t2 = s0.wrapping_add(majority);
        v = [
            t1.wrapping_add(t2),
            v[0],
            v[1],
            v[2],
            v[3].wrapping_add(t1),
            v[4],
            v[5],
            v[6],
        ];
    }
    for (slot, value) in state.iter_mut().zip(v) {
        *slot = slot.wrapping_add(value);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "flamewm-media-test-{}-{unique}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    0xedb8_8320 ^ (crc >> 1)
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut framed = kind.to_vec();
        framed.extend_from_slice(data);
        out.extend_from_slice(&crc32(&framed).to_be_bytes());
    }

    /// Minimal valid RGBA8 PNG using zlib stored blocks.
    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut raw = Vec::new();
        for row in 0..height {
            raw.push(0);
            for column in 0..width {
                raw.extend_from_slice(&[column as u8, row as u8, 0x40, 0xff]);
            }
        }
        let mut zlib = vec![0x78, 0x01];
        let parts: Vec<&[u8]> = raw.chunks(65535).collect();
        for (index, part) in parts.iter().enumerate() {
            zlib.push(u8::from(index + 1 == parts.len()));
            let len = part.len() as u16;
            zlib.extend_from_slice(&len.to_le_bytes());
            zlib.extend_from_slice(&(!len).to_le_bytes());
            zlib.extend_from_slice(part);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in &raw {
            a = (a + u32::from(byte)) % 65521;
            b = (b + a) % 65521;
        }
        zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);

        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        push_chunk(&mut png, b"IHDR", &ihdr);
        push_chunk(&mut png, b"IDAT", &zlib);
        push_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).expect("write fixture");
    }

    fn store_in(dir: &TempDir) -> MediaStore {
        MediaStore::new(dir.0.join("store"))
    }

    fn name_of(path: &Path) -> &str {
        path.file_name().unwrap().to_str().unwrap()
    }

    fn import_desktop(dir: &TempDir, src: &Path) -> Result<ImportedMedia, MediaError> {
        store_in(dir).import(src, MediaKind::DesktopWallpaper)
    }

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn imports_png_copy_that_survives_original_removal() {
        let dir = TempDir::new();
        let src = dir.0.join("photo.png");
        let bytes = png_bytes(4, 3);
        write_file(&src, &bytes);
        let store = store_in(&dir);

        let imported = store.import(&src, MediaKind::DesktopWallpaper).unwrap();
        fs::remove_file(&src).unwrap();

        assert_eq!((imported.width, imported.height), (4, 3));
        assert_eq!(imported.sha256_hex, sha256_hex(&bytes));
        assert!(imported.path.starts_with(&store.root));
        assert_eq!(
            name_of(&imported.path),
            format!("desktop-{}.png", &imported.sha256_hex[..16])
        );
        assert_eq!(fs::read(&imported.path).unwrap(), bytes);
        assert!(store.is_managed(&imported.path));
    }

    #[test]
    fn lock_kind_uses_lock_prefix() {
        let dir = TempDir::new();
        let src = dir.0.join("lock.png");
        write_file(&src, &png_bytes(2, 2));
        let imported = store_in(&dir)
            .import(&src, MediaKind::LockWallpaper)
            .unwrap();
        assert!(name_of(&imported.path).starts_with("lock-"));
    }

    #[test]
    fn avatar_kind_uses_distinct_avatar_prefix() {
        let dir = TempDir::new();
        let src = dir.0.join("face.png");
        write_file(&src, &png_bytes(2, 2));
        let store = store_in(&dir);
        let avatar = store.import(&src, MediaKind::Avatar).unwrap();
        let lock = store.import(&src, MediaKind::LockWallpaper).unwrap();
        let name = name_of(&avatar.path);
        assert!(name.starts_with("avatar-"), "{name}");
        assert!(name.ends_with(".png"), "{name}");
        assert_ne!(
            avatar.path, lock.path,
            "avatar and lock copies must not share a name"
        );
        assert!(store.is_managed(&avatar.path));
    }

    #[test]
    fn avatar_imports_jpeg_and_gif_by_content() {
        let dir = TempDir::new();
        let store = store_in(&dir);
        let jpeg = include_bytes!("../../flamewm-image-core/tests/fixtures/baseline-16x16.jpg");
        let src = dir.0.join("face.bin");
        write_file(&src, jpeg);
        let imported = store.import(&src, MediaKind::Avatar).unwrap();
        assert_eq!((imported.width, imported.height), (16, 16));
        assert!(imported.path.to_str().unwrap().ends_with(".jpg"));
        assert_eq!(fs::read(&imported.path).unwrap(), jpeg);

        let gif = include_bytes!("../../flamewm-image-core/tests/fixtures/first-frame-4x4.gif");
        write_file(&src, gif);
        let imported = store.import(&src, MediaKind::Avatar).unwrap();
        assert_eq!((imported.width, imported.height), (4, 4));
        assert!(imported.path.to_str().unwrap().ends_with(".gif"));
    }

    #[test]
    fn identical_content_is_idempotent() {
        let dir = TempDir::new();
        let src = dir.0.join("a.png");
        write_file(&src, &png_bytes(3, 3));
        let store = store_in(&dir);
        let first = store.import(&src, MediaKind::DesktopWallpaper).unwrap();
        let second = store.import(&src, MediaKind::DesktopWallpaper).unwrap();
        assert_eq!(first, second);
        assert_eq!(fs::read_dir(&store.root).unwrap().count(), 1);
    }

    #[test]
    fn existing_name_with_different_content_is_never_overwritten() {
        let dir = TempDir::new();
        let src = dir.0.join("a.png");
        write_file(&src, &png_bytes(3, 3));
        let store = store_in(&dir);
        let imported = store.import(&src, MediaKind::DesktopWallpaper).unwrap();
        write_file(&imported.path, b"other content");

        let error = store.import(&src, MediaKind::DesktopWallpaper).unwrap_err();

        assert!(matches!(error, MediaError::NameCollision(_)));
        assert_eq!(fs::read(&imported.path).unwrap(), b"other content");
    }

    #[test]
    fn existing_file_comparison_streams_multiple_chunks() {
        let dir = TempDir::new();
        let path = dir.0.join("existing.bin");
        let expected = vec![0x5a; 16 * 1024 * 3 + 7];
        write_file(&path, &expected);
        assert!(existing_file_matches(&path, &expected).unwrap());

        let mut different = expected.clone();
        different[16 * 1024 * 2] ^= 1;
        assert!(!existing_file_matches(&path, &different).unwrap());
    }

    #[test]
    fn publish_race_compares_identical_file_and_rejects_collision() {
        let dir = TempDir::new();
        let store = store_in(&dir);
        let identical = store.root.join("same.bin");
        let payload = vec![0xa5; 32 * 1024 + 3];
        let make_identical = |_: &mut File, bytes: &[u8]| {
            write_file(&identical, bytes);
            Ok(())
        };
        assert!(store.persist(&identical, &payload, make_identical).is_ok());

        let collision = store.root.join("collision.bin");
        let make_collision = |_: &mut File, _: &[u8]| {
            write_file(&collision, b"other content");
            Ok(())
        };
        assert!(matches!(
            store.persist(&collision, &payload, make_collision),
            Err(MediaError::NameCollision(_))
        ));
        assert_eq!(fs::read(&collision).unwrap(), b"other content");
    }

    #[test]
    fn existing_symlink_is_a_collision_without_reading_its_target() {
        let dir = TempDir::new();
        let store = store_in(&dir);
        store.ensure_root().unwrap();
        let dest = store.root.join("linked.bin");
        let outside = dir.0.join("outside.bin");
        write_file(&outside, b"same bytes");
        std::os::unix::fs::symlink(&outside, &dest).unwrap();

        assert!(matches!(
            store.persist(&dest, b"same bytes", |file, bytes| file.write_all(bytes)),
            Err(MediaError::NameCollision(_))
        ));
        assert_eq!(fs::read(&outside).unwrap(), b"same bytes");
    }

    #[test]
    fn failed_write_leaves_no_partial_file() {
        let dir = TempDir::new();
        let store = store_in(&dir);
        let dest = store.root.join("desktop-0123456789abcdef.png");
        let failing = |file: &mut File, _: &[u8]| -> io::Result<()> {
            file.write_all(b"partial")?;
            Err(io::Error::other("simulated disk failure"))
        };

        let error = store.persist(&dest, b"full content", failing).unwrap_err();

        assert!(matches!(error, MediaError::Io(_)));
        assert!(!dest.exists());
        assert_eq!(fs::read_dir(&store.root).unwrap().count(), 0);
    }

    #[test]
    fn failing_reader_surfaces_io_error() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("broken source"))
            }
        }
        assert!(matches!(read_source(Broken), Err(MediaError::Io(_))));
    }

    #[test]
    fn oversized_source_is_rejected_before_reading() {
        let dir = TempDir::new();
        let src = dir.0.join("huge.png");
        let file = File::create(&src).unwrap();
        file.set_len(MAX_SOURCE_BYTES + 1).unwrap();
        drop(file);

        let error = import_desktop(&dir, &src).unwrap_err();

        assert!(matches!(error, MediaError::TooLarge));
    }

    #[test]
    fn read_source_caps_at_limit() {
        let reader = io::repeat(0).take(MAX_SOURCE_BYTES + 10);
        assert!(matches!(read_source(reader), Err(MediaError::TooLarge)));
    }

    #[test]
    fn dimensions_above_4096_are_imported() {
        let dir = TempDir::new();
        let src = dir.0.join("wide.png");
        write_file(&src, &png_bytes(4097, 1));
        let imported = import_desktop(&dir, &src).unwrap();
        assert_eq!((imported.width, imported.height), (4097, 1));
    }

    #[test]
    fn zero_dimension_png_is_rejected() {
        let dir = TempDir::new();
        let src = dir.0.join("zero.png");
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&0u32.to_be_bytes());
        ihdr.extend_from_slice(&5u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        push_chunk(&mut bytes, b"IHDR", &ihdr);
        push_chunk(&mut bytes, b"IEND", &[]);
        write_file(&src, &bytes);
        let error = import_desktop(&dir, &src).unwrap_err();
        assert!(matches!(error, MediaError::Decode(_)));
    }

    /// Minimal valid 1x1 opaque black lossless WebP (VP8L, single-symbol prefix codes).
    fn webp_1x1_bytes() -> Vec<u8> {
        let mut bits: Vec<bool> = Vec::new();
        let mut push = |value: u32, count: u32| {
            for index in 0..count {
                bits.push((value >> index) & 1 == 1);
            }
        };
        push(0x2f, 8); // VP8L signature
        push(0, 14); // width - 1
        push(0, 14); // height - 1
        push(1, 1); // alpha used
        push(0, 3); // version
        push(0, 3); // no transform, no colour cache, no meta prefix
        for alphabet in 0..5 {
            push(1, 1); // simple code
            push(0, 1); // one symbol
            if alphabet == 3 {
                push(1, 1); // alpha symbol is 8 bits wide
                push(255, 8);
            } else {
                push(0, 1);
                push(0, 1);
            }
        }
        let mut payload: Vec<u8> = bits
            .chunks(8)
            .map(|byte| {
                byte.iter()
                    .enumerate()
                    .fold(0u8, |acc, (i, bit)| acc | (u8::from(*bit) << i))
            })
            .collect();
        if payload.len() % 2 == 1 {
            payload.push(0);
        }
        let mut riff = b"WEBP".to_vec();
        riff.extend_from_slice(b"VP8L");
        riff.extend_from_slice(&(bits.len().div_ceil(8) as u32).to_le_bytes());
        riff.extend_from_slice(&payload);
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(riff.len() as u32).to_le_bytes());
        out.extend_from_slice(&riff);
        out
    }

    #[test]
    fn start_icon_accepts_png_webp_and_jpeg_by_content() {
        let dir = TempDir::new();
        let store = store_in(&dir);
        let src = dir.0.join("icon.bin");

        write_file(&src, &png_bytes(2, 2));
        let png = store.import(&src, MediaKind::StartIcon).unwrap();
        assert!(name_of(&png.path).starts_with("start-"));
        assert!(png.path.to_str().unwrap().ends_with(".png"));

        let webp = webp_1x1_bytes();
        write_file(&src, &webp);
        let imported = store.import(&src, MediaKind::StartIcon).unwrap();
        assert_eq!((imported.width, imported.height), (1, 1));
        assert!(imported.path.to_str().unwrap().ends_with(".webp"));
        assert_eq!(fs::read(&imported.path).unwrap(), webp);

        let jpeg = include_bytes!("../../flamewm-image-core/tests/fixtures/baseline-16x16.jpg");
        write_file(&src, jpeg);
        let imported = store.import(&src, MediaKind::StartIcon).unwrap();
        assert_eq!((imported.width, imported.height), (16, 16));
        assert!(imported.path.to_str().unwrap().ends_with(".jpg"));
    }

    #[test]
    fn start_icon_rejects_svg_gif_and_non_images() {
        let dir = TempDir::new();
        let store = store_in(&dir);
        let src = dir.0.join("icon.bin");
        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"4\" height=\"4\"></svg>";
        for bytes in [
            &svg[..],
            include_bytes!("../../flamewm-image-core/tests/fixtures/first-frame-4x4.gif"),
            b"plain text",
        ] {
            write_file(&src, bytes);
            let error = store.import(&src, MediaKind::StartIcon).unwrap_err();
            assert!(
                matches!(error, MediaError::UnsupportedStartIconFormat),
                "{error}"
            );
            assert_eq!(
                error.to_string(),
                "unsupported start icon format (PNG, WebP or JPEG)"
            );
        }
        assert_eq!(
            fs::read_dir(&dir.0.join("store")).map_or(0, |entries| entries.count()),
            0
        );
    }

    #[test]
    fn start_icon_rejects_oversized_source_but_accepts_dimensions_above_4096() {
        let dir = TempDir::new();
        let store = store_in(&dir);
        let huge = dir.0.join("huge.png");
        File::create(&huge)
            .unwrap()
            .set_len(MAX_SOURCE_BYTES + 1)
            .unwrap();
        assert!(matches!(
            store.import(&huge, MediaKind::StartIcon),
            Err(MediaError::TooLarge)
        ));

        let wide = dir.0.join("wide.png");
        write_file(&wide, &png_bytes(4097, 1));
        let imported = store.import(&wide, MediaKind::StartIcon).unwrap();
        assert_eq!((imported.width, imported.height), (4097, 1));
    }

    #[test]
    fn start_icon_managed_path_round_trips_and_only_managed_names_are_removable() {
        let dir = TempDir::new();
        let store = store_in(&dir);
        let src = dir.0.join("icon.png");
        write_file(&src, &png_bytes(2, 2));
        let imported = store.import(&src, MediaKind::StartIcon).unwrap();

        let stored = imported.path.to_str().unwrap().to_owned();
        let reloaded = PathBuf::from(&stored);
        assert_eq!(reloaded, imported.path);
        assert!(store.is_managed(&reloaded));
        assert!(!store.is_managed(&dir.0.join("start-0123456789abcdef.png")));
        assert!(!store.is_managed(&store.root.join("notes.png")));
        assert!(!store.is_managed(&store.root.join("start-0123456789ABCDEF.png")));
    }

    #[test]
    fn unsupported_format_is_rejected() {
        let dir = TempDir::new();
        let src = dir.0.join("photo.bmp");
        write_file(&src, &[b'B', b'M', 0, 0, 0, 0, 0, 0, 0, 0]);
        let error = import_desktop(&dir, &src).unwrap_err();
        assert!(matches!(error, MediaError::UnsupportedFormat));
    }

    #[test]
    fn imports_jpeg_and_gif_by_content_not_extension() {
        let dir = TempDir::new();
        let store = store_in(&dir);
        let jpeg = include_bytes!("../../flamewm-image-core/tests/fixtures/baseline-16x16.jpg");
        let src = dir.0.join("wallpaper.bin");
        write_file(&src, jpeg);
        let imported = store.import(&src, MediaKind::LockWallpaper).unwrap();
        assert_eq!((imported.width, imported.height), (16, 16));
        assert!(imported.path.to_str().unwrap().ends_with(".jpg"));
        assert!(store.is_managed(&imported.path));
        assert_eq!(fs::read(&imported.path).unwrap(), jpeg);

        let gif = include_bytes!("../../flamewm-image-core/tests/fixtures/first-frame-4x4.gif");
        let src = dir.0.join("anim.gif");
        write_file(&src, gif);
        let imported = store.import(&src, MediaKind::DesktopWallpaper).unwrap();
        assert_eq!((imported.width, imported.height), (4, 4));
        assert!(imported.path.to_str().unwrap().ends_with(".gif"));
        assert!(store.is_managed(&imported.path));
    }

    #[test]
    fn corrupt_jpeg_is_rejected_as_decode_error() {
        let dir = TempDir::new();
        let jpeg = include_bytes!("../../flamewm-image-core/tests/fixtures/baseline-16x16.jpg");
        let src = dir.0.join("cut.jpg");
        write_file(&src, &jpeg[..jpeg.len() / 2]);
        let error = import_desktop(&dir, &src).unwrap_err();
        assert!(matches!(error, MediaError::Decode(_)));
    }

    #[test]
    fn progressive_jpeg_is_rejected_as_decode_error() {
        let dir = TempDir::new();
        let jpeg = include_bytes!("../../flamewm-image-core/tests/fixtures/progressive-16x16.jpg");
        let src = dir.0.join("progressive.jpg");
        write_file(&src, jpeg);
        let error = import_desktop(&dir, &src).unwrap_err();
        assert!(matches!(error, MediaError::Decode(_)));
    }

    #[test]
    fn directory_source_is_rejected() {
        let dir = TempDir::new();
        let src = dir.0.join("folder.png");
        fs::create_dir(&src).unwrap();
        let error = import_desktop(&dir, &src).unwrap_err();
        assert!(matches!(error, MediaError::NotRegularFile));
    }

    #[test]
    fn symlink_source_is_resolved_and_copied() {
        let dir = TempDir::new();
        let target = dir.0.join("real.png");
        let bytes = png_bytes(2, 5);
        write_file(&target, &bytes);
        let link = dir.0.join("link.png");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let imported = store_in(&dir)
            .import(&link, MediaKind::LockWallpaper)
            .unwrap();
        assert_eq!(fs::read(&imported.path).unwrap(), bytes);
    }

    #[test]
    fn dangling_symlink_source_is_rejected() {
        let dir = TempDir::new();
        let link = dir.0.join("dangling.png");
        std::os::unix::fs::symlink(dir.0.join("missing.png"), &link).unwrap();
        assert!(matches!(
            store_in(&dir).import(&link, MediaKind::DesktopWallpaper),
            Err(MediaError::Io(_))
        ));
    }

    #[test]
    fn stored_files_and_dirs_are_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = TempDir::new();
        let src = dir.0.join("p.png");
        write_file(&src, &png_bytes(1, 1));
        let store = store_in(&dir);
        let imported = store.import(&src, MediaKind::DesktopWallpaper).unwrap();
        let dir_mode = fs::metadata(&store.root).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(&imported.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    #[test]
    fn gc_removes_only_unreferenced_managed_files() {
        let dir = TempDir::new();
        let store = store_in(&dir);
        fs::create_dir_all(&store.root).unwrap();
        let current = store.root.join("desktop-0000000000000001.png");
        let previous = store.root.join("lock-0000000000000002.webp");
        let stale = store.root.join("desktop-0000000000000003.png");
        let foreign = store.root.join("notes.txt");
        for path in [&current, &previous, &stale, &foreign] {
            write_file(path, b"x");
        }

        store.gc(&[current.clone(), previous.clone()]).unwrap();

        assert!(current.exists());
        assert!(previous.exists());
        assert!(!stale.exists());
        assert!(foreign.exists());
    }

    #[test]
    fn gc_never_follows_symlinks_out_of_root() {
        let dir = TempDir::new();
        let store = store_in(&dir);
        fs::create_dir_all(&store.root).unwrap();
        let outside = dir.0.join("outside.png");
        write_file(&outside, b"outside");
        let link = store.root.join("desktop-00000000000000aa.png");
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        store.gc(&[]).unwrap();

        assert_eq!(fs::read(&outside).unwrap(), b"outside");
        assert!(fs::symlink_metadata(&link).is_ok());
    }

    #[test]
    fn is_managed_rejects_foreign_paths() {
        let dir = TempDir::new();
        let store = store_in(&dir);
        assert!(store.is_managed(&store.root.join("lock-0123456789abcdef.png")));
        assert!(!store.is_managed(&store.root.join("lock-0123456789ABCDEF.png")));
        assert!(!store.is_managed(&store.root.join("lock-0123456789abcdef.bmp")));
        assert!(!store.is_managed(&dir.0.join("lock-0123456789abcdef.png")));
        assert!(!store.is_managed(&store.root.join("sub/desktop-0123456789abcdef.png")));
        assert!(store.is_managed(&store.root.join("avatar-0123456789abcdef.webp")));
        assert!(!store.is_managed(&store.root.join("avatar-0123456789ABCDEF.webp")));
    }

    #[test]
    fn default_root_prefers_xdg_then_home() {
        assert_eq!(
            user_data_home(Some(OsStr::new("/home/u")), Some(OsStr::new("/data")))
                .map(|data| data.join(STORE_SUBDIR)),
            Some(PathBuf::from("/data/flamewm/wallpapers"))
        );
        assert_eq!(
            user_data_home(Some(OsStr::new("/home/u")), None).map(|data| data.join(STORE_SUBDIR)),
            Some(PathBuf::from("/home/u/.local/share/flamewm/wallpapers"))
        );
    }
}
