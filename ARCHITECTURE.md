# OmniFlux architecture map

**Purpose:** a new reader (human or AI) finds where a feature lives in under a
minute. If this file and the code disagree, the code is right and this file is
the bug — fix it in the same commit (Codebreaker Law 7).

**Status:** pre-alpha scaffold. Two crates, one spike, no editor yet. Anything
marked *(planned)* describes the intended shape, not existing code.

## Current tree

```
crates/
  omniflux-core/     the engine's root crate: vocabulary every crate may depend on
                     (time, the timeline model, the edits that change it)
  omniflux-project/  the .omni project file: saving a project and opening it again
spikes/
  playback/          benchmark workbench: ffmpeg -> frames, prints measured numbers
  timeline/          benchmark workbench: the timeline model's edit/undo cost
test-media/          local clips for manual runs (gitignored — the tests generate their own)
tools/               helper binaries and how to reproduce them
```

## Dependency direction (Law 11)

```
        ui / cli        <- shells: translate input, render output
             |
        features        <- editing operations, built on core
             |
          core          <- model, time, edit decisions. depends on nothing.
```

Rules that follow from it:

- `core` never depends on anything above it, and never learns that a window, a
  codec, or a filesystem path exists.
- No cycles, ever. `cargo tree -p <crate>` is how you check.
- Shells stay thin: business logic in an event handler is a bug, because it
  cannot be tested without a screen.
- `spikes/*` may depend on `crates/*`, never the reverse. A spike is disposable
  once its numbers are written down.

## Where does a new thing go?

| The thing is...                            | It goes in                                                        |
| ------------------------------------------ | ----------------------------------------------------------------- |
| a type two crates must agree on            | `crates/omniflux-core`                                            |
| a pure transformation (cut, trim, map time)| `crates/omniflux-core`, or a feature crate named when it lands    |
| decoding, encoding, codec interop          | its own crate, named when it lands *(planned)*                    |
| a window, a widget, a keystroke            | the UI shell crate, named when it lands *(planned)*               |
| a measurement experiment                   | `spikes/`                                                         |
| a build or audit helper binary             | `tools/`                                                          |

If nothing in the table fits, the structure needs a decision — say so out loud.
Do not dump the file into a `utils` module (Law 11: `utils` is a smell).

## Verifying the structure still holds

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo nextest run
cargo deny check
```

`cargo tree` should show every arrow pointing down the diagram above. If it
does not, the change is structural and needs its own commit and its own review
— not a footnote inside a feature commit.
