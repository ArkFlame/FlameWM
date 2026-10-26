use super::*;
use std::time::UNIX_EPOCH;

fn key(name: &str, secs: u64) -> ThumbKey {
    ThumbKey {
        path: PathBuf::from(name),
        modified: Some(UNIX_EPOCH + std::time::Duration::from_secs(secs)),
        size: 10,
    }
}

/// 1x1 opaque red PNG, generated once with PIL and embedded so the test needs no fixture file.
const PNG_1X1: [u8; 70] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// An SVG that decodes to a fully transparent canvas: a picture with nothing to paint.
const SVG_INVISIBLE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#e5484d" fill-opacity="0"/></svg>"##;

const SVG_SQUARE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8" viewBox="0 0 8 8"><rect width="8" height="8" fill="#e5484d"/></svg>"##;

/// One-entry 32 bpp ICO: a 2x2 opaque white square, BGRA rows bottom-up.
fn ico_2x2() -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&40u32.to_le_bytes());
    payload.extend_from_slice(&2i32.to_le_bytes());
    payload.extend_from_slice(&4i32.to_le_bytes());
    payload.extend_from_slice(&1u16.to_le_bytes());
    payload.extend_from_slice(&32u16.to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload.extend_from_slice(&16u32.to_le_bytes());
    payload.extend_from_slice(&[0; 16]);
    payload.extend_from_slice(&[255; 16]);
    payload.extend_from_slice(&[0; 8]);
    let mut file = vec![0, 0, 1, 0, 1, 0, 2, 2, 0, 0];
    file.extend_from_slice(&1u16.to_le_bytes());
    file.extend_from_slice(&32u16.to_le_bytes());
    file.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    file.extend_from_slice(&22u32.to_le_bytes());
    file.extend_from_slice(&payload);
    file
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "flamewm-thumb-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn entry_of(path: &Path) -> Entry {
    Entry::new(
        path.to_path_buf(),
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        false,
        false,
        fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        None,
    )
}

#[test]
fn image_vector_icon_and_launcher_rows_are_thumbnailable() {
    for name in ["a.PNG", "b.jpg", "c.jpeg", "d.gif", "e.webp"] {
        assert!(
            matches!(thumb_format(Path::new(name)), Some(ThumbFormat::Raster(_))),
            "{name}"
        );
    }
    assert_eq!(thumb_format(Path::new("v.svg")), Some(ThumbFormat::Svg));
    assert_eq!(thumb_format(Path::new("app.ICO")), Some(ThumbFormat::Ico));
    assert_eq!(thumb_format(Path::new("app.cur")), Some(ThumbFormat::Ico));
    assert_eq!(
        thumb_format(Path::new("app.desktop")),
        Some(ThumbFormat::Desktop)
    );
    // PPM stays a build format, and unknown extensions are not thumbnails.
    for name in ["f.bmp", "g.ppm", "h.txt", "noext"] {
        assert_eq!(thumb_format(Path::new(name)), None, "{name}");
    }
    assert_eq!(source_format(Path::new("v.svg")), None);
    assert_eq!(
        ThumbKey::of(&entry_of(Path::new("v.svg"))).map(|k| k.path),
        Some(PathBuf::from("v.svg"))
    );
}

#[test]
fn png_header_gives_dimensions_without_decoding() {
    assert_eq!(
        header_dimensions(&PNG_1X1, ImageSourceFormat::Png),
        Some((1, 1))
    );
    let mut bogus = PNG_1X1;
    bogus[12..16].copy_from_slice(b"JUNK");
    assert_eq!(header_dimensions(&bogus, ImageSourceFormat::Png), None);
}

#[test]
fn jpeg_scan_skips_app_segments_to_reach_sof() {
    // SOI, APP0 of length 4 (two payload bytes), then SOF0 with height 300 and width 640.
    let jpeg = [
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x01, 0x2C,
        0x02, 0x80,
    ];
    assert_eq!(
        header_dimensions(&jpeg, ImageSourceFormat::Jpeg),
        Some((640, 300))
    );
}

#[test]
fn gif_and_webp_headers_report_canvas_size() {
    let mut gif = b"GIF89a".to_vec();
    gif.extend_from_slice(&[0x40, 0x01, 0x20, 0x00]);
    assert_eq!(
        header_dimensions(&gif, ImageSourceFormat::Gif),
        Some((320, 32))
    );
    let mut webp = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
    // Payload: 4 flag bytes and 4 reserved bytes, then canvas width-1 and height-1 as 24-bit little-endian.
    webp.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0x1f, 0x00, 0x00, 0x09, 0x00, 0x00]);
    assert_eq!(
        header_dimensions(&webp, ImageSourceFormat::Webp),
        Some((32, 10))
    );
}

#[test]
fn svg_and_ico_headers_probe_without_rendering() {
    assert_eq!(probe_dimensions(SVG_SQUARE, ThumbFormat::Svg), Some((8, 8)));
    assert_eq!(probe_dimensions(&ico_2x2(), ThumbFormat::Ico), Some((2, 2)));
    // A vector without intrinsic dimensions probes as the SVG default canvas, and a broken one
    // probes as nothing at all.
    assert_eq!(
        probe_dimensions(
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
            ThumbFormat::Svg
        ),
        Some((100, 100))
    );
    assert_eq!(probe_dimensions(b"<svg", ThumbFormat::Svg), None);
    assert_eq!(probe_dimensions(&PNG_1X1, ThumbFormat::Desktop), None);
}

#[test]
fn fit_never_upscales_and_keeps_aspect() {
    assert_eq!(fit_dimensions(10, 5, THUMB_SIZE), (10, 5));
    assert_eq!(
        fit_dimensions(480, 240, THUMB_SIZE),
        (THUMB_SIZE, THUMB_SIZE / 2)
    );
    assert_eq!(
        fit_dimensions(240, 480, THUMB_SIZE),
        (THUMB_SIZE / 2, THUMB_SIZE)
    );
}

#[test]
fn decoded_png_is_centred_on_transparent_square() {
    let dir = temp_dir("png");
    let path = dir.join("red.png");
    fs::write(&path, PNG_1X1).unwrap();
    let thumb = render_thumbnail(&ThumbKey::of(&entry_of(&path)).unwrap()).expect("decodes");
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(thumb.len() as u32, THUMB_SIZE * THUMB_SIZE * 4);
    // A 1x1 image is not upscaled: exactly one opaque pixel at the centre, everything else transparent.
    let edge = THUMB_SIZE as usize;
    let opaque: Vec<usize> = thumb
        .chunks_exact(4)
        .enumerate()
        .filter(|(_, p)| p[3] == 255)
        .map(|(i, _)| i)
        .collect();
    let offset = (edge - 1) / 2;
    let centre = offset * edge + offset;
    assert_eq!(opaque, vec![centre]);
    assert_eq!(&thumb[centre * 4..centre * 4 + 4], &[255, 0, 0, 255]);
}

#[test]
fn svg_row_renders_at_thumbnail_size() {
    let dir = temp_dir("svg");
    let path = dir.join("square.svg");
    fs::write(&path, SVG_SQUARE).unwrap();
    let thumb = render_thumbnail(&ThumbKey::of(&entry_of(&path)).unwrap()).expect("renders");
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(thumb.len() as u32, THUMB_SIZE * THUMB_SIZE * 4);
    // An 8x8 source fits inside the box without upscaling: 8*8 opaque pixels, the rest transparent.
    let opaque = thumb.chunks_exact(4).filter(|p| p[3] == 255).count();
    assert_eq!(opaque, 8 * 8);
}

#[test]
fn ico_row_decodes_its_largest_entry() {
    let dir = temp_dir("ico");
    let path = dir.join("app.ico");
    fs::write(&path, ico_2x2()).unwrap();
    let thumb = render_thumbnail(&ThumbKey::of(&entry_of(&path)).unwrap()).expect("decodes");
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(thumb.len() as u32, THUMB_SIZE * THUMB_SIZE * 4);
    let opaque = thumb.chunks_exact(4).filter(|p| p[3] == 255).count();
    assert_eq!(opaque, 4, "a 2x2 icon is never upscaled");
    assert!(
        thumb
            .chunks_exact(4)
            .any(|p| p[3] == 255 && p[0] == 255 && p[1] == 255)
    );
}

#[test]
fn desktop_row_shows_the_application_icon_from_its_metadata() {
    let dir = temp_dir("desktop");
    let icons = dir.join("icons");
    let apps = icons.join("hicolor/48x48/apps");
    fs::create_dir_all(&apps).unwrap();
    fs::write(apps.join("myapp.png"), PNG_1X1).unwrap();
    let desktop = dir.join("myapp.desktop");
    fs::write(
        &desktop,
        "[Desktop Entry]\nType=Application\nName=My App\nIcon=myapp\nExec=/bin/false\n",
    )
    .unwrap();
    // The `.desktop` row is a thumbnail row even though the file is not a picture.
    assert_eq!(
        ThumbKey::of(&entry_of(&desktop)).map(|key| key.path),
        Some(desktop.clone())
    );
    // The resolver is injected here: the real icon roots cannot see this fixture.
    let icon =
        appicon::icon_file_in(&desktop, std::slice::from_ref(&icons)).expect("icon resolves");
    assert_eq!(icon, apps.join("myapp.png"));
    let thumb = thumbnail_from_bytes(
        &read_source(&icon).unwrap(),
        ThumbFormat::Raster(ImageSourceFormat::Png),
    )
    .expect("decodes");
    assert!(visible_canvas(&thumb));
    // A hidden entry names no icon, so the row falls back to the embedded file icon.
    fs::write(
        &desktop,
        "[Desktop Entry]\nType=Application\nHidden=true\nIcon=myapp\n",
    )
    .unwrap();
    assert!(appicon::icon_file_in(&desktop, std::slice::from_ref(&icons)).is_none());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn invisible_or_broken_sources_fall_back_instead_of_showing_an_empty_slot() {
    let dir = temp_dir("fallback");
    // Decodable but fully transparent: nothing would be painted, so the row keeps its type icon.
    let clear = dir.join("clear.svg");
    fs::write(&clear, SVG_INVISIBLE).unwrap();
    assert!(thumbnail_from_bytes(&read_source(&clear).unwrap(), ThumbFormat::Svg).is_none());
    assert!(render_thumbnail(&ThumbKey::of(&entry_of(&clear)).unwrap()).is_none());
    // Truncated and garbage files are misses, not panics.
    let broken = dir.join("broken.png");
    fs::write(&broken, &PNG_1X1[..40]).unwrap();
    assert!(render_thumbnail(&ThumbKey::of(&entry_of(&broken)).unwrap()).is_none());
    let garbage = dir.join("garbage.ico");
    fs::write(&garbage, b"not an icon at all").unwrap();
    assert!(render_thumbnail(&ThumbKey::of(&entry_of(&garbage)).unwrap()).is_none());
    let svg_broken = dir.join("broken.svg");
    fs::write(
        &svg_broken,
        b"<svg width=\"4\" height=\"4\"><rect width=\"4\"",
    )
    .unwrap();
    assert!(render_thumbnail(&ThumbKey::of(&entry_of(&svg_broken)).unwrap()).is_none());
    // An image whose header claims more pixels than the pipeline decodes is skipped before decoding.
    let huge = dir.join("huge.png");
    let mut header = PNG_1X1.to_vec();
    header[16..20].copy_from_slice(&10_000u32.to_be_bytes());
    header[20..24].copy_from_slice(&10_000u32.to_be_bytes());
    fs::write(&huge, header).unwrap();
    assert!(render_thumbnail(&ThumbKey::of(&entry_of(&huge)).unwrap()).is_none());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn missing_and_over_cap_sources_are_misses_without_reading_them() {
    let dir = temp_dir("bounds");
    // A file listed but removed before the worker reaches it: a miss, not a panic.
    let missing = dir.join("gone.png");
    fs::write(&missing, PNG_1X1).unwrap();
    let key = ThumbKey::of(&entry_of(&missing)).unwrap();
    fs::remove_file(&missing).unwrap();
    assert_eq!(render_thumbnail(&key), None);
    assert_eq!(read_source(&missing), None);
    // A sparse file one byte over the cap is refused by its metadata, so its bytes are never read.
    let oversized = dir.join("over.png");
    let file = fs::File::create(&oversized).unwrap();
    file.set_len(MAX_SOURCE_BYTES + 1).unwrap();
    assert!(fs::metadata(&oversized).unwrap().len() > MAX_SOURCE_BYTES);
    assert_eq!(read_source(&oversized), None);
    assert_eq!(
        render_thumbnail(&ThumbKey::of(&entry_of(&oversized)).unwrap()),
        None
    );
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn directories_and_non_images_are_never_queued() {
    let mut folder = entry_of(Path::new("/tmp/folder"));
    folder.directory = true;
    assert!(ThumbKey::of(&folder).is_none());
    assert!(ThumbKey::of(&entry_of(Path::new("/tmp/notes.txt"))).is_none());
}

#[test]
fn cache_is_bounded_lru_and_caches_failures() {
    let mut cache = Thumbnails::default();
    assert!(cache.begin(&key("a", 1)));
    assert!(
        !cache.begin(&key("a", 1)),
        "second request for a queued key is ignored"
    );
    cache.complete(key("a", 1), None);
    assert!(matches!(cache.lookup(&key("a", 1)), Lookup::Failed));
    for i in 0..CACHE_CAPACITY - 1 {
        cache.complete(key(&format!("f{i}"), 1), Some(vec![0; 4]));
    }
    assert_eq!(cache.entries.len(), CACHE_CAPACITY);
    // Touching "a" makes it the newest, so the insert that overflows the cache evicts "f0" instead.
    assert!(matches!(cache.lookup(&key("a", 1)), Lookup::Failed));
    cache.complete(key("new", 1), Some(vec![0; 4]));
    assert_eq!(cache.entries.len(), CACHE_CAPACITY);
    assert!(matches!(cache.lookup(&key("f0", 1)), Lookup::Missing));
    assert!(matches!(cache.lookup(&key("a", 1)), Lookup::Failed));
    assert!(matches!(cache.lookup(&key("new", 1)), Lookup::Ready(..)));
}

#[test]
fn changed_mtime_is_a_different_key() {
    assert_ne!(key("a", 1), key("a", 2));
}
