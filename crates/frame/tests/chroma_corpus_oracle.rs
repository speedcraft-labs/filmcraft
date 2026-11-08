use std::{
    env,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
};

use filmcraft_color::{ChromaSiting, ColorInfo, Matrix, Range};
use filmcraft_frame::{Chroma, PixelData, VideoFrame};

fn matrix_name(matrix: Matrix) -> &'static str {
    match matrix {
        Matrix::Bt601 => "bt601",
        Matrix::Bt709 => "bt709",
        Matrix::Bt2020Ncl => "bt2020",
    }
}

fn raw_yuv420(frame: &VideoFrame) -> Vec<u8> {
    let PixelData::Yuv8 { planes, chroma: Chroma::C420, .. } = &frame.data else { panic!("oracle input must be 8-bit 4:2:0") };
    let mut raw = Vec::with_capacity(planes.iter().map(|plane| plane.len()).sum());
    for plane in planes {
        raw.extend_from_slice(plane);
    }
    raw
}

fn swscale(frame: &VideoFrame) -> Vec<u8> {
    let (width, height) = (frame.width, frame.height);
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
            .find_map(|line| line.strip_prefix(&format!("{name}=")).and_then(|value| value.parse::<usize>().ok()))
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

#[test]
fn pinned_vp9_frame_matches_swscale() {
    let Ok(root) = env::var("ARTCRAFT_HYPER_ROOT") else {
        eprintln!("SKIPPED: ARTCRAFT_HYPER_ROOT is unavailable");
        return;
    };
    let path = Path::new(&root).join("corpus/conformance-vp9/vp90-2-00-quantizer-18.webm");
    if !path.is_file() || Command::new("ffmpeg").arg("-version").output().is_err() || Command::new("ffprobe").arg("-version").output().is_err() {
        eprintln!("SKIPPED: pinned VP9 sample or FFmpeg tools are unavailable");
        return;
    }
    let frame = file_frame(&path);
    let oracle = swscale(&frame);
    let actual = frame.to_rgba8();
    let max_delta = actual.iter().zip(&oracle).map(|(actual, expected)| actual.abs_diff(*expected)).max().unwrap_or(0);
    assert!(max_delta <= 2, "pinned VP9 max channel delta {max_delta}");
}
