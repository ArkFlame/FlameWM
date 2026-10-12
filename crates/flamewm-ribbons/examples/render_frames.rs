//! Dev tool: renders the Ribbons instrument at 1280x720, N=6, to binary PPM (P6) frames.
//!
//! Usage: `cargo run -p flamewm-ribbons --release --example render_frames -- <out_dir> [seed]`
//! Frames are written as `frame_t03.ppm`, `frame_t06.ppm`, `frame_t09.ppm`, `frame_t12.ppm`.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use flamewm_ribbons::{Config, Quality, Rasterizer, Ribbons};

const W: u32 = 1280;
const H: u32 = 720;
const FRAME_TIMES: [u64; 4] = [3, 6, 9, 12];

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next().map(PathBuf::from) else {
        eprintln!("usage: render_frames <out_dir> [seed]");
        std::process::exit(2);
    };
    let seed: u64 = args.next().map_or(42, |s| s.parse().unwrap_or(42));
    fs::create_dir_all(&dir)?;

    let mut state = Ribbons::new(
        Config {
            num: 6,
            width_px: 11.0,
            span_rects: Vec::new(),
            seed,
        },
        0.0,
    );
    let mut rast = Rasterizer::new(Quality::Half);
    let mut bgra = vec![0u8; (W * H * 4) as usize];
    let mut next = FRAME_TIMES.iter().peekable();
    for k in 1..=(12 * 60) {
        state.step_to(k as f64 / 60.0);
        if next.peek().is_some_and(|&&sec| k == sec * 60) {
            let sec = *next.next().unwrap();
            rast.render(&state, &mut bgra);
            let mut rgb = Vec::with_capacity((W * H * 3) as usize);
            for px in bgra.chunks_exact(4) {
                rgb.extend_from_slice(&[px[2], px[1], px[0]]);
            }
            let path = dir.join(format!("frame_t{sec:02}.ppm"));
            let mut file = fs::File::create(&path)?;
            write!(file, "P6\n{W} {H}\n255\n")?;
            file.write_all(&rgb)?;
            println!("wrote {}", path.display());
        }
    }
    Ok(())
}
