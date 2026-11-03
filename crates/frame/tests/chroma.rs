use std::sync::Arc;

use filmcraft_color::{ChromaSiting, ColorInfo};
use filmcraft_frame::{
    Chroma, PixelData, Region, VideoFrame,
    chroma::{ChromaFilter, ChromaUpsampler},
};
use proptest::prelude::*;

const SITINGS: [ChromaSiting; 6] =
    [ChromaSiting::Left, ChromaSiting::Center, ChromaSiting::TopLeft, ChromaSiting::Top, ChromaSiting::BottomLeft, ChromaSiting::Bottom];

proptest! {
    #[test]
    fn constant_chroma_is_exact(value in any::<u16>(), width in 1usize..33, height in 1usize..25) {
        for chroma in [Chroma::C420, Chroma::C422, Chroma::C444] {
            let (horizontal, vertical) = chroma.shifts();
            let plane = vec![value; width.div_ceil(1 << horizontal) * height.div_ceil(1 << vertical)];
            for siting in SITINGS {
                let sampler = ChromaUpsampler::new([&plane, &plane], (width, height), chroma, siting, ChromaFilter::Bilinear).unwrap();
                for row in 0..height {
                    for column in 0..width {
                        prop_assert_eq!(sampler.row(row).sample(column), [u32::from(value); 2]);
                    }
                }
            }
        }
    }

    #[test]
    fn full_chroma_is_identity(plane in prop::collection::vec(any::<u16>(), 35)) {
        for siting in SITINGS {
            let sampler = ChromaUpsampler::new([&plane, &plane], (7, 5), Chroma::C444, siting, ChromaFilter::Bilinear).unwrap();
            for row in 0..5 {
                for column in 0..7 {
                    prop_assert_eq!(sampler.row(row).sample(column), [u32::from(plane[row * 7 + column]); 2]);
                }
            }
        }
    }
}

#[test]
fn bilinear_reproduces_a_two_dimensional_ramp_within_one_lsb() {
    for chroma in [Chroma::C420, Chroma::C422] {
        let (_, vertical) = chroma.shifts();
        let (width, height) = (13usize, 9usize);
        let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(1 << vertical));
        let plane: Vec<u8> = (0..chroma_width * chroma_height).map(|index| (17 + 11 * (index % chroma_width) + 7 * (index / chroma_width)) as u8).collect();
        for siting in SITINGS {
            let sampler = ChromaUpsampler::new([&plane, &plane], (width, height), chroma, siting, ChromaFilter::Bilinear).unwrap();
            let (phase_x, phase_y) = siting.half_sample_offsets();
            for row in 0..height {
                for column in 0..width {
                    let source_x = ((column as f64 - f64::from(phase_x) / 2.0) / 2.0).clamp(0.0, (chroma_width - 1) as f64);
                    let source_y = ((row as f64 - if vertical == 0 { 0.0 } else { f64::from(phase_y) / 2.0 }) / f64::from(1 << vertical))
                        .clamp(0.0, (chroma_height - 1) as f64);
                    let expected = 17.0 + source_x * 11.0 + source_y * 7.0;
                    assert!((f64::from(sampler.row(row).sample(column)[0]) - expected).abs() <= 0.5, "{chroma:?} {siting:?} at {column},{row}");
                }
            }
        }
    }
}

#[test]
fn centered_step_is_shifted_half_a_luma_sample() {
    let plane = [0u8, 0, 200, 200];
    for chroma in [Chroma::C420, Chroma::C422] {
        let edge = |siting| {
            let sampler = ChromaUpsampler::new([&plane, &plane], (8, 1), chroma, siting, ChromaFilter::Bilinear).unwrap();
            (0..8).map(|column| sampler.row(0).sample(column)[0]).collect::<Vec<_>>()
        };
        assert_eq!(edge(ChromaSiting::Left), [0, 0, 0, 100, 200, 200, 200, 200]);
        assert_eq!(edge(ChromaSiting::Center), [0, 0, 0, 50, 150, 200, 200, 200]);
    }
    let edge = |siting| {
        let sampler = ChromaUpsampler::new([&plane, &plane], (1, 8), Chroma::C420, siting, ChromaFilter::Bilinear).unwrap();
        (0..8).map(|row| sampler.row(row).sample(0)[0]).collect::<Vec<_>>()
    };
    assert_eq!(edge(ChromaSiting::TopLeft), [0, 0, 0, 100, 200, 200, 200, 200]);
    assert_eq!(edge(ChromaSiting::Left), [0, 0, 0, 50, 150, 200, 200, 200]);
}

#[test]
fn nearest_keeps_legacy_block_replication() {
    let plane = [10u8, 50, 100, 200];
    let sampler = ChromaUpsampler::new([&plane, &plane], (4, 4), Chroma::C420, ChromaSiting::Center, ChromaFilter::Nearest).unwrap();
    for row in 0..4 {
        for column in 0..4 {
            assert_eq!(sampler.row(row).sample(column), [u32::from(plane[row / 2 * 2 + column / 2]); 2]);
        }
    }
}

#[test]
fn invalid_planes_and_dimensions_are_rejected() {
    for size in [(0, 0), (0, 1), (1, 0), (usize::MAX, 2), (1 << 29, 1), (5, 5)] {
        assert!(ChromaUpsampler::new([&[0u8], &[0u8]], size, Chroma::C420, ChromaSiting::Left, ChromaFilter::Bilinear).is_none());
    }
    let plane = [65535u16];
    let sampler = ChromaUpsampler::new([&plane, &plane], (1, 1), Chroma::C420, ChromaSiting::Center, ChromaFilter::Bilinear).unwrap();
    assert_eq!(sampler.row(usize::MAX).sample(usize::MAX), [65535; 2]);
}

#[test]
fn oversized_coordinates_clamp_to_the_last_luma_pixel() {
    let plane = [0u8, 10, 20, 30, 40, 50, 60, 70, 80];
    let sampler = ChromaUpsampler::new([&plane, &plane], (5, 5), Chroma::C420, ChromaSiting::Center, ChromaFilter::Bilinear).unwrap();
    assert_eq!(sampler.row(usize::MAX).sample(usize::MAX), sampler.row(4).sample(4));
}

#[test]
fn conversion_paths_match_explicit_full_resolution_planes() {
    for chroma in [Chroma::C420, Chroma::C422, Chroma::C444] {
        for siting in [ChromaSiting::Center, ChromaSiting::Left] {
            for bits in [8, 10, 12, 16] {
                let (width, height) = (9usize, 7usize);
                let (horizontal, vertical) = chroma.shifts();
                let count = width.div_ceil(1 << horizontal) * height.div_ceil(1 << vertical);
                let luma = vec![100u16 << (bits - 8); width * height];
                let plane: Vec<u16> = (0..count).map(|index| (32 + (index % 32) as u16 * 5) << (bits - 8)).collect();
                let sampler = ChromaUpsampler::new([&plane, &plane], (width, height), chroma, siting, ChromaFilter::Bilinear).unwrap();
                let expanded: Vec<u16> = (0..height)
                    .flat_map(|row| (0..width).map(move |column| (row, column)))
                    .map(|(row, column)| sampler.row(row).sample(column)[0] as u16)
                    .collect();
                for has_alpha in [false, true] {
                    let frame = |plane: Vec<u16>, chroma| VideoFrame {
                        width: width as u32,
                        height: height as u32,
                        data: if bits == 8 {
                            let to8 = |values: Vec<u16>| Arc::new(values.into_iter().map(|value| value as u8).collect());
                            PixelData::Yuv8 {
                                planes: [to8(luma.clone()), to8(plane.clone()), to8(plane)],
                                chroma,
                                alpha: has_alpha.then(|| Arc::new(vec![191; width * height])),
                            }
                        } else {
                            PixelData::Yuv16 {
                                planes: [Arc::new(luma.clone()), Arc::new(plane.clone()), Arc::new(plane)],
                                chroma,
                                bits,
                                alpha: has_alpha.then(|| Arc::new(vec![191 << (bits - 8); width * height])),
                            }
                        },
                        color: ColorInfo { chroma_siting: siting, ..ColorInfo::REC709 },
                        par: (1, 1),
                        pts: Default::default(),
                    };
                    let source = frame(plane.clone(), chroma);
                    let reference = frame(expanded.clone(), Chroma::C444);
                    assert_eq!(source.to_rgba8(), reference.to_rgba8());
                    for decimation in [1, 2, 3] {
                        assert_eq!(source.to_linear_f32_decimated(decimation), reference.to_linear_f32_decimated(decimation));
                        assert_eq!(source.to_opaque_srgb8(decimation), reference.to_opaque_srgb8(decimation));
                        let region = Region { x: 1, y: 1, w: 3, h: 2 };
                        assert_eq!(source.to_linear_f32_region(decimation, None, region), reference.to_linear_f32_region(decimation, None, region));
                    }
                }
            }
        }
    }
}
