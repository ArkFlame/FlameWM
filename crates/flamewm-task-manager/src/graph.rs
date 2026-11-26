//! Small line graphs for the metric cards: a one-minute history per series, drawn into one
//! straight-alpha RGBA raster per card on each sample. No per-frame state beyond the history.

pub const WIDTH: u32 = 208;
pub const HEIGHT: u32 = 88;
pub const SAMPLES: usize = 60;

const GRID: [u8; 4] = [255, 255, 255, 22];
const AREA_ALPHA: u8 = 52;

/// Fixed-size ring: `SAMPLES` values with an O(1) push and no shifting or reallocation.
pub struct History {
    values: [f32; SAMPLES],
    /// Number of valid entries, at most `SAMPLES`.
    len: usize,
    /// Physical slot that the next push overwrites.
    head: usize,
}

impl Default for History {
    fn default() -> Self {
        Self {
            values: [0.0; SAMPLES],
            len: 0,
            head: 0,
        }
    }
}

impl History {
    pub fn push(&mut self, value: f32) {
        self.values[self.head] = if value.is_finite() {
            value.max(0.0)
        } else {
            0.0
        };
        self.head = (self.head + 1) % SAMPLES;
        if self.len < SAMPLES {
            self.len += 1;
        }
    }

    /// Logical order, oldest sample first.
    fn in_order(&self) -> impl Iterator<Item = f32> + '_ {
        let start = (self.head + SAMPLES - self.len) % SAMPLES;
        (0..self.len).map(move |index| self.values[(start + index) % SAMPLES])
    }

    /// Copies the history in logical order into `out`; returns the number of written entries.
    fn copy_in_order(&self, out: &mut [f32; SAMPLES]) -> usize {
        let mut count = 0;
        for (slot, value) in out.iter_mut().zip(self.in_order()) {
            *slot = value;
            count += 1;
        }
        count
    }

    pub fn peak(&self) -> f32 {
        self.in_order().fold(0.0, f32::max)
    }
}

/// One series of a graph: its history, its colour and whether the area under it is filled.
pub struct Series<'a> {
    pub history: &'a History,
    pub rgb: u32,
    pub filled: bool,
}

/// Draws `series` over a faint grid, each value scaled against `full_scale`, newest sample at
/// the right edge. `pixels` is reused: clearing and resizing keeps its capacity, so a visible
/// task manager redraws its three graphs every sample without allocating a fresh raster.
pub fn render_into(series: &[Series<'_>], full_scale: f32, pixels: &mut Vec<u8>) {
    let (width, height) = (WIDTH as usize, HEIGHT as usize);
    pixels.clear();
    pixels.resize(width * height * 4, 0);
    for quarter in 1..4 {
        let y = height * quarter / 4;
        for x in (0..width).step_by(2) {
            blend(&mut pixels[..], x, y, GRID);
        }
    }
    let scale = if full_scale > 0.0 { full_scale } else { 1.0 };
    let step = (width - 1) as f32 / (SAMPLES - 1) as f32;
    let mut values = [0.0_f32; SAMPLES];
    for line in series {
        let count = line.history.copy_in_order(&mut values);
        if count == 0 {
            continue;
        }
        let values = &values[..count];
        let [r, g, b] = [
            (line.rgb >> 16) as u8,
            (line.rgb >> 8) as u8,
            line.rgb as u8,
        ];
        let first_x = (width - 1) as f32 - (values.len() - 1) as f32 * step;
        let y_of = |value: f32| (height - 1) as f32 * (1.0 - (value / scale).clamp(0.0, 1.0));
        let mut previous_y: Option<usize> = None;
        for x in first_x.ceil().max(0.0) as usize..width {
            let position = (x as f32 - first_x) / step;
            let index = (position.floor() as usize).min(values.len() - 1);
            let next = (index + 1).min(values.len() - 1);
            let blend_factor = position - index as f32;
            let value = values[index] + (values[next] - values[index]) * blend_factor;
            let y = y_of(value).round() as usize;
            if line.filled {
                for fill_y in y..height {
                    blend(&mut pixels[..], x, fill_y, [r, g, b, AREA_ALPHA]);
                }
            }
            let (top, bottom) = previous_y.map_or((y, y), |last| (last.min(y), last.max(y)));
            for line_y in top..=(bottom + 1).min(height - 1) {
                blend(&mut pixels[..], x, line_y, [r, g, b, 255]);
            }
            previous_y = Some(y);
        }
    }
}

fn blend(pixels: &mut [u8], x: usize, y: usize, source: [u8; 4]) {
    let index = (y * WIDTH as usize + x) * 4;
    let Some(target) = pixels.get_mut(index..index + 4) else {
        return;
    };
    // An opaque line and an empty pixel both resolve to the source exactly; skipping the
    // floating-point source-over for them removes most of the per-sample graph math.
    if source[3] == 255 || (source[3] != 0 && target[3] == 0) {
        target.copy_from_slice(&source);
        return;
    }
    let source_alpha = f32::from(source[3]) / 255.0;
    let target_alpha = f32::from(target[3]) / 255.0;
    let alpha = source_alpha + target_alpha * (1.0 - source_alpha);
    if alpha <= 0.0 {
        return;
    }
    for channel in 0..3 {
        let mixed = (f32::from(source[channel]) * source_alpha
            + f32::from(target[channel]) * target_alpha * (1.0 - source_alpha))
            / alpha;
        target[channel] = mixed.round() as u8;
    }
    target[3] = (alpha * 255.0).round() as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only convenience: draws into a fresh raster through the reuse path.
    fn raster(series: &[Series<'_>], full_scale: f32) -> Vec<u8> {
        let mut pixels = Vec::new();
        render_into(series, full_scale, &mut pixels);
        pixels
    }

    #[test]
    fn history_keeps_one_minute_and_rejects_bad_values() {
        let mut history = History::default();
        for value in 0..100 {
            history.push(value as f32);
        }
        history.push(f32::NAN);
        history.push(-4.0);
        assert_eq!(history.len, SAMPLES);
        assert_eq!(history.in_order().last(), Some(0.0));
        assert_eq!(history.peak(), 99.0);
    }

    #[test]
    fn full_value_draws_the_line_at_the_top_right() {
        let mut history = History::default();
        history.push(100.0);
        history.push(100.0);
        let pixels = raster(
            &[Series {
                history: &history,
                rgb: 0x0012_3456,
                filled: true,
            }],
            100.0,
        );
        let top_right = ((WIDTH - 1) * 4) as usize;
        assert_eq!(&pixels[top_right..top_right + 4], &[0x12, 0x34, 0x56, 255]);
        assert_eq!(pixels.len(), (WIDTH * HEIGHT * 4) as usize);
        assert_eq!(pixels[3], 0);
    }

    #[test]
    fn empty_graph_is_only_the_grid() {
        let pixels = raster(&[], 0.0);
        let grid_row = (HEIGHT as usize / 4) * WIDTH as usize * 4;
        assert_eq!(pixels[grid_row + 3], GRID[3]);
        assert!(pixels[..grid_row].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn render_into_reuses_the_caller_buffer_and_clears_it() {
        let mut history = History::default();
        history.push(100.0);
        // A wrong-sized, dirty buffer is reset in place to the raster size and cleared.
        let mut pixels = vec![7_u8; 10];
        render_into(
            &[Series {
                history: &history,
                rgb: 0x0012_3456,
                filled: true,
            }],
            100.0,
            &mut pixels,
        );
        assert_eq!(pixels.len(), (WIDTH * HEIGHT * 4) as usize);
        assert_eq!(pixels[3], 0);
        let capacity = pixels.capacity();
        render_into(&[], 0.0, &mut pixels);
        assert_eq!(pixels.capacity(), capacity, "capacity is kept across draws");
    }

    #[test]
    fn blend_fast_paths_preserve_the_source_value() {
        let mut pixels = vec![0_u8; 16];
        // Empty target: straight-alpha source-over resolves to the source exactly.
        blend(&mut pixels, 1, 0, [10, 20, 30, 52]);
        assert_eq!(&pixels[4..8], &[10, 20, 30, 52]);
        // Opaque line over a translucent fill stays exactly opaque.
        blend(&mut pixels, 1, 0, [200, 100, 50, 255]);
        assert_eq!(&pixels[4..8], &[200, 100, 50, 255]);
    }
}
