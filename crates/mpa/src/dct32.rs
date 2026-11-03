use std::sync::OnceLock;

fn factors() -> &'static [f64; 31] {
    static FACTORS: OnceLock<[f64; 31]> = OnceLock::new();
    FACTORS.get_or_init(|| {
        let mut factors = [0.0; 31];
        for half in [1, 2, 4, 8, 16] {
            for index in 0..half {
                factors[half - 1 + index] = 0.5 / ((2 * index + 1) as f64 * std::f64::consts::PI / (4 * half) as f64).cos();
            }
        }
        factors
    })
}

macro_rules! stage {
    ($name:ident, $size:literal, $half:literal, $child:ident) => {
        #[inline(always)]
        fn $name(input: &[f64; $size], factors: &[f64; 31]) -> [f64; $size] {
            let mut sums = [0.0; $half];
            let mut differences = [0.0; $half];
            for index in 0..$half {
                sums[index] = input[index] + input[$size - 1 - index];
                differences[index] = (input[index] - input[$size - 1 - index]) * factors[$half - 1 + index];
            }
            let even = $child(&sums, factors);
            let odd = $child(&differences, factors);
            let mut output = [0.0; $size];
            for index in 0..$half - 1 {
                output[2 * index] = even[index];
                output[2 * index + 1] = odd[index] + odd[index + 1];
            }
            output[$size - 2] = even[$half - 1];
            output[$size - 1] = odd[$half - 1];
            output
        }
    };
}

#[inline(always)]
fn dct2(input: &[f64; 2], factors: &[f64; 31]) -> [f64; 2] {
    [input[0] + input[1], (input[0] - input[1]) * factors[0]]
}

stage!(dct4, 4, 2, dct2);
stage!(dct8, 8, 4, dct4);
stage!(dct16, 16, 8, dct8);
stage!(transform, 32, 16, dct16);

pub(crate) fn dct32(input: &[f64; 32]) -> [f64; 32] {
    transform(input, factors())
}

#[cfg(test)]
mod tests {
    use super::dct32;

    #[test]
    fn matches_definition() {
        let mut random = 0x1234_5678_u32;
        let mut maximum_f64 = 0.0_f64;
        let mut maximum_f32 = 0.0_f32;
        for case in 0..1058 {
            let input = std::array::from_fn(|index| {
                if case < 32 {
                    return if index == case { 1.0 } else { 0.0 };
                }
                if case == 32 {
                    return 1.0;
                }
                if case == 33 {
                    return if index % 2 == 0 { 1.0 } else { -1.0 };
                }
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                f64::from((f64::from(random) / f64::from(u32::MAX) * 2.0 - 1.0) as f32)
            });
            let output = dct32(&input);
            for (frequency, actual) in output.iter().enumerate() {
                let expected: f64 = input
                    .iter()
                    .enumerate()
                    .map(|(index, sample)| sample * ((2 * index + 1) as f64 * frequency as f64 * std::f64::consts::PI / 64.0).cos())
                    .sum();
                maximum_f64 = maximum_f64.max((actual - expected).abs());
                maximum_f32 = maximum_f32.max((*actual as f32 - expected as f32).abs());
            }
        }
        println!("DCT-32 max delta: f64={maximum_f64:e}, f32={maximum_f32:e}");
        assert!(maximum_f64 <= 1e-9, "{maximum_f64:e}");
        assert!(maximum_f32 <= 1e-6, "{maximum_f32:e}");
    }
}
