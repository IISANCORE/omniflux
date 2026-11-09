//! Playback spike pipeline — decode a media file into raw RGB24 frames and
//! measure the honest numbers: time-to-first-frame, per-frame latency
//! percentiles, pipe throughput.
//!
//! Spikes don't accrete: this either graduates into the real engine or dies.
//! It must stay readable enough that its numbers are believable — per
//! codebreaker Law 6, every number printed here was MEASURED, not hoped for.
//!
//! Decode-stack decision recorded for THIS spike: FFmpeg CLI sidecar over a
//! pipe (zero FFI, zero unsafe, GPL runtime kept outside the build's crate
//! graph). The production engine may revisit bindings later; that decision
//! needs this spike's numbers first.

use std::io::{BufReader, Read, Seek};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Instant;

/// How the spike finds the FFmpeg binary, in order:
/// 1. `OMNIFLUX_FFMPEG` env var (explicit override)
/// 2. `<repo>/tools/ffmpeg/bin/ffmpeg.exe` (dev-machine local build, gitignored)
/// 3. plain `ffmpeg` on PATH
///
/// Documented here in ONE place (Law 2: binary discovery should be an
/// auditor-readable chain, not scattered path strings).
pub fn locate_ffmpeg() -> Result<PathBuf, String> {
    if let Ok(env_path) = std::env::var("OMNIFLUX_FFMPEG") {
        let p = PathBuf::from(env_path);
        if p.is_file() {
            return Ok(p);
        }
        return Err(format!(
            "OMNIFLUX_FFMPEG is set but the file does not exist: {}",
            p.display()
        ));
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let exe_name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
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
/// that need to *skip vs fail* ask this (tests do); spawners get a clean
/// FfmpegMissing error either way.
pub fn probe_usable(ffmpeg: &Path) -> bool {
    Command::new(ffmpeg)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub struct PipelineSpec {
    /// Media file to decode (the spike never reads anything but this).
    pub input: PathBuf,
    /// Output scale forced so every frame has one known size — trivial framing,
    /// deterministic byte counts (the real engine will keep native resolution).
    pub scale: (u32, u32),
    /// Stop after N frames (None = decode to file end).
    pub max_frames: Option<usize>,
}

pub struct Summary {
    pub frames: usize,
    pub time_to_first_frame_ms: f64,
    pub frame_ms_p50: f64,
    pub frame_ms_p95: f64,
    pub avg_frame_hz: f64,
    pub total_bytes_per_sec: f64,
    /// FNV-1a over frame #1's bytes: proof the pipeline delivered REAL pixels,
    /// not silence. Two runs of the same input produce the SAME value; two
    /// DIFFERENT inputs almost surely differ. It is a content witness, not a
    /// quality claim.
    pub first_frame_fnv1a: u32,
}

impl Summary {
    /// One human line; every value measured by this run. No aspirational
    /// numbers are allowed anywhere near this print (Law 6).
    pub fn line(&self) -> String {
        format!(
            "frames={} ttf={:.1}ms p50={:.1}ms p95={:.1}ms avg={:.0}fps throughput={:.1}MB/s first_frame_fnv1a={:08x}",
            self.frames,
            self.time_to_first_frame_ms,
            self.frame_ms_p50,
            self.frame_ms_p95,
            self.avg_frame_hz,
            self.total_bytes_per_sec / (1024.0 * 1024.0),
            self.first_frame_fnv1a,
        )
    }
}

#[derive(Debug)]
pub enum SpikeError {
    /// The ffmpeg binary itself is missing — the fix is user-side, documented.
    FfmpegMissing(String),
    /// Decoding produced zero frames: bad file, bad args, or backend failure.
    NoFrames {
        ffmpeg_status: Option<i32>,
        stderr_tail: String,
    },
    Io(std::io::Error),
}

impl std::fmt::Display for SpikeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpikeError::FfmpegMissing(msg) => write!(
                f,
                "ffmpeg binary not usable: {msg}.\n  Set OMNIFLUX_FFMPEG=<path to ffmpeg.exe>, \
                 or place a build at <repo>/tools/ffmpeg/bin/ffmpeg.exe, or install ffmpeg on PATH."
            ),
            SpikeError::NoFrames {
                ffmpeg_status,
                stderr_tail,
            } => write!(
                f,
                "decode produced 0 frames (ffmpeg status {ffmpeg_status:?}). stderr tail: {stderr_tail}"
            ),
            SpikeError::Io(e) => write!(f, "spike I/O error: {e}"),
        }
    }
}
impl std::error::Error for SpikeError {}

/// Runs the spike. Returns a measured summary; errors are for humans.
pub fn run_pipeline(spec: &PipelineSpec, ffmpeg: &Path) -> Result<Summary, SpikeError> {
    let (w, h) = spec.scale;
    let frame_bytes = (w as usize) * (h as usize) * 3; // rgb24: 3 bytes per pixel

    // stderr goes to a temp FILE, not a pipe: ffmpeg error bursts can't block
    // the stdout reader, and the file is only read when the pipeline fails.
    // Bounded I/O — the spike never buffers unbounded data (RAM discipline).
    let stderr_path = std::env::temp_dir().join("omniflux-spike-ffmpeg-stderr.log");
    let stderr_file = std::fs::File::create(&stderr_path).map_err(SpikeError::Io)?;

    // Rawvideo over stdout pipe: after scale+pix_fmt are forced, one frame is
    // exactly w*h*3 bytes. -nostdin keeps ffmpeg off the terminal; loglevel
    // error keeps stderr quiet in the success path.
    let mut child: Child = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-i",
            &spec.input.to_string_lossy(),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-map",
            "0:v:0",
            "-",
        ])
        .stdout(Stdio::piped())
        .stdin(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SpikeError::FfmpegMissing(format!("could not spawn {}", ffmpeg.display()))
            } else {
                SpikeError::Io(e)
            }
        })?;

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SpikeError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "ffmpeg stdout pipe vanished",
            )));
        }
    };

    let mut reader = BufReader::with_capacity(frame_bytes, stdout);
    let mut frame = vec![0u8; frame_bytes];
    let mut frame_times: Vec<u128> = Vec::new();
    let mut frames = 0usize;
    let mut first_frame_fnv1a = 0u32;
    let start = Instant::now();
    // Bounded early exit: if we stop before the file ends, kill the child so
    // it doesn't keep decoding into a closed pipe.
    let mut truncated = false;

    loop {
        if let Some(max) = spec.max_frames {
            if frames >= max {
                truncated = true;
                break;
            }
        }
        let frame_start = Instant::now();
        if let Err(e) = reader.read_exact(&mut frame) {
            // UnexpectedEof = the decoder reached file end normally.
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                break;
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(SpikeError::Io(e));
        }
        frame_times.push(frame_start.elapsed().as_millis());
        if frames == 0 {
            first_frame_fnv1a = fnv1a(&frame);
        }
        frames += 1;
    }

    if truncated {
        let _ = child.kill();
    }
    let total_ms = start.elapsed().as_millis() as f64;
    let _ = child.wait();

    if frames == 0 {
        return Err(SpikeError::NoFrames {
            ffmpeg_status: child.try_wait().ok().flatten().and_then(|s| s.code()),
            stderr_tail: read_stderr_tail(&stderr_path),
        });
    }

    let p50 = percentile(&frame_times, 0.50);
    let p95 = percentile(&frame_times, 0.95);
    Ok(Summary {
        frames,
        // Honest TTF: the duration of reading frame #0, which starts right
        // after spawn (includes the process-spawn overhead — that's why it is
        // called time-to-first-FRAME, not time-to-pixels-on-a-GPU).
        time_to_first_frame_ms: *frame_times.first().unwrap_or(&0) as f64,
        frame_ms_p50: p50,
        frame_ms_p95: p95,
        avg_frame_hz: if total_ms > 0.0 {
            frames as f64 / (total_ms / 1000.0)
        } else {
            0.0
        },
        total_bytes_per_sec: frames as f64 * frame_bytes as f64 / (total_ms / 1000.0).max(0.001),
        first_frame_fnv1a,
    })
}

/// FNV-1a, so a checksum needs no dependency (Law 4).
fn fnv1a(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in data {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn percentile(times: &[u128], q: f64) -> f64 {
    if times.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<u128> = times.to_vec();
    sorted.sort_unstable();
    let idx = ((sorted.len() as f64) * q).min((sorted.len() - 1) as f64) as usize;
    sorted[idx] as f64
}

fn read_stderr_tail(path: &Path) -> String {
    let len = std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
    let tail = len.min(1024);
    let mut s = String::new();
    if let Ok(mut f) = std::fs::File::open(path) {
        let _ = f.seek(std::io::SeekFrom::End(-tail));
        let _ = f.read_to_string(&mut s);
    }
    s
}

/// Generates a deterministic test clip (testsrc2 pattern) for spike runs.
/// Kept beside the pipeline so tests and users generate IDENTICAL inputs.
pub fn generate_test_clip(
    ffmpeg: &Path,
    out_path: &Path,
    frames: usize,
    scale: (u32, u32),
) -> Result<(), SpikeError> {
    let stderr_path = std::env::temp_dir().join("omniflux-spike-ffmpeg-gen.log");
    let stderr_file = std::fs::File::create(&stderr_path).map_err(SpikeError::Io)?;
    let status = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc2=size={}x{}:rate=30", scale.0, scale.1),
            "-frames:v",
            &frames.to_string(),
            // libx264 = the GPL codec reality: documented, stays OUTSIDE the
            // Rust build, reachable only if the application shells out.
            "-c:v",
            "libx264",
            "-crf",
            "28",
            "-y",
            &out_path.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .status()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SpikeError::FfmpegMissing(format!("could not spawn {}", ffmpeg.display()))
            } else {
                SpikeError::Io(e)
            }
        })?;
    if !status.success() {
        return Err(SpikeError::NoFrames {
            ffmpeg_status: status.code(),
            stderr_tail: read_stderr_tail(&stderr_path),
        });
    }
    Ok(())
}
