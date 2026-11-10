//! Distortion metrics: SAD, SATD (4x4 Hadamard), SSD.

use craft_distortion::{sad as craft_sad, sad8 as craft_sad8, sad16 as craft_sad16, satd as craft_satd};

#[inline]
pub fn sad(a: &[u8], sa: usize, b: &[u8], sb: usize, w: usize, h: usize) -> u32 {
    craft_sad(a, sa, b, sb, w, h)
}

/// SAD of a 16-wide block (fast path).
#[inline]
pub fn sad16(a: &[u8], sa: usize, b: &[u8], sb: usize, h: usize) -> u32 {
    craft_sad16(a, sa, b, sb, h)
}

/// SAD of an 8-wide block (fast path).
#[inline]
pub fn sad8(a: &[u8], sa: usize, b: &[u8], sb: usize, h: usize) -> u32 {
    craft_sad8(a, sa, b, sb, h)
}

/// SATD of 4x4 Hadamard-transformed differences (halved), for w, h multiples of 4.
pub fn satd(a: &[u8], sa: usize, b: &[u8], sb: usize, w: usize, h: usize) -> u32 {
    craft_satd(a, sa, b, sb, w, h)
}

pub fn ssd(a: &[u8], sa: usize, b: &[u8], sb: usize, w: usize, h: usize) -> u64 {
    let mut s = 0u64;
    for r in 0..h {
        let ra = &a[r * sa..r * sa + w];
        let rb = &b[r * sb..r * sb + w];
        s += ra.iter().zip(rb).map(|(&x, &y)| (x as i32 - y as i32).pow(2) as u32).sum::<u32>() as u64;
    }
    s
}

/// Variance*N (sum of squares minus square of sum / N) of a block.
pub fn block_var(a: &[u8], sa: usize, w: usize, h: usize) -> u32 {
    let mut s = 0u32;
    let mut ss = 0u32;
    for r in 0..h {
        for &v in &a[r * sa..r * sa + w] {
            s += v as u32;
            ss += (v as u32) * (v as u32);
        }
    }
    ss - ((s as u64 * s as u64) / (w * h) as u64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hadamard4x4_sum(d: &[i32; 16]) -> u32 {
        let mut t = [0i32; 16];
        for i in 0..4 {
            let a0 = d[i * 4] + d[i * 4 + 1];
            let a1 = d[i * 4] - d[i * 4 + 1];
            let a2 = d[i * 4 + 2] + d[i * 4 + 3];
            let a3 = d[i * 4 + 2] - d[i * 4 + 3];
            t[i * 4] = a0 + a2;
            t[i * 4 + 1] = a1 + a3;
            t[i * 4 + 2] = a0 - a2;
            t[i * 4 + 3] = a1 - a3;
        }
        let mut sum = 0u32;
        for i in 0..4 {
            let a0 = t[i] + t[4 + i];
            let a1 = t[i] - t[4 + i];
            let a2 = t[8 + i] + t[12 + i];
            let a3 = t[8 + i] - t[12 + i];
            sum += (a0 + a2).unsigned_abs() + (a1 + a3).unsigned_abs() + (a0 - a2).unsigned_abs() + (a1 - a3).unsigned_abs();
        }
        sum
    }
    #[test]
    fn satd_of_dc_difference() {
        let a = [10u8; 16];
        let b = [0u8; 16];
        // DC-only difference of 10: hadamard DC = 160, halved = 80.
        assert_eq!(satd(&a, 4, &b, 4, 4, 4), 80);
        assert_eq!(sad(&a, 4, &b, 4, 4, 4), 160);
    }

    #[test]
    fn satd_matches_reference() {
        let mut seed = 7u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            (seed >> 16) as u8
        };
        let a: Vec<u8> = (0..16 * 16).map(|_| rnd()).collect();
        let b: Vec<u8> = (0..16 * 16).map(|_| rnd()).collect();
        for (w, h) in [(16, 16), (8, 8), (4, 4), (16, 8), (8, 16), (12, 4)] {
            let mut want = 0;
            for by in (0..h).step_by(4) {
                for bx in (0..w).step_by(4) {
                    let mut d = [0i32; 16];
                    for r in 0..4 {
                        for c in 0..4 {
                            d[r * 4 + c] = a[(by + r) * 16 + bx + c] as i32 - b[(by + r) * 16 + bx + c] as i32;
                        }
                    }
                    want += hadamard4x4_sum(&d);
                }
            }
            assert_eq!(satd(&a, 16, &b, 16, w, h), want / 2, "{w}x{h}");
        }
    }

    #[test]
    fn satd_matches_reference_at_the_extremes() {
        // the largest differences (every lane at +-255) are where 16-bit sums could overflow
        let (zero, full) = ([0u8; 16 * 16], [255u8; 16 * 16]);
        let stripes: Vec<u8> = (0..16 * 16).map(|i| if (i / 16 + i % 16) % 2 == 0 { 255 } else { 0 }).collect();
        for (a, b) in [(&zero[..], &full[..]), (&full[..], &zero[..]), (&stripes[..], &zero[..]), (&zero[..], &stripes[..])] {
            for (w, h) in [(16, 16), (8, 8), (4, 4), (8, 16), (12, 4)] {
                let mut want = 0;
                for by in (0..h).step_by(4) {
                    for bx in (0..w).step_by(4) {
                        let mut d = [0i32; 16];
                        for r in 0..4 {
                            for c in 0..4 {
                                d[r * 4 + c] = a[(by + r) * 16 + bx + c] as i32 - b[(by + r) * 16 + bx + c] as i32;
                            }
                        }
                        want += hadamard4x4_sum(&d);
                    }
                }
                assert_eq!(satd(a, 16, b, 16, w, h), want / 2, "{w}x{h}");
            }
        }
    }
}
