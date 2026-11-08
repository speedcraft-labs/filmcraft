use std::{
    env, fs,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
};

use craft_bench::{Config, Paired, report::Revs};
use filmcraft_color::{ChromaSiting, ColorInfo, Matrix, Range};
use filmcraft_frame::{Chroma, PixelData, VideoFrame};
use rayon::prelude::*;

struct Input {
    frame: VideoFrame,
    swscale: Arc<Vec<u8>>,
}

struct Output {
    rgba: Vec<u8>,
    swscale: Arc<Vec<u8>>,
}

fn matrix_name(matrix: Matrix) -> &'static str {
    match matrix {
        Matrix::Bt601 => "bt601",
        Matrix::Bt709 => "bt709",
        Matrix::Bt2020Ncl => "bt2020",
    }
}

fn raw_yuv420(frame: &VideoFrame) -> Vec<u8> {
    let PixelData::Yuv8 { planes, chroma: Chroma::C420, .. } = &frame.data else { panic!("benchmark input must be 8-bit 4:2:0") };
    let mut raw = Vec::with_capacity(planes.iter().map(|p| p.len()).sum());
    for plane in planes {
        raw.extend_from_slice(plane);
    }
    raw
}

fn swscale(frame: &VideoFrame) -> Vec<u8> {
    let width = frame.width;
    let height = frame.height;
    let matrix = matrix_name(frame.color.matrix);
    let range = if frame.color.range == Range::Full { "pc" } else { "tv" };
    let location = match frame.color.chroma_siting {
        ChromaSiting::Left => 1,
        ChromaSiting::Center => 2,
        ChromaSiting::TopLeft => 3,
        ChromaSiting::Top => 4,
        ChromaSiting::BottomLeft => 5,
        ChromaSiting::Bottom => 6,
    };
    let filter = format!(
        "scale={width}:{height}:flags=bilinear+accurate_rnd+full_chroma_int:in_chroma_loc={location}:in_color_matrix={matrix}:out_color_matrix={matrix}"
    );
    let mut child = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-threads",
            "1",
            "-f",
            "rawvideo",
            "-pixel_format",
            "yuv420p",
            "-video_size",
            &format!("{width}x{height}"),
            "-color_range",
            range,
            "-i",
            "pipe:0",
            "-frames:v",
            "1",
            "-vf",
            &filter,
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
        .expect("start ffmpeg swscale oracle");
    use std::io::Write;
    child.stdin.take().expect("ffmpeg stdin").write_all(&raw_yuv420(frame)).expect("write YUV oracle input");
    let result = child.wait_with_output().expect("wait for ffmpeg swscale oracle");
    assert!(result.status.success(), "ffmpeg failed: {}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(result.stdout.len(), width as usize * height as usize * 4);
    result.stdout
}

fn file_frame(path: &Path) -> VideoFrame {
    let dimensions = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "default=noprint_wrappers=1"])
        .arg(path)
        .output()
        .expect("start ffprobe");
    assert!(dimensions.status.success(), "ffprobe failed");
    let dimensions = String::from_utf8(dimensions.stdout).expect("ffprobe dimensions");
    let dimension = |name: &str| {
        dimensions
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name}=")))
            .expect("ffprobe dimension")
            .parse::<usize>()
            .expect("numeric ffprobe dimension")
    };
    let (width, height) = (dimension("width"), dimension("height"));
    let decoded = Command::new("ffmpeg")
        .args(["-v", "error", "-threads", "1", "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-frames:v", "1", "-pix_fmt", "yuv420p", "-f", "rawvideo", "pipe:1"])
        .output()
        .expect("start ffmpeg corpus decode");
    assert!(decoded.status.success(), "ffmpeg decode failed: {}", String::from_utf8_lossy(&decoded.stderr));
    let y = width * height;
    let c = width.div_ceil(2) * height.div_ceil(2);
    assert_eq!(decoded.stdout.len(), y + 2 * c);
    let bytes = decoded.stdout;
    let matrix = filmcraft_frame::default_matrix(width as u32, height as u32);
    VideoFrame {
        width: width as u32,
        height: height as u32,
        data: PixelData::Yuv8 {
            planes: [Arc::new(bytes[..y].to_vec()), Arc::new(bytes[y..y + c].to_vec()), Arc::new(bytes[y + c..].to_vec())],
            chroma: Chroma::C420,
            alpha: None,
        },
        // Swscale matches this unsignalled VP9 sample when interpreted as top-left.
        color: ColorInfo { matrix, chroma_siting: ChromaSiting::TopLeft, ..ColorInfo::REC709 },
        par: (1, 1),
        pts: Default::default(),
    }
}

fn synthetic_1080p() -> VideoFrame {
    let (width, height) = (1920usize, 1080usize);
    let y = (0..width * height).map(|i| (16 + (i * 17 / 13) % 220) as u8).collect();
    let u = (0..width * height / 4).map(|i| (16 + (i * 29 / 7) % 225) as u8).collect();
    let v = (0..width * height / 4).map(|i| (240 - (i * 31 / 11) % 225) as u8).collect();
    VideoFrame {
        width: width as u32,
        height: height as u32,
        data: PixelData::Yuv8 { planes: [Arc::new(y), Arc::new(u), Arc::new(v)], chroma: Chroma::C420, alpha: None },
        color: ColorInfo { chroma_siting: ChromaSiting::Center, ..ColorInfo::REC709 },
        par: (1, 1),
        pts: Default::default(),
    }
}

fn legacy_nearest(frame: &VideoFrame) -> Vec<u8> {
    let PixelData::Yuv8 { planes, chroma: Chroma::C420, .. } = &frame.data else { panic!("benchmark input must be 8-bit 4:2:0") };
    let (w, h) = (frame.width as usize, frame.height as usize);
    let (kr, kb) = frame.color.matrix.kr_kb();
    let kg = 1.0 - kr - kb;
    let (ys, yo, cs) = if frame.color.range == Range::Limited { (255.0 / 219.0, 16.0, 255.0 / 224.0) } else { (1.0, 0.0, 1.0) };
    let (crr, cbb) = (2.0 * (1.0 - kr) * cs, 2.0 * (1.0 - kb) * cs);
    let (cgr, cgb) = (crr * kr / kg, cbb * kb / kg);
    let mut out = vec![0; w * h * 4];
    out.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let cy = y / 2;
        let yrow = &planes[0][y * w..(y + 1) * w];
        let urow = &planes[1][cy * w.div_ceil(2)..(cy + 1) * w.div_ceil(2)];
        let vrow = &planes[2][cy * w.div_ceil(2)..(cy + 1) * w.div_ceil(2)];
        for x in 0..w {
            let yy = (f32::from(yrow[x]) - yo) * ys;
            let (u, v) = (f32::from(urow[x / 2]) - 128.0, f32::from(vrow[x / 2]) - 128.0);
            let o = &mut row[x * 4..x * 4 + 4];
            o[0] = (yy + crr * v).round().clamp(0.0, 255.0) as u8;
            o[1] = (yy - cgr * v - cgb * u).round().clamp(0.0, 255.0) as u8;
            o[2] = (yy + cbb * u).round().clamp(0.0, 255.0) as u8;
            o[3] = 255;
        }
    });
    out
}

#[test]
fn pinned_vp9_frame_matches_swscale() {
    let Ok(root) = env::var("ARTCRAFT_HYPER_ROOT") else {
        eprintln!("SKIPPED: ARTCRAFT_HYPER_ROOT is unavailable");
        return;
    };
    let path = Path::new(&root).join("corpus/conformance-vp9/vp90-2-00-quantizer-18.webm");
    if !path.is_file() {
        eprintln!("SKIPPED: pinned VP9 corpus sample is unavailable");
        return;
    }
    let frame = file_frame(&path);
    let expected = swscale(&frame);
    let actual = frame.to_rgba8();
    let max_delta = actual.iter().zip(&expected).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
    assert!(max_delta <= 2);
}

#[test]
#[ignore = "paired old-vs-new chroma conversion; run with --release --ignored --nocapture under scripts/bench-run"]
fn paired_chroma_upsampling() {
    let root = env::var("ARTCRAFT_HYPER_ROOT").expect("ARTCRAFT_HYPER_ROOT");
    let corpus = Path::new(&root).join("corpus/conformance-vp9/vp90-2-00-quantizer-18.webm");
    let mut cases = Vec::new();
    for (name, frame) in [("vp9-conformance-352x288", file_frame(&corpus)), ("synthetic-1080p", synthetic_1080p())] {
        cases.push((name, Input { swscale: Arc::new(swscale(&frame)), frame }));
    }
    let report = Paired::new("filmcraft 4:2:0 YUV to RGBA, nearest vs centered bilinear")
        .old(|input: &Input| Output { rgba: legacy_nearest(&input.frame), swscale: Arc::clone(&input.swscale) })
        .new(|input: &Input| Output { rgba: input.frame.to_rgba8(), swscale: Arc::clone(&input.swscale) })
        .inputs(cases)
        .check(|old, new| {
            old.rgba.len() == old.swscale.len()
                && new.rgba.len() == new.swscale.len()
                && new.rgba.iter().zip(new.swscale.iter()).all(|(a, b)| a.abs_diff(*b) <= 2)
        })
        .run(&Config {
            repetitions: 20,
            warmup: 3,
            resamples: 2000,
            revs: Revs { old: env::var("CHROMA_BASE_REV").ok(), new: env::var("CHROMA_HEAD_REV").ok() },
            ..Config::default()
        })
        .expect("run paired chroma benchmark");
    println!("{}", report.to_markdown());
    report.require_publishable().expect("publishable paired benchmark");
    let output = env::var("CHROMA_BENCH_OUT").expect("CHROMA_BENCH_OUT");
    fs::write(output, report.to_json()).expect("write craft-bench report");
}
