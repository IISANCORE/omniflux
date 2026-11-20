# OmniFlux architecture map

**Purpose:** a new reader (human or AI) finds where a feature lives in under a
minute. If this file and the code disagree, the code is right and this file is
the bug — fix it in the same commit (Codebreaker Law 7).

**Status:** pre-alpha scaffold. Three engine crates (`core`, `project`,
`playback`), two benchmark spikes, one UI prototype window, no editor yet.
Anything marked *(planned)* describes the intended shape, not existing code.

## Current tree

```
crates/
  omniflux-core/     the engine's root crate: vocabulary every crate may depend on
                     (time, the timeline model, the edits that change it)
  omniflux-playback/ the decode side: ffmpeg sidecar, packet index, warm decoder, Player
  omniflux-project/  the .omni project file: saving a project and opening it again
  omniflux-ui/       the prototype window: frame display, scrub, play, split, delete, undo/redo, save
spikes/
  playback/          benchmark workbench: ffmpeg -> frames, prints measured numbers
  timeline/          benchmark workbench: the timeline model's edit/undo cost
  census/            the RAM census: what the engine actually uses, in one table
test-media/          local clips for manual runs (gitignored — the tests generate their own)
tools/               helper binaries and how to reproduce them
```

## Dependency direction (Law 11)

```
        ui / project    <- shells: display, scrub, play, save; the .omni file
             |
          features*     <- editing operations *(planned; none yet — the
             |             prototype edits go through the engine's history)*
          playback      <- decode: packet index, warm decoder, Player
             |
            core        <- model, time, edit decisions. depends on nothing.
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
| decoding, encoding, codec interop          | `crates/omniflux-playback`                                        |
| a window, a widget, a keystroke            | `crates/omniflux-ui` (the prototype window; more to come)         |
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
