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
  playback/             first benchmark spike: decode-to-frame pipeline workbench
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

Baseline (2026-11-09, release profile, this dev machine, BtbN
`win64-gpl` build): **1080p ≈ 122 fps sustained, 6 ms/frame (p95 8–9 ms),
first frame 139–170 ms on a cold ffmpeg spawn** — decode is ~4× faster than
the 30 fps realtime need; the sub-100 ms first-frame budget is a warm-engine
question, not a decode-speed one. Full numbers + verdict: the planning
vault's Task Note. These numbers describe ONE day on ONE machine — rerun on
any machine before trusting them there.

## License
OmniFlux is free software under the **GNU GPL v3.0 or later** — the full text
lives in [`LICENSE`](LICENSE). That is the free-forever guard: anyone may use,
study, share, and modify OmniFlux, and anyone who builds on it must keep their
version equally free. No paid tiers, no locked features, anywhere — ever.
