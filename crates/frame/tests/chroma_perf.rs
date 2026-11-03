use std::{hint::black_box, sync::Arc, time::Instant};

use filmcraft_color::ColorInfo;
use filmcraft_frame::chroma::ChromaFilter;
use filmcraft_frame::{Chroma, PixelData, VideoFrame};

#[test]
#[ignore = "1080p CPU conversion timing; run with --release --ignored --nocapture"]
fn chroma_1080p_conversion() {
    let width = 1920;
    let height = 1080;
    let frame = VideoFrame {
        width,
        height,
        data: PixelData::Yuv8 {
            planes: [
                Arc::new((0..width * height).map(|index| (16 + index % 220) as u8).collect()),
                Arc::new((0..width * height / 4).map(|index| (16 + index % 225) as u8).collect()),
                Arc::new((0..width * height / 4).map(|index| (240 - index % 225) as u8).collect()),
            ],
            chroma: Chroma::C420,
            alpha: None,
        },
        color: ColorInfo::REC709,
        par: (1, 1),
        pts: Default::default(),
    };
    let expected = frame.to_opaque_srgb8(1).unwrap();
    let linear = frame.to_linear_f32();
    for (pixel, sample) in expected.as_chunks::<4>().0.iter().zip(linear.as_chunks::<4>().0) {
        assert_eq!(
            pixel,
            &[filmcraft_color::linear_to_srgb_u8(sample[0]), filmcraft_color::linear_to_srgb_u8(sample[1]), filmcraft_color::linear_to_srgb_u8(sample[2]), 255]
        );
    }
    for _ in 0..5 {
        black_box(frame.to_opaque_srgb8(1));
    }
    let mut timings = Vec::new();
    let mut nearest_timings = Vec::new();
    for _ in 0..21 {
        let start = Instant::now();
        black_box(frame.to_opaque_srgb8(1));
        timings.push(start.elapsed().as_secs_f64() * 1000.0);
        let start = Instant::now();
        black_box(frame.to_opaque_srgb8_with_filter(1, ChromaFilter::Nearest));
        nearest_timings.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    timings.sort_by(f64::total_cmp);
    nearest_timings.sort_by(f64::total_cmp);
    eprintln!("1080p YUV420 -> RGBA8: median {:.3} ms, min {:.3} ms; {} Rayon threads", timings[10], timings[0], rayon::current_num_threads());
    eprintln!("Nearest comparison: median {:.3} ms, min {:.3} ms", nearest_timings[10], nearest_timings[0]);
}
