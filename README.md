# OmniFlux

A free & open-source video editor in Rust. Desktop first, minimal surface for
everyone, deep engine underneath the hood.

**Status:** pre-alpha, engine scaffolding phase (see the planning vault's Task Note).

## Pillars
1. Runs on 4 GB RAM — performance is a feature, not an afterthought.
2. Local-first and forever-free; Codebreaker laws are the non-negotiables.
3. Minimalist UI, deliberately deep — progressive disclosure, not smoked-glass menus.

## The Codebreaker law
All code in this repository follows the **Codebreaker** rules (clean, auditable,
no backdoors, no cheating, understandable by Rust devs and updatable by the owner
 without the original AI). The law lives at:
- Skill definition available to every coding session: see `codebreaker` skill.

Short version: if any change is shrink-wrapped to a reader who has five minutes,
it ships. Nothing hidden, nothing fake, no cleverness for cleverness's sake.

## Layout
```
crates/
  omniflux-core/        the engine's root crate: vocabulary every crate may depend on
spikes/
  playback/             decode-to-frame pipeline + warm-scrub seek mode
  timeline/             the timeline model measured under load: edit cost, undo cost
```

Where a new feature belongs, and which crate owns what:
see [`ARCHITECTURE.md`](ARCHITECTURE.md).

## Build
```
cargo build            # needs the MSVC toolchain on Windows (VS Build Tools)
cargo nextest run      # tests (install-able via `cargo install cargo-nextest`)
cargo fmt --check      # formatting gate
cargo clippy -- -D warnings
cargo audit            # RustSec advisory gate — needs `cargo install cargo-audit`
cargo deny check       # license/ban gate — needs `cargo install cargo-deny`
```

## Run the playback spike — measured reality
The spike decodes by shelling out to a local ffmpeg build and prints its
numbers for the run. Set up the binary once per `tools/README.md`
(or point `OMNIFLUX_FFMPEG` at any ffmpeg you trust):

```
cargo run --release -p playback-spike -- <clip> [max frames] [WxH]
# example: the 1080p budget question
cargo run --release -p playback-spike -- test-media/spike-1080p-120f.mp4 120 1920x1080
```

(`test-media/` holds local clips and is gitignored — the integration tests
generate their own footage, so a fresh clone needs no media to pass CI.)

Baseline (2026-11-09, release profile, this dev machine, BtbN
`win64-gpl` build): **1080p ≈ 122 fps sustained, 6 ms/frame (p95 8–9 ms),
first frame 139–170 ms on a cold ffmpeg spawn** — decode is far past the
realtime budget; the sub-100 ms first-frame budget is a warm-engine
question, not a decode-speed one. Full numbers + verdict: the planning
vault's Task Note. These numbers describe ONE day on ONE machine — rerun on
any machine before trusting them there.

## Run the seek spike — warm decoder vs cold spawn
Scrubbing is the other half of the story: every jump used to pay a fresh
ffmpeg process. The seek mode keeps ONE decoder alive and feeds it keyframes
over stdin:

```
cargo run --release -p playback-spike -- seek <clip> [jumps]   # default 8 jumps
```

Both paths decode the same keyframe and hand this process rgb24 pixels; the
run compares them by FNV-1a witness and **fails loudly if the pixels differ**.

How the protocol works (the code says the same thing in `spikes/playback/src/warm.rs`):
`L = ffprobe has_b_frames` is how many decoded frames the decoder holds back
for reordering, so a jump feeds the target keyframe plus the `L` packets that
follow it, then reads `L+1` frames and takes the last as the target. Three
FFmpeg details are load-bearing, and each was found by measurement rather
than by reading docs: `-flags low_delay` must come **before** `-i` (after it,
later stdin bursts are demuxed but never decoded until EOF — a persistent
decoder that answers one burst and then goes silent), the probe must be
smaller than the first feed (`-analyzeduration 0 -probesize 4k`), and this
build's `rawvideo` encoder delays every frame by one unless pinned with
`-threads:v 1`.

Measured (2026-11-10, release profile, this dev machine, 40 s clips: 1200
packets, 80 keyframes, `has_b_frames=2`):

| clip | cold p50 | warm p50 | warm p95 | speedup | pixels identical |
|---|---|---|---|---|---|
| 640×360 | 47.1 ms | **2.5 ms** | 3.2 ms | 19.0× | 8/8 |
| 1280×720 | 66.9 ms | **8.8 ms** | 9.2 ms | 7.6× | 8/8 |
| 1920×1080, dense GOP | 96.6 ms | **17.8 ms** | 19.4 ms | 5.4× | 8/8 |
| 1920×1080, sparse GOP | 95.6 ms | **18.3 ms** | 19.6 ms | 5.2× | 4/4 |

(Reruns move the warm p50 by about ±1 ms at 1080p — the cold path is the
steady one, the warm path is a 4–20 ms number either way.)

Honest read of those numbers: the warm path removes the spawn-and-open cost
(~80 ms), and what is left is the same per-frame decode+pixel cost the
playback pipeline already measured (≈6 ms/frame at 1080p). So the next scrub
lever is **preview resolution**, not a faster decoder — and mid-GOP positions
still need the frames in between, which is what dense-GOP scrub proxies are
for. Keyframe jumps only, for now.

## The timeline model — the arrangement an editor edits

The engine's core crate now holds the thing an editor actually edits: where
material sits, for how long, on which track, measured on one exact clock.

- **Exact time, not floats.** Every position is a count of *ticks* on a
  project timebase of 90 000 ticks per second — which divides exactly by 24,
  25, 30, 50 and 60 fps, so a frame is always a whole number of ticks and
  "the clip does not line up with the audio" is not a bug waiting to happen.
- **The model enforces its own rules.** Clips on one track never overlap, a
  clip never runs past the end of its source, a clip always lasts more than
  zero. A refused edit changes nothing at all.
- **Edits are commands that know how to undo themselves.** Applying an edit
  *returns* the command that undoes it, so undo and redo are one operation
  with two stacks — there is no second copy of "undo" to drift out of step.

Measured (2026-11-10, release profile, this dev machine): 500 clips on 4
tracks, 5 000 edits, then every edit undone and redone.

| what | measured |
|---|---|
| build 500 clips | 0.1 ms (0.1 µs/clip) |
| one edit | p50 **0.001 ms**, p95 0.002 ms (~1M edits/s) |
| one undo / redo | p50 0.001 ms (5 000 undos in 5.3 ms) |
| one undoable edit in RAM | **~100 bytes** → ~10 MB for 100 000 undo steps |
| one clip in RAM | ~140 bytes (500-clip project: 4 MB resident) |
| undoing everything | **restores the project exactly** (and so does redo) |

(The two memory figures come from resident-memory deltas, so they move in
pages — good enough to decide a design, not to quote to three decimals.)

What those numbers decide: the edit machinery will never be what makes the
editor feel slow (an edit is ~1 µs; a frame is 16 ms), "unlimited undo" fits
inside the 4 GB budget (9 MB per 100 000 steps), and a project with hundreds
of clips is small (~4 MB). The heavy part of the memory budget is the decode
side, not the model — which the RAM census still has to measure.

```
cargo run --release -p timeline-spike -- [clips] [edits]
```

## License
OmniFlux is free software under the **GNU GPL v3.0 or later** — the full text
lives in [`LICENSE`](LICENSE). That is the free-forever guard: anyone may use,
study, share, and modify OmniFlux, and anyone who builds on it must keep their
version equally free. No paid tiers, no locked features, anywhere — ever.
