use super::*;

pub(super) fn local_wall_time() -> WallTime {
    let now = Local::now();
    WallTime {
        year: now.year(),
        month: now.month() as u8,
        day: now.day() as u8,
        weekday: now.weekday().num_days_from_sunday() as u8,
        hour: now.hour() as u8,
        minute: now.minute() as u8,
        second: now.second().min(59) as u8,
    }
}

pub(super) fn is_analog(widget: &Widget) -> bool {
    widget.kind == WidgetKind::Clock && widget.value("style") == CLOCK_STYLE_ANALOG
}

/// Hand positions as fractions of a full turn, clockwise from 12 o'clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct HandTurns {
    pub(super) hour: f32,
    pub(super) minute: f32,
    pub(super) second: f32,
}

pub(super) fn hand_turns(time: WallTime) -> HandTurns {
    let hour = f32::from(time.hour % 12);
    let minute = f32::from(time.minute);
    let second = f32::from(time.second);
    HandTurns {
        hour: (hour + minute / 60.0 + second / 3600.0) / 12.0,
        minute: (minute + second / 60.0) / 60.0,
        second: second / 60.0,
    }
}

/// Distance-based anti-aliased coverage of a capsule from `from` to `to`.
pub(super) fn segment_coverage(px: f32, py: f32, from: (f32, f32), to: (f32, f32), half: f32) -> f32 {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len_sq = (dx * dx + dy * dy).max(f32::EPSILON);
    let t = (((px - from.0) * dx + (py - from.1) * dy) / len_sq).clamp(0.0, 1.0);
    let (ex, ey) = (from.0 + dx * t - px, from.1 + dy * t - py);
    (half + 0.5 - (ex * ex + ey * ey).sqrt()).clamp(0.0, 1.0)
}

/// Composites `color` over a non-premultiplied RGBA pixel.
pub(super) fn over(pixel: &mut [f32; 4], color: Rgb, coverage: f32) {
    if coverage <= 0.0 {
        return;
    }
    let below = pixel[3] * (1.0 - coverage);
    let alpha = coverage + below;
    for (channel, value) in pixel[..3]
        .iter_mut()
        .zip([color.red, color.green, color.blue])
    {
        *channel = (f32::from(value) * coverage + *channel * below) / alpha;
    }
    pixel[3] = alpha;
}

/// RGBA8 analog face: 60 ticks (5-minute ticks heavier), hour/minute hands,
/// accent-colored second hand and hub. Numerals are HTML nodes, not pixels.
pub(crate) fn analog_face_pixels(size: u32, time: WallTime, hands: Rgb, accent: Rgb) -> Vec<u8> {
    let size = size.max(1);
    let side = size as f32;
    let center = side / 2.0;
    let radius = center - 1.0;
    let point = |turn: f32, r: f32| {
        let angle = turn * std::f32::consts::TAU;
        (center + r * angle.sin(), center - r * angle.cos())
    };
    let turns = hand_turns(time);
    let hour_half = (side * 0.012).max(0.6);
    let minute_half = (side * 0.005).max(0.5);
    let hand_specs = [
        (hands, turns.hour, -0.12, 0.5, side * 0.028),
        (hands, turns.minute, -0.12, 0.78, side * 0.018),
        (accent, turns.second, -0.25, 0.9, (side * 0.006).max(0.6)),
    ];
    let mut pixels = vec![[0.0f32; 4]; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let pixel = &mut pixels[(y * size + x) as usize];
            if (px - center).hypot(py - center) >= radius * 0.82 - 1.0 {
                for minute in 0..60 {
                    let turn = minute as f32 / 60.0;
                    let (inner, half) = if minute % 5 == 0 {
                        (radius * 0.84, hour_half)
                    } else {
                        (radius * 0.95, minute_half)
                    };
                    let coverage =
                        segment_coverage(px, py, point(turn, inner), point(turn, radius), half);
                    over(pixel, hands, coverage);
                }
            }
            for (color, turn, from, to, half) in hand_specs {
                let coverage = segment_coverage(
                    px,
                    py,
                    point(turn, radius * from),
                    point(turn, radius * to),
                    half,
                );
                over(pixel, color, coverage);
            }
            let hub = segment_coverage(px, py, (center, center), (center, center), side * 0.035);
            over(pixel, hands, hub);
        }
    }
    pixels
        .iter()
        .flat_map(|pixel| {
            [
                pixel[0].round() as u8,
                pixel[1].round() as u8,
                pixel[2].round() as u8,
                (pixel[3] * 255.0).round() as u8,
            ]
        })
        .collect()
}

pub(super) fn color(rgb: Rgb, alpha: u8) -> UiColor {
    UiColor {
        r: rgb.red,
        g: rgb.green,
        b: rgb.blue,
        a: alpha,
    }
}

pub(super) fn percent_to_alpha(percent: i64) -> u8 {
    ((percent.clamp(0, 100) * 255 + 50) / 100) as u8
}


pub(crate) fn ring_pixels(
    size: u32,
    thickness: f32,
    percent: u8,
    ring: Rgb,
    track: Rgb,
) -> Vec<u8> {
    let size = size.max(1);
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let center = size as f32 / 2.0;
    let half = (thickness / 2.0).clamp(0.5, center);
    let radius = center - half - 0.5;
    let fraction = f32::from(percent.min(100)) / 100.0;
    let cap = |angle: f32| {
        let theta = angle * std::f32::consts::TAU;
        (center + radius * theta.sin(), center - radius * theta.cos())
    };
    let (start_x, start_y) = cap(0.0);
    let (end_x, end_y) = cap(fraction);
    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = px - center;
            let dy = py - center;
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage = (half + 0.5 - (distance - radius).abs()).clamp(0.0, 1.0);
            let near = |cx: f32, cy: f32| {
                let ex = px - cx;
                let ey = py - cy;
                (half + 0.5 - (ex * ex + ey * ey).sqrt()).clamp(0.0, 1.0)
            };
            let mut angle = dx.atan2(-dy) / std::f32::consts::TAU;
            if angle < 0.0 {
                angle += 1.0;
            }
            let on_arc = fraction > 0.0 && angle <= fraction;
            let cap_coverage = if fraction > 0.0 {
                near(start_x, start_y).max(near(end_x, end_y))
            } else {
                0.0
            };
            let arc_coverage = if on_arc { coverage } else { 0.0 }.max(cap_coverage);
            let track_coverage = coverage * (1.0 - arc_coverage);
            let alpha = arc_coverage + track_coverage * 0.55;
            if alpha <= 0.0 {
                continue;
            }
            let mix = |a: u8, b: u8| {
                let total = f32::from(a) * arc_coverage + f32::from(b) * track_coverage * 0.55;
                (total / alpha).round().clamp(0.0, 255.0) as u8
            };
            let index = ((y * size + x) * 4) as usize;
            pixels[index] = mix(ring.red, track.red);
            pixels[index + 1] = mix(ring.green, track.green);
            pixels[index + 2] = mix(ring.blue, track.blue);
            pixels[index + 3] = (alpha.min(1.0) * 255.0).round() as u8;
        }
    }
    pixels
}
