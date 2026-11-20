//! The decode side of OmniFlux: turning a media file into pixels.
//!
//! One job: find FFmpeg, learn where a file's packets live, and keep one
//! decoder process alive that can be asked for a frame at a given packet.
//! Not responsible for: what the frames are *for* (that is the timeline and
//! the window), or for measuring performance (that is the playback spike,
//! which now uses this crate like any other consumer).
//!
//! Why FFmpeg as a sidecar and not a library: the process boundary means no
//! FFI, no `unsafe`, and a GPL codec runtime that never enters this build's
//! crate graph. The cost is a pipe, and the warm-decoder protocol below is
//! what makes that cost small.
//!
//! The three pieces:
//! - [`index`] asks `ffprobe` where every packet sits in the file (byte
//!   position, size, timestamp, keyframe flag) and how the stream is encoded
//!   (SPS/PPS, reorder depth). One `ffprobe` run per file, cached by the
//!   caller.
//! - [`annexb`] converts MP4's length-prefixed H.264 samples into the
//!   Annex-B byte stream a raw decoder expects. Pure byte work.
//! - [`warm`] is the long-lived decoder: one `ffmpeg -f h264 -i pipe:0` that
//!   is fed a self-contained slice of the stream per seek, instead of paying
//!   process spawn on every jump.
//!
//! Known limit, stated where a caller will read it: this crate decodes H.264
//! in MP4 only. Every other codec and container is a decision not yet made,
//! not a thing that quietly fails.

pub mod annexb;
pub mod index;
pub mod warm;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// How OmniFlux finds the FFmpeg binary, in order:
/// 1. `OMNIFLUX_FFMPEG` env var (explicit override)
/// 2. `<repo>/tools/ffmpeg/bin/ffmpeg(.exe)` (dev-machine local build, gitignored)
/// 3. plain `ffmpeg` on PATH
///
/// Documented here in ONE place (Law 2: binary discovery should be an
/// auditor-readable chain, not scattered path strings).
pub fn locate_ffmpeg() -> Result<PathBuf, String> {
    if let Ok(env_path) = std::env::var("OMNIFLUX_FFMPEG") {
        let path = PathBuf::from(env_path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "OMNIFLUX_FFMPEG is set but the file does not exist: {}",
            path.display()
        ));
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let exe_name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    // The crate sits at <repo>/crates/omniflux-playback, so two levels up is
    // the repository root that holds the gitignored tools build.
    let repo_tools = manifest
        .join("..")
        .join("..")
        .join("tools")
        .join("ffmpeg")
        .join("bin")
        .join(exe_name);
    if repo_tools.is_file() {
        return Ok(repo_tools);
    }
    Ok(PathBuf::from(exe_name))
}

/// Verifies a located binary can actually run. `locate_ffmpeg` cannot vouch
/// for the PATH fallback (it does not exist-check PATH entries), so callers
/// that need to *skip vs fail* ask this; spawners get a clean error either way.
pub fn probe_usable(ffmpeg: &Path) -> bool {
    Command::new(ffmpeg)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// FNV-1a, so a frame witness needs no dependency (Law 4). Two runs over the
/// same input produce the same value; two different frames almost surely
/// differ. It is a content witness, not a quality claim.
pub fn fnv1a(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &byte in data {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// The last few hundred bytes of a log file, for putting a decoder's own
/// complaint into an error message. Missing file means an empty string — the
/// caller is already reporting a failure and must not fail again here.
pub fn read_stderr_tail(path: &Path) -> String {
    use std::io::{Read, Seek};

    const TAIL_BYTES: i64 = 1024;
    let length = std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
    let tail = length.min(TAIL_BYTES);
    let mut text = String::new();
    if let Ok(mut file) = std::fs::File::open(path) {
        let _ = file.seek(std::io::SeekFrom::End(-tail));
        let _ = file.read_to_string(&mut text);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_witness_is_stable_and_content_dependent() {
        let frame_a = [1u8, 2, 3, 4];
        let frame_b = [1u8, 2, 3, 5];
        assert_eq!(fnv1a(&frame_a), fnv1a(&frame_a), "same bytes, same witness");
        assert_ne!(
            fnv1a(&frame_a),
            fnv1a(&frame_b),
            "different bytes must produce a different witness"
        );
    }

    #[test]
    fn a_missing_log_reads_as_empty_text_not_an_error() {
        let missing = std::env::temp_dir().join("omniflux-no-such-log-file.log");
        assert_eq!(read_stderr_tail(&missing), "");
    }
}
