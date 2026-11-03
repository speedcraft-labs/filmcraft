use super::*;

fn dense_synthesize(history: &mut [f32; 1024], matrix: &[[f32; 32]; 64], window: &[f32; 512], input: &[f32; 32], output: &mut [f32; 32]) {
    history.copy_within(0..960, 64);
    for (value, row) in history.iter_mut().zip(matrix) {
        *value = row.iter().zip(input).map(|(coefficient, sample)| coefficient * sample).sum();
    }
    for (sample, output) in output.iter_mut().enumerate() {
        let mut accumulated = 0.0_f32;
        for block in 0..8 {
            accumulated += history[block * 128 + sample] * window[block * 64 + sample] + history[block * 128 + 96 + sample] * window[block * 64 + 32 + sample];
        }
        *output = accumulated;
    }
}

fn random_input(seed: &mut u32) -> [f32; 32] {
    std::array::from_fn(|_| {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        (*seed as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32 / 32.0
    })
}

#[test]
fn ring_matches_dense_history_and_pcm() {
    let mut history = [0.0; 1024];
    let mut reference = [0.0; 1024];
    let mut offset = 0;
    let mut random = 0x1234_5678;
    let mut maximum = 0.0_f32;
    for slot in 0..4096 {
        let input = if slot < 32 {
            std::array::from_fn(|band| if band == slot { 1.0 } else { 0.0 })
        } else if slot >= 4064 {
            [0.0; 32]
        } else {
            random_input(&mut random)
        };
        let mut actual = [0.0; 32];
        let mut expected = [0.0; 32];
        synthesize(&mut history, &mut offset, tables::window(), &input, &mut actual);
        dense_synthesize(&mut reference, tables::matrix(), tables::window(), &input, &mut expected);
        for (index, value) in reference.iter().enumerate() {
            assert!((history[(offset + index) & 1023] - value).abs() <= 1e-6, "slot {slot}, history {index}");
        }
        for (actual, expected) in actual.iter().zip(expected) {
            maximum = maximum.max((actual - expected).abs());
        }
    }
    println!("synthesis random/impulse PCM max delta: {maximum:e}");
    assert!(maximum <= 1e-6, "{maximum:e}");
    assert!(history.iter().all(|sample| *sample == 0.0));
}

#[test]
fn synthesis_allocates_nothing() {
    let mut history = [0.0; 1024];
    let mut offset = 0;
    let input = [0.03125; 32];
    let mut output = [0.0; 32];
    let window = tables::window();
    let allocations = allocation_counter::measure(|| {
        for _ in 0..36 {
            synthesize(&mut history, &mut offset, window, std::hint::black_box(&input), &mut output);
            std::hint::black_box(output);
        }
    });
    assert_eq!(allocations.count_total, 0);
}

#[test]
fn reset_clears_ring_position_and_both_channels() {
    let mut decoder = Decoder::new();
    decoder.v.fill([1.0; 1024]);
    decoder.v_offset = [64, 960];
    decoder.reset();
    assert_eq!(decoder.v_offset, [0, 0]);
    assert!(decoder.v.iter().flatten().all(|sample| *sample == 0.0));
}

#[test]
#[ignore = "generate oracle fixtures first; checks every decoded sample against the previous synthesis"]
fn fixtures_match_dense_synthesis() {
    let mut paths: Vec<_> = std::fs::read_dir(filmcraft_testkit::fixtures_dir("mpa"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "mp1" || extension == "mp2"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "run the oracle tests to generate fixtures");
    let mut maximum = 0.0_f32;
    let mut sample_count = 0;
    for path in &paths {
        let data = std::fs::read(path).unwrap();
        let mut remaining = data.as_slice();
        let mut decoder = Decoder::new();
        let mut reference = [[0.0; 1024]; 2];
        let mut fixture_maximum = 0.0_f32;
        while !remaining.is_empty() {
            let frame = decoder.decode(remaining).unwrap();
            for (channel, samples) in frame.channels.iter().enumerate() {
                for (slot, actual) in samples.as_chunks::<32>().0.iter().enumerate() {
                    let mut expected = [0.0; 32];
                    dense_synthesize(&mut reference[channel], tables::matrix(), tables::window(), &decoder.sb[channel][slot], &mut expected);
                    for (actual, expected) in actual.iter().zip(expected) {
                        fixture_maximum = fixture_maximum.max((actual - expected).abs());
                        sample_count += 1;
                    }
                }
            }
            remaining = &remaining[frame.header.frame_bytes..];
        }
        println!("{}: max PCM delta {fixture_maximum:e}", path.display());
        maximum = maximum.max(fixture_maximum);
    }
    println!("{} fixtures, {sample_count} samples, max PCM delta {maximum:e}", paths.len());
    assert!(maximum <= 1e-6, "max PCM delta {maximum:e}");
}

#[test]
#[ignore = "release-mode synthesis benchmark"]
fn benchmark_synthesis() {
    let mut random = 0x1234_5678;
    let inputs: [[f32; 32]; 36] = std::array::from_fn(|_| random_input(&mut random));
    let mut fast_history = [0.0; 1024];
    let mut dense_history = [0.0; 1024];
    let mut offset = 0;
    let mut output = [0.0; 32];
    let window = tables::window();
    let matrix = tables::matrix();
    dct32::dct32(&[0.0; 32]);
    let mut fast_timings = [0.0; 9];
    let mut dense_timings = [0.0; 9];
    for round in 0..9 {
        for fast in [round % 2 == 0, round % 2 != 0] {
            let start = std::time::Instant::now();
            for _ in 0..1000 {
                for input in &inputs {
                    if fast {
                        synthesize(&mut fast_history, &mut offset, window, std::hint::black_box(input), &mut output);
                    } else {
                        dense_synthesize(&mut dense_history, matrix, window, std::hint::black_box(input), &mut output);
                    }
                    std::hint::black_box(output);
                }
            }
            let elapsed = start.elapsed().as_secs_f64() / 36_000.0;
            if fast {
                fast_timings[round] = elapsed;
            } else {
                dense_timings[round] = elapsed;
            }
        }
    }
    fast_timings.sort_by(f64::total_cmp);
    dense_timings.sort_by(f64::total_cmp);
    println!(
        "synthesis median: dense {:.1} ns/slot, fast {:.1} ns/slot, {:.2}x",
        dense_timings[4] * 1e9,
        fast_timings[4] * 1e9,
        dense_timings[4] / fast_timings[4]
    );
}
