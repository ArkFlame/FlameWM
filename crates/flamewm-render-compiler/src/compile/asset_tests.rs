use super::tests::*;
use super::*;

#[test]
fn parses_binary_ppm_asset_payload() {
    let mut bytes = b"P6\n1 1\n255\n".to_vec();
    bytes.extend_from_slice(&[0x12, 0x34, 0x56]);
    let parsed = flamewm_image_core::ppm::decode(&bytes).unwrap();
    assert_eq!(parsed.width, 1);
    assert_eq!(parsed.height, 1);
    assert_eq!(parsed.pixels, vec![0x12, 0x34, 0x56, 255]);
}

#[test]
fn opaque_ppm_asset_compiles_to_fully_opaque_rgba8() {
    use std::io::Write;
    let dir = temp_fixture_dir("j05-ppm");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let ppm = dir.join("icon.ppm");
    let mut bytes = b"P6\n1 1\n255\n".to_vec();
    bytes.extend_from_slice(&[0x11, 0x22, 0x33]);
    std::fs::write(&ppm, &bytes).expect("temp ppm");
    let html = dir.join("doc.html");
    let mut file = std::fs::File::create(&html).expect("temp html");
    write!(
        file,
        "<html><body><img id=\"i\" src=\"icon.ppm\"></body></html>"
    )
    .expect("write html");
    let output = compile_file(&html, &[], CompileOptions::default()).expect("compile");
    assert_eq!(output.document.assets.len(), 1);
    assert_eq!(
        output.document.assets[0].pixels,
        vec![0x11, 0x22, 0x33, 255]
    );
    assert!(output.document.assets[0].is_fully_opaque());
}

fn compile_svg_html(name: &str, body: &str) -> CompileOutput {
    use std::io::Write;
    let breeze = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/web/breeze");
    let bytes = std::fs::read(breeze.join(name)).expect("read breeze svg");
    let dir = temp_fixture_dir(&format!("j03-svg-{name}"));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join(name), &bytes).expect("copy svg");
    let html = dir.join("doc.html");
    let mut file = std::fs::File::create(&html).expect("temp html");
    write!(file, "<html><body>{body}</body></html>").expect("write html");
    compile_file(&html, &[], CompileOptions::default()).expect("compile")
}

fn compile_svg_fixture(name: &str) -> CompileOutput {
    compile_svg_html(name, &format!("<img id=\"i\" src=\"{name}\">"))
}

fn mean_rgb(pixels: &[u8]) -> (f32, f32, f32) {
    let mut sum = [0u64; 3];
    let mut count = 0u64;
    for pixel in pixels.chunks_exact(4) {
        if pixel[3] > 8 {
            sum[0] += u64::from(pixel[0]);
            sum[1] += u64::from(pixel[1]);
            sum[2] += u64::from(pixel[2]);
            count += 1;
        }
    }
    assert!(count > 0, "expected visible pixels");
    (
        sum[0] as f32 / count as f32,
        sum[1] as f32 / count as f32,
        sum[2] as f32 / count as f32,
    )
}

fn count_near(pixels: &[u8], target: [u8; 3], tol: i16) -> usize {
    pixels
        .chunks_exact(4)
        .filter(|pixel| {
            pixel[3] > 8
                && (i16::from(pixel[0]) - i16::from(target[0])).abs() <= tol
                && (i16::from(pixel[1]) - i16::from(target[1])).abs() <= tol
                && (i16::from(pixel[2]) - i16::from(target[2])).abs() <= tol
        })
        .count()
}

#[test]
fn semantic_svg_text_fallback_compiles_to_light_text() {
    let output = compile_svg_fixture("network-wireless.svg");
    assert_eq!(output.document.assets.len(), 1);
    let (r, g, b) = mean_rgb(&output.document.assets[0].pixels);
    assert!(
        (r - 0xf1 as f32).abs() < 20.0
            && (g - 0xf2 as f32).abs() < 20.0
            && (b - 0xf3 as f32).abs() < 20.0,
        "mean=({r},{g},{b}) expected near #f1f2f3"
    );
}

#[test]
fn semantic_svg_accent_compiles_to_flame_red_preserving_alpha() {
    let output = compile_svg_fixture("folder.svg");
    assert_eq!(output.document.assets.len(), 1);
    let asset = &output.document.assets[0];
    assert!(asset.pixels.chunks_exact(4).any(|p| p[3] > 8 && p[3] < 255));
    assert!(count_near(&asset.pixels, [0xef, 0x40, 0x48], 32) > 200);
    // Dedup: same src twice in one document yields one asset, both nodes share it.
    let dup = compile_svg_html(
        "folder.svg",
        r#"<img id="a" src="folder.svg"><img id="b" src="folder.svg">"#,
    );
    assert_eq!(dup.document.assets.len(), 1);
    let image_of = |id: &str| {
        dup.document
            .nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap()
            .image
            .unwrap()
    };
    assert_eq!(image_of("a"), image_of("b"));
}

#[test]
fn full_color_svg_compiles_unchanged() {
    let output = compile_svg_fixture("applications-games.svg");
    assert_eq!(output.document.assets.len(), 1);
    let asset = &output.document.assets[0];
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/web/breeze/applications-games.svg"),
    )
    .expect("read games svg");
    let (w, h) = flamewm_image_core::svg::natural_size(&bytes).unwrap();
    let plain = flamewm_image_core::svg::render(&bytes, w, h).unwrap();
    assert_eq!(asset.width, w);
    assert_eq!(asset.height, h);
    assert_eq!(asset.pixels, plain.pixels);
    // Multi-color: distinct blue, green, red, yellow populations.
    assert!(count_near(&asset.pixels, [0x0c, 0x85, 0xdc], 64) > 0);
    assert!(count_near(&asset.pixels, [0x27, 0xab, 0x5f], 64) > 0);
    assert!(count_near(&asset.pixels, [0xda, 0x44, 0x53], 64) > 0);
    assert!(count_near(&asset.pixels, [0xfd, 0xae, 0x23], 64) > 0);
}

#[test]
fn internet_svg_compiles_readable() {
    let output = compile_svg_fixture("internet-web-browser.svg");
    assert_eq!(output.document.assets.len(), 1);
    let asset = &output.document.assets[0];
    let visible: usize = asset.pixels.chunks_exact(4).filter(|p| p[3] > 8).count();
    assert!(visible > 100, "expected readable icon, visible={visible}");
    assert!(count_near(&asset.pixels, [0x19, 0x7c, 0xf1], 64) > 20);
}

#[test]
fn focus_visible_and_focus_compile_to_focus_style() {
    let html = r#"<body><button id="a">A</button><button id="b">B</button><button id="c">C</button></body>"#;
    let css = "#a:focus-visible { border-color: #ff0000; } #b:focus { border-color: #00ff00; }";
    let output = compile_str(html, css, CompileOptions::default()).unwrap();
    let focus_of = |id: &str| {
        output
            .document
            .nodes
            .iter()
            .find(|node| node.id == id)
            .unwrap()
            .focus_style
            .is_some()
    };
    assert!(focus_of("a"));
    assert!(focus_of("b"));
    assert!(!focus_of("c"));
}

#[test]
fn focus_ring_token_is_builtin_accent_derived() {
    let output = compile_body("<body><button id='a'>A</button></body>");
    assert!(output
        .document
        .variables
        .iter()
        .any(|variable| variable.name == "--focus-ring"));
}
