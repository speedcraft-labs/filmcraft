# filmcraft-media

Media sources, still-image and image-sequence import, generated media, audio streams, and decoded
frame caching for FilmCraft.

## Frame cache

`FrameCache` uses a key-to-slot `HashMap` and a doubly linked LRU list embedded in reusable
entry slots. Hits, insertion bookkeeping and each eviction take expected O(1) time, without
collecting or sorting the resident map. An insertion that evicts multiple frames takes O(v)
for v victims; insertion is amortized O(1). `retain` and `clear` remain O(n).

Policy is unchanged: hits and replacements move the entry to newest; misses do not change
relative recency. Only an insertion exceeding the byte budget starts eviction, removing oldest
first until resident frame bytes are at most `floor(9 * budget / 10)`. This can evict the new
frame too, including an oversized frame. Zero-byte frames survive until pressure evicts them
in normal recency order. Replacement costs, hit/miss counters and `get_or_insert_with` results
are preserved; clearing and retaining entries do not reset counters. Accounting uses a widened
intermediate so summing costs and computing the 90% target cannot wrap.

There is **no pin/unpin API or pinned-entry exemption** in this cache. Holding an `Arc<VideoFrame>`
keeps the returned frame alive, not its cache membership; those externally held bytes are not
counted after eviction. The previous counter assigned distinct stamps under the same mutex,
so equal-recency ties were unreachable before counter overflow. The linked list preserves that
strict operation order without a clock or clock-overflow/tie ambiguity. Keys still need only
`Eq + Hash + Clone`, not `Ord`.

Tests compare against an independent oldest-to-newest `Vec` model over 512 generated sequences
of insert/replacement, get, held-Arc acquisition/release, explicit internal LRU removal, retain,
clear and fallible `get_or_insert_with`. They check hit/miss and frame identity, exact recency
and victim order, byte totals, counters, both link directions and free-slot integrity after every
operation. Deterministic regressions cover the 90% threshold, rounding, zero/oversized frames,
held Arcs, replacement and slot reuse.

Manual benchmark (ignored by normal tests):

```sh
cargo test -p filmcraft-media --release bench_frame_cache_insert -- --ignored --nocapture
```

On November 3, 2026, Apple M6 / arm64 macOS, rustup stable Rust 1.99.0, the same release-mode
harness warmed a 1,000-entry / 16,000-byte cache, then timed 100,000 unique inserts of a shared
16-byte frame, five runs per implementation:

| Implementation | Median | Range |
| --- | ---: | ---: |
| Original map collection + sort | 18.28 ms | 17.69–22.57 ms |
| Linked reusable slots | 4.05 ms | 3.74–4.79 ms |

The measured median improvement is about 4.5×. This is a microbenchmark, not an end-to-end
playback claim. The machine was shared: load averages (1/5/15 minutes) were 5.71/10.84/11.32
before the baseline build and 71.02/49.85/28.45 after the optimized run, so these are not
controlled-machine measurements. Both runs used `CARGO_TARGET_DIR=target-104` in the worktree.

## Image-sequence filenames

`sequence::Numbered` identifies the last run of 1–18 ASCII frame-number digits in a filename's
stem when the extension is a recognized still-image format. `/` and `\` separate directory
components. A prefix may contain other UTF-8 characters, including accented letters, CJK and emoji:
`é0001.png`, `图0001.png` and `🎞️0001.png`. Parsing preserves the prefix, number padding and suffix
when generating sibling-frame paths. Sibling recognition compares the prefix's UTF-8 bytes exactly.
Stems without ASCII digits or with a final frame-number run longer than 18 characters are not numbered
image-sequence filenames.
