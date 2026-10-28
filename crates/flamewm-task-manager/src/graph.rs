//! Small line graphs for the metric cards: a one-minute history per series, drawn into one
//! straight-alpha RGBA raster per card on each sample. No per-frame state beyond the history.

pub const WIDTH: u32 = 208;
pub const HEIGHT: u32 = 88;
pub const SAMPLES: usize = 60;

const GRID: [u8; 4] = [255, 255, 255, 22];
const AREA_ALPHA: u8 = 52;

#[derive(Default)]
pub struct History {
    values: Vec<f32>,
}

impl History {
    pub fn push(&mut self, value: f32) {
        if self.values.len() == SAMPLES {
            self.values.remove(0);
        }
        self.values.push(if value.is_finite() {
            value.max(0.0)
        } else {
            0.0
        });
    }

    pub fn peak(&self) -> f32 {
        self.values.iter().copied().fold(0.0, f32::max)
    }
}

/// One series of a graph: its history, its colour and whether the area under it is filled.
pub struct Series<'a> {
    pub history: &'a History,
    pub rgb: u32,
    pub filled: bool,
}

/// Draws `series` over a faint grid, each value scaled against `full_scale`, newest sample at
/// the right edge.
#[must_use]
pub fn render(series: &[Series<'_>], full_scale: f32) -> Vec<u8> {
    let (width, height) = (WIDTH as usize, HEIGHT as usize);
    let mut pixels = vec![0_u8; width * height * 4];
    for quarter in 1..4 {
        let y = height * quarter / 4;
        for x in (0..width).step_by(2) {
            blend(&mut pixels, x, y, GRID);
        }
    }
    let scale = if full_scale > 0.0 { full_scale } else { 1.0 };
    let step = (width - 1) as f32 / (SAMPLES - 1) as f32;
    for line in series {
        let values = &line.history.values;
        if values.is_empty() {
            continue;
        }
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
                    blend(&mut pixels, x, fill_y, [r, g, b, AREA_ALPHA]);
                }
            }
            let (top, bottom) = previous_y.map_or((y, y), |last| (last.min(y), last.max(y)));
            for line_y in top..=(bottom + 1).min(height - 1) {
                blend(&mut pixels, x, line_y, [r, g, b, 255]);
            }
            previous_y = Some(y);
        }
    }
    pixels
}

fn blend(pixels: &mut [u8], x: usize, y: usize, source: [u8; 4]) {
    let index = (y * WIDTH as usize + x) * 4;
    let Some(target) = pixels.get_mut(index..index + 4) else {
        return;
    };
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

    #[test]
    fn history_keeps_one_minute_and_rejects_bad_values() {
        let mut history = History::default();
        for value in 0..100 {
            history.push(value as f32);
        }
        history.push(f32::NAN);
        history.push(-4.0);
        assert_eq!(history.values.len(), SAMPLES);
        assert_eq!(history.values[SAMPLES - 1], 0.0);
        assert_eq!(history.peak(), 99.0);
    }

    #[test]
    fn full_value_draws_the_line_at_the_top_right() {
        let mut history = History::default();
        history.push(100.0);
        history.push(100.0);
        let pixels = render(
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
        let pixels = render(&[], 0.0);
        let grid_row = (HEIGHT as usize / 4) * WIDTH as usize * 4;
        assert_eq!(pixels[grid_row + 3], GRID[3]);
        assert!(pixels[..grid_row].iter().all(|byte| *byte == 0));
    }
}
