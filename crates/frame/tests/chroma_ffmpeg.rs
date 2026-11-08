use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

use filmcraft_color::{ChromaSiting, ColorInfo};
use filmcraft_frame::{Chroma, PixelData, VideoFrame};

#[test]
fn centered_420_matches_ffmpeg_swscale_bilinear() {
    let width = 8usize;
    let height = 8usize;
    let luma = vec![96; width * height];
    let u = [32, 80, 176, 224].repeat(4);
    let v = [224, 176, 80, 32].repeat(4);
    let mut raw = luma.clone();
    raw.extend_from_slice(&u);
    raw.extend_from_slice(&v);
    let mut child = match Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "rawvideo",
            "-pixel_format",
            "yuv420p",
            "-video_size",
            "8x8",
            "-color_range",
            "tv",
            "-colorspace",
            "bt709",
            "-i",
            "pipe:0",
            "-frames:v",
            "1",
            "-vf",
            "scale=8:8:flags=bilinear+accurate_rnd+full_chroma_int:in_h_chr_pos=128:in_v_chr_pos=128:in_color_matrix=bt709:out_color_matrix=bt709",
            "-pix_fmt",
            "rgba",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("SKIPPED: ffmpeg is unavailable");
            return;
        }
        Err(error) => panic!("could not start ffmpeg: {error}"),
    };
    child.stdin.take().unwrap().write_all(&raw).unwrap();
    let oracle = child.wait_with_output().unwrap();
    assert!(oracle.status.success(), "ffmpeg failed: {}", String::from_utf8_lossy(&oracle.stderr));
    assert_eq!(oracle.stdout.len(), width * height * 4);

    let frame = VideoFrame {
        width: width as u32,
        height: height as u32,
        data: PixelData::Yuv8 { planes: [Arc::new(luma), Arc::new(u), Arc::new(v)], chroma: Chroma::C420, alpha: None },
        color: ColorInfo { chroma_siting: ChromaSiting::Center, ..ColorInfo::REC709 },
        par: (1, 1),
        pts: Default::default(),
    };
    let actual = frame.to_rgba8();
    for (index, (actual, expected)) in actual.iter().zip(&oracle.stdout).enumerate() {
        assert!(actual.abs_diff(*expected) <= 2, "channel {index}: FilmCraft {actual}, ffmpeg {expected}");
    }
}
