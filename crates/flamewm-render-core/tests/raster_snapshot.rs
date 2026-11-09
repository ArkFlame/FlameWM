//! Bit-exact snapshot of `SoftPixels::paint` batches on a deterministic frame.
//!
//! Digests were captured from the copy-based batch path before the in-place fast path.

use flamewm_render_core::{PixelOp, RoundedRect, SoftPixels};

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn frame(width: u32, height: u32, seed: u32) -> Vec<u8> {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..(width as usize * height as usize * 4))
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect()
}

fn source(width: u32, height: u32, seed: u32) -> Vec<u8> {
    frame(width, height, seed)
}

fn run(width: u32, height: u32, ops: &[PixelOp<'_>]) -> u64 {
    let mut pixels = frame(width, height, 21);
    SoftPixels::new()
        .paint(&mut pixels, width, height, ops)
        .expect("valid batch");
    fnv1a(&pixels)
}

fn cases() -> Vec<(&'static str, u64)> {
    let mask = frame(37, 29, 4)
        .iter()
        .step_by(4)
        .copied()
        .collect::<Vec<u8>>();
    let image = source(64, 64, 8);
    let rounded = RoundedRect {
        x: 6,
        y: 4,
        width: 80,
        height: 50,
        radius: 14,
    };
    let stroke = RoundedRect {
        x: 9,
        y: 7,
        width: 74,
        height: 44,
        radius: 11,
    };
    vec![
        (
            "fill_rounded_alpha",
            run(
                97,
                61,
                &[PixelOp::FillRoundedRect {
                    outer: rounded,
                    hole: None,
                    color: [200, 100, 50, 140],
                }],
            ),
        ),
        (
            "stroke_ring",
            run(
                97,
                61,
                &[PixelOp::FillRoundedRect {
                    outer: rounded,
                    hole: Some(stroke),
                    color: [10, 240, 90, 255],
                }],
            ),
        ),
        (
            "blend_mask_partly_outside",
            run(
                97,
                61,
                &[PixelOp::BlendMask {
                    x: -5,
                    y: 3,
                    width: 37,
                    height: 29,
                    mask: &mask,
                    color: [255, 255, 255, 200],
                }],
            ),
        ),
        (
            "blend_image_partly_outside",
            run(
                97,
                61,
                &[PixelOp::BlendImage {
                    x: 60,
                    y: 40,
                    width: 64,
                    height: 64,
                    rgba: &image,
                }],
            ),
        ),
        (
            "mixed_batch_with_blur",
            run(
                97,
                61,
                &[
                    PixelOp::FillRoundedRect {
                        outer: rounded,
                        hole: None,
                        color: [1, 2, 3, 255],
                    },
                    PixelOp::BlendImage {
                        x: 20,
                        y: 10,
                        width: 64,
                        height: 64,
                        rgba: &image,
                    },
                    PixelOp::Blur { radius: 3.0 },
                    PixelOp::BlendMask {
                        x: 0,
                        y: 0,
                        width: 37,
                        height: 29,
                        mask: &mask,
                        color: [9, 8, 7, 128],
                    },
                ],
            ),
        ),
        (
            "mixed_batch_no_blur",
            run(
                97,
                61,
                &[
                    PixelOp::FillRoundedRect {
                        outer: rounded,
                        hole: Some(stroke),
                        color: [40, 80, 160, 90],
                    },
                    PixelOp::BlendImage {
                        x: 20,
                        y: 10,
                        width: 64,
                        height: 64,
                        rgba: &image,
                    },
                    PixelOp::BlendMask {
                        x: -3,
                        y: 50,
                        width: 37,
                        height: 29,
                        mask: &mask,
                        color: [9, 8, 7, 128],
                    },
                ],
            ),
        ),
    ]
}

#[test]
fn paint_batches_match_pinned_snapshot() {
    #[allow(clippy::unreadable_literal)]
    let expected: &[(&str, u64)] = &[
        ("fill_rounded_alpha", 0x844387e1651a53f9),
        ("stroke_ring", 0xffe15cbcc9cec601),
        ("blend_mask_partly_outside", 0x840be9497da49cc1),
        ("blend_image_partly_outside", 0xddbbe4844b454ceb),
        ("mixed_batch_with_blur", 0x345d7a92e134d31b),
        ("mixed_batch_no_blur", 0x71d69374dde0c108),
    ];
    let actual = cases();
    assert_eq!(actual.len(), expected.len(), "snapshot case count");
    let mut changed = Vec::new();
    for ((name, digest), (exp_name, exp_digest)) in actual.iter().zip(expected) {
        assert_eq!(name, exp_name);
        if digest != exp_digest {
            changed.push(format!(
                "{name}: got {digest:#018x} want {exp_digest:#018x}"
            ));
        }
    }
    assert!(
        changed.is_empty(),
        "snapshot changed:\n{}",
        changed.join("\n")
    );
}
