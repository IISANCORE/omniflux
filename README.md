# OmniFlux

A free & open-source video editor in Rust. Desktop first, minimal surface for
everyone, deep engine underneath the hood.

**Status:** pre-alpha, engine scaffolding phase (see the planning vault's Task Note).

## Pillars
1. Runs on 4 GB RAM — performance is a feature, not an afterthought.
2. Local-first and forever-free; Quality-Codebreak laws are the non-negotiables.
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
  omniflux-core/        engine fundamentals shared by everything
spikes/
  playback/             first benchmark spike: decode-to-frame pipeline workbench
```

## Build
```
cargo build          # needs the MSVC toolchain on Windows (VS Build Tools)
cargo nextest run    # tests
cargo clippy         # lint gate
```
