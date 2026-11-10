# tools/

Dev-machine-local binaries — **gitignored**; only this README is tracked, so
auditors can see WHY binaries live here and how to reproduce them (Law 2:
the FFmpeg choice is documented, not hardcoded into the build).

## `ffmpeg/` — BtbN static build (`win64-gpl`)

- Source (single release URL, always latest master):
  `https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip`
  (~192 MB; extracts one folder — move it so the binary lands at
  `ffmpeg/bin/ffmpeg.exe`, which is the path the spike's `locate_ffmpeg`
  expects).
- `win64-gpl` on purpose: it includes `libx264`. The GPL build is used
  **out-of-process** (CLI sidecar over a pipe, see `spikes/playback`); no GPL
  code enters the Rust crate graph, so the engine's own licensing stays an
  open, documented decision — see the planning vault's licensing notes.
- The folder also ships `ffprobe.exe` — the seek spike needs it for the
  packet index (keyframes, byte offsets, `has_b_frames`), so it is a
  requirement of `playback-spike seek`, not optional tooling. It is found
  next to the located ffmpeg, or via `OMNIFLUX_FFPROBE`. (HTML docs ship
  with the release too — candor: mostly dead weight, kept because they come
  with the archive.)
- `OMNIFLUX_FFMPEG` env var overrides the location for any local setup.
- Build used by all spike numbers recorded in the Task Note:
  `N-128718-gb2b543c079-20261109`.
