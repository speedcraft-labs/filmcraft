# filmcraft-frame

Video frames with explicit colour metadata and shared CPU conversion to straight sRGB bytes or
premultiplied linear RGBA. Conversion workers use Rayon over independent output rows.

## Chroma reconstruction

`chroma::ChromaUpsampler` validates a pair of tightly packed Cb/Cr planes and reconstructs chroma
on the luma grid. It supports 4:2:0, 4:2:2 and identity 4:4:4, for both 8-bit and 16-bit storage.
Every CPU planar conversion (display, opaque fast path, float, decimated and ROI) uses it;
packed DPX YCbCr uses the same sampler. GPU sampling is unchanged.

For a subsampled axis, the chroma coordinate is `(luma_coordinate - phase) / 2`. Co-sited samples
have phase 0; centred samples have phase 0.5 luma samples (0.25 chroma samples). Left-sited 4:2:0
is horizontally co-sited and vertically centred. `ColorInfo::chroma_siting` also represents the
top/bottom variants from ITU-T H.264 (08/2024), Annex E, and H.265 (08/2021), Annex E. An axis
without subsampling ignores its phase. Boundary taps clamp to the first/last sample.

The separable bilinear kernel uses quarter-sample integer weights, retains both axes' fractional
bits, and rounds once. It precomputes horizontal taps, not full-resolution temporary planes.
Constant planes are exact, including full-range 16-bit maxima. Normalisation tables for 9–12-bit
codes are cached per bit depth/range in `OnceLock`; larger depths normalise directly.

Codec/container adapters preserve H.264/HEVC VUI, AV1 sample position, VP9 `vpcC`, Matroska
per-axis siting and Y4M JPEG/MPEG-2 conventions. Unspecified positions fall back to Left;
VP8 and JPEG-style Y4M use Center. Interlaced field-dependent reconstruction and PAL-DV's
different Cb/Cr vertical positions are not implemented by this progressive sampler.

Existing conversion APIs default to bilinear. Explicit fast-preview callers can request
`ChromaFilter::Nearest` through `to_opaque_srgb8_with_filter` or
`to_linear_f32_decimated_with_filter`; it retains legacy block replication. Current engine
preview tiers control resolution/draft decoding rather than chroma quality, so no tier is
silently switched back to nearest. Incomplete YUV planes produce transparent output (or `None`
from the opaque fast path) instead of indexing missing interpolation neighbours.

## Validation and timing

`cargo test -p filmcraft-frame -p filmcraft-color` covers exact DC preservation, 4:4:4 identity,
2D ramps within one LSB, horizontal/vertical siting step edges, 4:2:0/4:2:2, malformed planes,
8/10/12/16-bit conversion, alpha, decimation and ROI consistency.

The synthetic benchmark uses 1920×1080 YUV420, five warm-ups and 21 timed conversions,
reporting median/minimum and checking the opaque path against the float conversion:

```sh
CARGO_TARGET_DIR=target/agent-110 RAYON_NUM_THREADS=4 \
  cargo test --release -p filmcraft-frame --test chroma_perf -- --ignored --nocapture
```

Timing is diagnostic, not a CI threshold; run without concurrent builds or oracle tests.
On Apple M6 with four Rayon workers, the pre-change nearest conversion measured 1.330 ms median
(1.293 ms minimum); bilinear measured 1.995 ms median (1.938 ms minimum), about 50% more time.
The shared sampler's explicit Nearest mode measured 2.015 ms median in the same run; it preserves
legacy sampling but is not currently a faster specialized kernel.
