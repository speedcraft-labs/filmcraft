use super::{Frame, McBlock, Plane, RefPic, build_hpel};
use crate::dsp::satd;
use crate::mbinfo::Mv;

const TAPS: [i32; 6] = [1, -5, 20, 20, -5, 1];

fn sample(plane: &Plane, x: isize, y: isize) -> i32 {
    let x = x.clamp(0, plane.w as isize - 1);
    let y = y.clamp(0, plane.h as isize - 1);
    i32::from(plane.data[plane.idx(x, y)])
}

fn horizontal(plane: &Plane, x: isize, y: isize) -> i32 {
    TAPS.iter().enumerate().map(|(offset, tap)| tap * sample(plane, x + offset as isize - 2, y)).sum()
}

fn interpolate(plane: &Plane, x: isize, y: isize, fx: i16, fy: i16) -> u8 {
    let full = |dx, dy| sample(plane, x + dx, y + dy);
    let half_h = |dy| ((horizontal(plane, x, y + dy) + 16) >> 5).clamp(0, 255);
    let half_v = |dx| {
        let sum: i32 = TAPS.iter().enumerate().map(|(offset, tap)| tap * sample(plane, x + dx, y + offset as isize - 2)).sum();
        ((sum + 16) >> 5).clamp(0, 255)
    };
    let half_hv = || {
        let sum: i32 = TAPS.iter().enumerate().map(|(offset, tap)| tap * horizontal(plane, x, y + offset as isize - 2)).sum();
        ((sum + 512) >> 10).clamp(0, 255)
    };
    let avg = |first, second| (first + second + 1) >> 1;
    (match (fx, fy) {
        (0, 0) => full(0, 0),
        (1, 0) => avg(full(0, 0), half_h(0)),
        (2, 0) => half_h(0),
        (3, 0) => avg(half_h(0), full(1, 0)),
        (0, 1) => avg(full(0, 0), half_v(0)),
        (1, 1) => avg(half_h(0), half_v(0)),
        (2, 1) => avg(half_h(0), half_hv()),
        (3, 1) => avg(half_h(0), half_v(1)),
        (0, 2) => half_v(0),
        (1, 2) => avg(half_v(0), half_hv()),
        (2, 2) => half_hv(),
        (3, 2) => avg(half_hv(), half_v(1)),
        (0, 3) => avg(half_v(0), full(0, 1)),
        (1, 3) => avg(half_v(0), half_h(1)),
        (2, 3) => avg(half_hv(), half_h(1)),
        (3, 3) => avg(half_v(1), half_h(1)),
        _ => panic!("invalid test phase"),
    }) as u8
}

fn random(seed: &mut u64) -> u8 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (*seed >> 32) as u8
}

#[test]
fn cached_plane_costs_match_per_candidate_interpolation() {
    let mut seed = 111;
    for (width, height) in [(2, 2), (18, 22), (48, 40)] {
        let mut reuse = None;
        for pattern in 0..6 {
            let mut frame = Frame::new(width, height);
            for row in 0..height {
                for column in 0..width {
                    let value = match pattern {
                        0 | 1 => random(&mut seed),
                        2 => {
                            if (row / 2 + column / 2) % 2 == 0 {
                                0
                            } else {
                                255
                            }
                        }
                        3 => {
                            if row == height / 2 && column == width / 2 {
                                255
                            } else {
                                0
                            }
                        }
                        4 => 0,
                        _ => 255,
                    };
                    let offset = frame.y.idx(column as isize, row as isize);
                    frame.y.data[offset] = value;
                }
            }
            frame.y.extend_edges();
            let hpel = build_hpel(&frame.y, reuse.take());
            let reference = RefPic { frame, hpel, mbs: Vec::new(), poc: 0, id: 0 };
            let edge_x = width as isize;
            let edge_y = height as isize;
            for (x, y) in [
                (-16, -16),
                (-1, -1),
                (0, 0),
                (5, 7),
                (edge_x - 16, edge_y - 16),
                (edge_x - 1, edge_y - 1),
                (edge_x, edge_y),
                (edge_x + 16, edge_y + 16),
                (-1, edge_y - 1),
                (edge_x - 1, -1),
                (-1, edge_y / 2),
                (edge_x / 2, -1),
                (edge_x, edge_y / 2),
                (edge_x / 2, edge_y),
            ] {
                for block_width in [4, 8, 16] {
                    for block_height in [4, 8, 16] {
                        let block = McBlock { x: width / 2, y: height / 2, w: block_width, h: block_height };
                        let source: Vec<u8> = (0..((block_height - 1) * 23 + block_width + 1)).map(|_| random(&mut seed)).collect();
                        let source = &source[1..];
                        for fy in 0..4 {
                            for fx in 0..4 {
                                let mv = Mv::new((x - block.x as isize) as i32 * 4 + fx, (y - block.y as isize) as i32 * 4 + fy);
                                let mut expected = [0; 256];
                                for row in 0..block_height {
                                    for column in 0..block_width {
                                        expected[row * 16 + column] =
                                            interpolate(&reference.frame.y, x + column as isize, y + row as isize, fx as i16, fy as i16);
                                    }
                                }
                                let mut buffer = [0xa5; 256];
                                let (cached, stride) = reference.luma_ref(block.x, block.y, mv, block_width, block_height, &mut buffer);
                                for row in 0..block_height {
                                    assert_eq!(
                                        &cached[row * stride..row * stride + block_width],
                                        &expected[row * 16..row * 16 + block_width],
                                        "{width}x{height}, pattern {pattern}, ({x},{y}), phase ({fx},{fy})"
                                    );
                                }
                                let cost = satd(source, 23, &expected, 16, block_width, block_height);
                                assert_eq!(
                                    satd(source, 23, cached, stride, block_width, block_height),
                                    cost,
                                    "{block_width}x{block_height}, ({x},{y}), phase ({fx},{fy})"
                                );
                                if fx % 2 == 0 && fy % 2 == 0 {
                                    assert_eq!(stride, reference.frame.y.stride);
                                    assert_eq!(buffer, [0xa5; 256]);
                                }
                                reference.mc_luma(block, mv, &mut buffer, 16);
                                for row in 0..block_height {
                                    assert_eq!(&buffer[row * 16..row * 16 + block_width], &expected[row * 16..row * 16 + block_width]);
                                }
                            }
                        }
                    }
                }
            }
            reuse = Some(reference.hpel);
        }
    }
}

#[test]
#[ignore = "timing-only comparison with the deliberately simple per-candidate test oracle"]
fn benchmark_cached_plane_costs() {
    use std::hint::black_box;
    use std::time::Instant;

    let mut seed = 111;
    let mut frame = Frame::new(1920, 1088);
    for byte in &mut frame.y.data {
        *byte = random(&mut seed);
    }
    frame.y.extend_edges();
    let hpel = build_hpel(&frame.y, None);
    let reference = RefPic { frame, hpel, mbs: Vec::new(), poc: 0, id: 0 };
    let source: [u8; 256] = std::array::from_fn(|_| random(&mut seed));
    for (width, height) in [(8, 8), (8, 16), (16, 8), (16, 16)] {
        let block = McBlock { x: 500, y: 500, w: width, h: height };
        for _ in 0..5 {
            let mut sums = [0u64; 2];
            for (cached, sum) in sums.iter_mut().enumerate() {
                let start = Instant::now();
                let mut buffer = [0; 256];
                for _ in 0..200 {
                    for dy in -4..=4 {
                        for dx in -4..=4 {
                            let mv = black_box(Mv::new(dx, dy));
                            let block = black_box(block);
                            *sum += u64::from(if cached == 0 {
                                for row in 0..block.h {
                                    for column in 0..block.w {
                                        buffer[row * 16 + column] = interpolate(
                                            &reference.frame.y,
                                            (block.x + column) as isize + (mv.x as isize >> 2),
                                            (block.y + row) as isize + (mv.y as isize >> 2),
                                            mv.x & 3,
                                            mv.y & 3,
                                        );
                                    }
                                }
                                satd(black_box(&source), 16, &buffer, 16, block.w, block.h)
                            } else {
                                let (samples, stride) = reference.luma_ref(block.x, block.y, mv, block.w, block.h, &mut buffer);
                                satd(black_box(&source), 16, samples, stride, block.w, block.h)
                            });
                        }
                    }
                }
                println!("{width}x{height} cached={cached} {:?} sum={}", start.elapsed(), black_box(*sum));
            }
            assert_eq!(sums[0], sums[1]);
        }
    }
}
