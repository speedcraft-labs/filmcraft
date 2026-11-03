//! Siting-aware, separable bilinear reconstruction on the luma grid.
//! Half-luma phases become quarter-chroma phases on a subsampled axis. Both
//! axes retain their fractional bits until a single rounded integer result.

use filmcraft_color::ChromaSiting;

use crate::Chroma;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChromaFilter {
    #[default]
    Bilinear,
    /// Legacy block replication, for callers explicitly requesting fast previews.
    Nearest,
}

#[derive(Clone, Copy)]
struct Taps {
    first: usize,
    second: usize,
    weight: u32,
}

#[inline]
fn taps(position: usize, length: usize, shift: u32, phase: u32, filter: ChromaFilter) -> Taps {
    if shift == 0 || filter == ChromaFilter::Nearest {
        let index = (position >> shift).min(length - 1);
        return Taps { first: index, second: index, weight: 0 };
    }
    let coordinate = (position as i64 * 2 - i64::from(phase)).max(0) as usize;
    let first = (coordinate / 4).min(length - 1);
    Taps { first, second: (first + 1).min(length - 1), weight: (coordinate % 4) as u32 }
}

/// Validated pair of tightly packed integer chroma planes, sampled at full luma
/// resolution without allocating full-size temporary planes. Rows are independent
/// and can be used by the conversion's Rayon workers.
pub struct ChromaUpsampler<'a, T> {
    planes: [&'a [T]; 2],
    width: usize,
    height: usize,
    luma_height: usize,
    vertical_shift: u32,
    vertical_phase: u32,
    horizontal: Vec<Taps>,
    filter: ChromaFilter,
}

impl<'a, T: Copy + Into<u16>> ChromaUpsampler<'a, T> {
    /// Rejects empty, truncated or excessively large pictures before indexing.
    pub fn new(planes: [&'a [T]; 2], size: (usize, usize), chroma: Chroma, siting: ChromaSiting, filter: ChromaFilter) -> Option<Self> {
        let (width, height) = size;
        if width == 0 || height == 0 || width.checked_mul(height)? > 1 << 28 {
            return None;
        }
        let (horizontal_shift, vertical_shift) = chroma.shifts();
        let (horizontal_phase, vertical_phase) = siting.half_sample_offsets();
        let chroma_width = width.div_ceil(1 << horizontal_shift);
        let chroma_height = height.div_ceil(1 << vertical_shift);
        let samples = chroma_width.checked_mul(chroma_height)?;
        if planes.iter().any(|plane| plane.len() < samples) {
            return None;
        }
        Some(Self {
            planes,
            width: chroma_width,
            height: chroma_height,
            luma_height: height,
            vertical_shift,
            vertical_phase,
            horizontal: (0..width).map(|column| taps(column, chroma_width, horizontal_shift, horizontal_phase, filter)).collect(),
            filter,
        })
    }

    #[inline]
    pub fn row(&self, row: usize) -> ChromaRow<'_, T> {
        let row = row.min(self.luma_height - 1);
        let vertical = taps(row, self.height, self.vertical_shift, self.vertical_phase, self.filter);
        let planes = self.planes.map(|plane| {
            [&plane[vertical.first * self.width..(vertical.first + 1) * self.width], &plane[vertical.second * self.width..(vertical.second + 1) * self.width]]
        });
        ChromaRow { planes, horizontal: &self.horizontal, weight: vertical.weight }
    }
}

pub struct ChromaRow<'a, T> {
    planes: [[&'a [T]; 2]; 2],
    horizontal: &'a [Taps],
    weight: u32,
}

impl<T: Copy + Into<u16>> ChromaRow<'_, T> {
    /// Returns Cb/Cr integer codes, rounded once after both axes and edge-clamped.
    #[inline]
    pub fn sample(&self, column: usize) -> [u32; 2] {
        let horizontal = self.horizontal[column.min(self.horizontal.len() - 1)];
        self.planes.map(|rows| {
            let interpolate =
                |row: &[T]| u32::from(row[horizontal.first].into()) * (4 - horizontal.weight) + u32::from(row[horizontal.second].into()) * horizontal.weight;
            let upper = interpolate(rows[0]);
            let lower = interpolate(rows[1]);
            (upper * (4 - self.weight) + lower * self.weight + 8) >> 4
        })
    }
}
