//! Warm-decoder spike: keep ONE ffmpeg process alive and hand it a fresh
//! slice of the stream over stdin for every jump, instead of paying process
//! spawn + file open on every seek. This file is the experiment, not the
//! player: it measures time-to-frame-on-jump against the cold spawn-per-jump
//! baseline and proves pixel equality between the two paths.
//!
//! Protocol (v0, keyframe jumps only — the unambiguous case). Let
//! `L = ffprobe has_b_frames`, the decoder's reorder hold. A jump at a
//! keyframe feeds that keyframe plus the L packets that follow it, as
//! self-contained Annex-B access units (parameter sets + AUD + sample, see
//! `annexb.rs`), then reads L+1 frames and takes the LAST as the target.
//! The first feed after spawning is the warm-up: it also reads one frame,
//! which is that keyframe; from then on the decoder holds exactly L frames
//! back, so every later burst is L held frames followed by the target.
//! Measured across x264 bf=0/1/2/3: L = has_b_frames exactly (0→0, 1→1, 2→2).
//!
//! Two FFmpeg details are load-bearing and were found by measurement:
//! - `-flags low_delay` MUST precede `-i`. Without it FFmpeg demuxes later
//!   stdin bursts but does not decode them until stdin reaches EOF, so a
//!   persistent decoder emits one burst and then stalls forever.
//! - `-analyzeduration 0 -probesize 4k`: the first feed is only a few KB;
//!   a probe larger than that stalls waiting for bytes that may never come
//!   (32k stalled on 6 KB feeds; 4k opened every feed tested down to 1 KB).
//! - `-threads:v 1` AFTER `-i`: this build's rawvideo encoder advertises
//!   frame threading and delays every frame by one, so the first feed muxes
//!   nothing until the next arrives. Pinning the encoder to one thread makes
//!   each burst arrive immediately (measured A→1, B→3, C→3 frames; default
//!   threading gave A→0, B→2). The decoder keeps its normal threading.
//! - Shutdown order: close the decoder's stdin FIRST (EOF is how a decoder is
//!   told to finish), then kill the process we spawned, then wait for the
//!   reader thread. Killing alone is not enough: the binary we spawn may be a
//!   WRAPPER — Chocolatey's `choco install ffmpeg` puts a shim on PATH, and
//!   killing the shim leaves the real ffmpeg running behind it, still holding
//!   the stdout pipe open. That is exactly how the first CI run hung for
//!   15 minutes on Windows: every jump had succeeded and the test could not
//!   finish, because the reader thread waited on a pipe nobody would close.
//!   Closing stdin reaches the real binary whatever sits in front of it.
//!
//! Honest limits, stated up front:
//! - Cases are KEYFRAME timestamps only. Mid-GOP jumps (the typical scrub
//!   position) still require decoding the frames in between; this spike
//!   measures the spawn/open overhead being removed, which is the part a
//!   warm engine can remove. Dense keyframes (proxies) remain the lever for
//!   the in-between frames.
//! - Keyframes are intra-coded, so the target's pixels do not depend on the
//!   L following packets; those are fed only to push the target out of the
//!   reorder buffer. The pixel witness checks that claim on every jump.
//! - Frames already queued before a feed is written are counted as
//!   `stale_drained`. A non-zero count is a protocol error, never silently
//!   attributed to the jump.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::annexb;
use crate::index::{build_index, locate_ffprobe, MediaIndex, Packet};
use crate::{fnv1a, read_stderr_tail};

/// Upper bound for one feed's decoder response before the spike declares
/// failure. Generous on purpose: a real hang must be loud, not a spinner.
const JUMP_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound for the reader thread to finish once the decoder is gone.
/// Past it we detach the thread (and say so) rather than hang the run.
const READER_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Frame {
    /// Moment the full frame was in this process's hands.
    pub arrived: Instant,
    /// Exactly width*height*3 bytes of rgb24.
    pub bytes: Vec<u8>,
}

/// One feed's worth of decoder output.
pub struct Burst {
    /// Last frame of the burst — for a jump, the target keyframe's frame.
    pub frame: Frame,
    /// Frames the decoder emitted for this feed (1 during warm-up, L+1 for
    /// every jump).
    pub frames_read: usize,
    /// From starting to write the feed to the last frame arriving.
    pub elapsed: Duration,
    /// Frames already queued before this feed was written. Must stay 0.
    pub stale_drained: usize,
}

/// A long-lived `ffmpeg -f h264 -i pipe:0` whose frames arrive on a reader
/// thread; the main thread stays free to time each jump.
pub struct WarmDecoder {
    child: Child,
    /// The decoder's stdin, in an Option so shutdown can close it (EOF) on
    /// purpose — see the module comment on wrapper processes.
    stdin: Option<ChildStdin>,
    frames: Receiver<Frame>,
    reader: Option<JoinHandle<()>>,
    /// Signalled by the reader thread when it has finished, so shutdown can
    /// wait for it without joining blindly.
    reader_done: Receiver<()>,
    stderr_path: PathBuf,
    frame_bytes: usize,
    /// `L` from ffprobe's `has_b_frames`; every jump reads L+1 frames.
    reorder_depth: usize,
}

impl WarmDecoder {
    /// Starts the persistent decoder. `reorder_depth` is the index's
    /// `has_b_frames` — the protocol cannot work without the true value, so
    /// it is an explicit argument rather than guessed here.
    pub fn spawn(
        ffmpeg: &Path,
        width: u32,
        height: u32,
        reorder_depth: u32,
    ) -> Result<Self, String> {
        let frame_bytes = width as usize * height as usize * 3;
        let stderr_path = std::env::temp_dir().join("omniflux-seek-ffmpeg-stderr.log");
        let stderr_file = File::create(&stderr_path)
            .map_err(|e| format!("could not create decoder stderr log: {e}"))?;

        let mut child = Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                // BEFORE -i: applies to the stdin decoder. After -i it lands
                // on the rawvideo encoder and the decoder stalls after one
                // burst (measured; see the module comment).
                "-flags",
                "low_delay",
                // The first feed is a few KB; a bigger probe would wait for
                // bytes that may never arrive (measured; see module comment).
                "-analyzeduration",
                "0",
                "-probesize",
                "4k",
                "-f",
                "h264",
                "-i",
                "pipe:0",
                // AFTER -i: applies to the rawvideo encoder. Its default
                // frame threading holds one frame back (measured; see the
                // module comment), which would break per-jump reads.
                "-threads:v",
                "1",
                "-fps_mode",
                "passthrough",
                "-pix_fmt",
                "rgb24",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr_file))
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    format!("could not spawn ffmpeg at {}", ffmpeg.display())
                } else {
                    format!("could not spawn warm decoder: {e}")
                }
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "warm decoder stdout pipe vanished".to_string())?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "warm decoder stdin pipe vanished".to_string())?;

        let (tx, rx) = mpsc::channel::<Frame>();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let reader = std::thread::spawn(move || {
            let mut stdout = stdout;
            loop {
                let mut bytes = vec![0u8; frame_bytes];
                match stdout.read_exact(&mut bytes) {
                    Ok(()) => {
                        let arrived = Instant::now();
                        if tx.send(Frame { arrived, bytes }).is_err() {
                            break; // receiver gone; stop cleanly
                        }
                    }
                    Err(_) => break, // decoder closed/killed: no more frames
                }
            }
            let _ = done_tx.send(());
        });

        Ok(WarmDecoder {
            child,
            stdin: Some(stdin),
            frames: rx,
            reader: Some(reader),
            reader_done: done_rx,
            stderr_path,
            frame_bytes,
            reorder_depth: reorder_depth as usize,
        })
    }

    pub fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }

    pub fn reorder_depth(&self) -> usize {
        self.reorder_depth
    }

    /// Sends the first feed and reads the ONE frame it emits. Leaves the
    /// decoder holding L frames — the steady state every jump assumes.
    pub fn warm_up(&mut self, feed: &[u8]) -> Result<Burst, String> {
        self.send_and_read(feed, 1)
    }

    /// Sends one jump feed and reads L+1 frames; the last is the target.
    pub fn jump(&mut self, feed: &[u8]) -> Result<Burst, String> {
        self.send_and_read(feed, self.reorder_depth + 1)
    }

    fn send_and_read(&mut self, feed: &[u8], frames_wanted: usize) -> Result<Burst, String> {
        let stale_drained = self.drain_stale()?;

        let started = Instant::now();
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "the warm decoder's stdin is already closed".to_string())?;
        stdin
            .write_all(feed)
            .and_then(|()| stdin.flush())
            .map_err(|e| {
                format!(
                    "writing {} feed bytes to the warm decoder failed: {e}; stderr tail: {}",
                    feed.len(),
                    self.stderr_tail()
                )
            })?;

        let mut read = 0usize;
        let mut last: Option<Frame> = None;
        for _ in 0..frames_wanted {
            match self.frames.recv_timeout(JUMP_TIMEOUT) {
                Ok(frame) => {
                    read += 1;
                    last = Some(frame);
                }
                Err(e) => {
                    return Err(format!(
                        "warm decoder delivered only {read}/{frames_wanted} frames within {} s \
                         ({e}); feed was {} bytes; stderr tail: {}",
                        JUMP_TIMEOUT.as_secs(),
                        feed.len(),
                        self.stderr_tail()
                    ));
                }
            }
        }
        let frame = last.ok_or_else(|| "a feed must request at least one frame".to_string())?;
        Ok(Burst {
            elapsed: frame.arrived.duration_since(started),
            frame,
            frames_read: read,
            stale_drained,
        })
    }

    /// Frames that arrived before the current feed are protocol slack: they
    /// belong to the previous burst but were not counted. Returning an error
    /// on a dead channel keeps a crashed decoder from looking like a slow one.
    fn drain_stale(&mut self) -> Result<usize, String> {
        let mut stale = 0usize;
        loop {
            match self.frames.try_recv() {
                Ok(_) => stale += 1,
                Err(TryRecvError::Empty) => return Ok(stale),
                Err(TryRecvError::Disconnected) => {
                    return Err(format!(
                        "the warm decoder exited before a feed could be sent; stderr tail: {}",
                        self.stderr_tail()
                    ));
                }
            }
        }
    }

    fn stderr_tail(&self) -> String {
        read_stderr_tail(&self.stderr_path)
    }
}

impl Drop for WarmDecoder {
    fn drop(&mut self) {
        // Order matters, and it is the order that survives a wrapper process
        // (Chocolatey's shim, for one) sitting between us and the real
        // decoder — see the module comment.
        //
        // 1. Close stdin: EOF tells the decoder to finish. This is the only
        //    signal that reaches the REAL binary when a wrapper is in front.
        self.stdin = None;
        // 2. Kill the process we spawned (the wrapper, in that case). Killing
        //    is best-effort: it may already be gone.
        let _ = self.child.kill();
        let _ = self.child.wait();
        // 3. Wait for the reader thread, but only for a while: it ends when
        //    the decoder's stdout closes. If something outside this process
        //    still holds that pipe open, detach rather than hang the whole
        //    run — and say so loudly, because a stranded decoder is a fact
        //    the reader of the log deserves.
        if let Some(reader) = self.reader.take() {
            match self.reader_done.recv_timeout(READER_JOIN_TIMEOUT) {
                Ok(()) => {
                    let _ = reader.join();
                }
                Err(_) => {
                    eprintln!(
                        "warning: the warm decoder's reader thread did not finish within {} s \
                         (a wrapper process may still hold the decoder's stdout pipe); \
                         detaching it instead of hanging",
                        READER_JOIN_TIMEOUT.as_secs()
                    );
                    drop(reader); // detach: the thread ends with the process
                }
            }
        }
    }
}

/// The baseline the warm path must beat: one fresh ffmpeg process per jump,
/// fast input seek to the target keyframe, one rgb24 frame delivered.
/// Returns (wall time spawn→exit, frame bytes).
pub fn cold_seek(ffmpeg: &Path, file: &Path, t: f64) -> Result<(Duration, Vec<u8>), String> {
    let t_arg = format!("{t:.6}");
    let started = Instant::now();
    let output = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-ss",
            &t_arg,
            "-i",
            &file.to_string_lossy(),
            "-frames:v",
            "1",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "-",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run cold seek: {e}"))?;
    let elapsed = started.elapsed();
    if !output.status.success() {
        return Err(format!(
            "cold seek at t={t:.3}s failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if output.stdout.is_empty() {
        return Err(format!("cold seek at t={t:.3}s produced no frame"));
    }
    Ok((elapsed, output.stdout))
}

/// Reads packets `[start, start + count)` and assembles one self-contained
/// decoder feed: parameter sets + AUD + Annex-B sample for every packet,
/// plus a trailing AUD to close the final access unit. Shared by the CLI
/// spike and its integration test so both feed byte-identical data.
pub fn build_feed(
    file: &mut File,
    index: &MediaIndex,
    start: usize,
    count: usize,
) -> Result<Vec<u8>, String> {
    if count == 0 {
        return Err("a feed needs at least one packet".to_string());
    }
    let end = start
        .checked_add(count)
        .ok_or_else(|| format!("packet range {start} + {count} overflows"))?;
    if end > index.packets.len() {
        return Err(format!(
            "packet range {start}..{end} exceeds the {} packets in the index",
            index.packets.len()
        ));
    }
    let params = annexb::param_sets_annexb(&index.avc);
    let mut feed = Vec::new();
    for packet in &index.packets[start..end] {
        let sample = read_sample(file, packet)?;
        annexb::append_access_unit(&mut feed, &params, &sample, index.avc.nal_length_size)?;
    }
    feed.extend_from_slice(&annexb::AUD);
    Ok(feed)
}

fn read_sample(file: &mut File, packet: &Packet) -> Result<Vec<u8>, String> {
    file.seek(SeekFrom::Start(packet.pos))
        .map_err(|e| format!("seek to packet byte {} failed: {e}", packet.pos))?;
    let mut buf = vec![0u8; packet.size as usize];
    file.read_exact(&mut buf).map_err(|e| {
        format!(
            "reading {} bytes at {} failed: {e}",
            packet.size, packet.pos
        )
    })?;
    Ok(buf)
}

fn percentile_f64(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((sorted.len() as f64 * q) as usize).min(sorted.len() - 1);
    sorted[idx]
}

/// Runs the whole warm-vs-cold experiment on one file and prints measured
/// rows plus a summary. Fails loudly on any pixel-witness mismatch: numbers
/// from a pipeline that produced a different frame than the baseline are
/// not allowed to masquerade as valid (Law 6).
pub fn run_seek_spike(ffmpeg: &Path, file: &Path, jumps: usize) -> Result<(), String> {
    let ffprobe = locate_ffprobe(ffmpeg)?;
    let index = build_index(&ffprobe, file)?;
    let frame_bytes = index.width as usize * index.height as usize * 3;
    let reorder = index.has_b_frames as usize;
    let feed_packets = reorder + 1;

    println!("ffprobe={}", ffprobe.display());
    println!(
        "file={} w={} h={} packets={} keyframes={} reorder_depth={}",
        file.display(),
        index.width,
        index.height,
        index.packets.len(),
        index.keyframe_indices().len(),
        reorder
    );
    println!(
        "protocol: per jump feed the target keyframe + {reorder} following packet(s) \
         ({feed_packets} access units); warm-up reads 1 frame, every jump reads {feed_packets} \
         and takes the last as the target."
    );
    println!(
        "method: warm = ONE persistent ffmpeg (`-flags low_delay` before `-i`, \
         `-analyzeduration 0 -probesize 4k`, `-f h264 -i pipe:0`) fed over stdin;\n\
         \x20       cold = fresh ffmpeg `-ss` fast-seek per jump. Both deliver rgb24 frames \
         to this process; witness = FNV-1a of the frame bytes (equal = identical pixels)."
    );

    // A keyframe needs L packets after it to push its frame out of the
    // decoder's reorder hold; keyframes at the very end of the file cannot.
    let usable: Vec<usize> = index
        .keyframe_indices()
        .into_iter()
        .filter(|&i| i + reorder < index.packets.len())
        .collect();
    if usable.len() < 2 {
        return Err(format!(
            "need at least 2 keyframes with {reorder} following packet(s) (the reorder depth), \
             found {} — the clip is too short or its GOP too sparse for this spike",
            usable.len()
        ));
    }

    let mut file_reader =
        File::open(file).map_err(|e| format!("could not open {}: {e}", file.display()))?;

    // Warm-up: pays the decoder's spawn+probe cost once. Measured and
    // printed, but excluded from the jump statistics — that cost is the
    // engine's file-open story, not its scrub story.
    let warm_kf = usable[0];
    let warm_pts = index.packets[warm_kf].pts_time;
    let warm_feed = build_feed(&mut file_reader, &index, warm_kf, feed_packets)?;
    let mut warm = WarmDecoder::spawn(ffmpeg, index.width, index.height, index.has_b_frames)?;
    let warmup = warm.warm_up(&warm_feed)?;
    if warmup.frame.bytes.len() != frame_bytes {
        return Err(format!(
            "warm-up frame has {} bytes, expected {frame_bytes}",
            warmup.frame.bytes.len()
        ));
    }
    let warmup_witness = fnv1a(&warmup.frame.bytes);
    let (_, warmup_cold) = cold_seek(ffmpeg, file, warm_pts)?;
    if warmup_cold.len() != frame_bytes {
        return Err(format!(
            "cold warm-up frame has {} bytes, expected {frame_bytes}",
            warmup_cold.len()
        ));
    }
    let warmup_cold_witness = fnv1a(&warmup_cold);
    if warmup_witness != warmup_cold_witness {
        return Err(format!(
            "pixel witness mismatch at warm-up t={warm_pts:.3}s: warm {warmup_witness:08x} \
             != cold {warmup_cold_witness:08x} — the warm path is not delivering the same frame"
        ));
    }
    println!(
        "warm_start={:.1}ms frames_read={} witness={:08x} cold_witness={:08x} match=yes",
        warmup.elapsed.as_secs_f64() * 1000.0,
        warmup.frames_read,
        warmup_witness,
        warmup_cold_witness
    );

    // Every measured jump starts after the warm-up keyframe, so no feed is a
    // byte-for-byte replay of the warm-up.
    let pool = &usable[1..];
    let n = jumps.max(1).min(pool.len());
    let last_pool = pool.len() - 1;
    let mut chosen: Vec<usize> = Vec::with_capacity(n);
    for i in 0..n {
        let at = if n == 1 { 0 } else { i * last_pool / (n - 1) };
        if chosen.last() != Some(&at) {
            chosen.push(at);
        }
    }

    let mut warm_ms: Vec<f64> = Vec::with_capacity(chosen.len());
    let mut cold_ms: Vec<f64> = Vec::with_capacity(chosen.len());
    let mut matches = 0usize;

    for &at in &chosen {
        let kf_idx = pool[at];
        let packet = &index.packets[kf_idx];
        let feed = build_feed(&mut file_reader, &index, kf_idx, feed_packets)?;
        let burst = warm.jump(&feed)?;
        if burst.stale_drained != 0 {
            return Err(format!(
                "jump at t={:.3}s found {} frame(s) already queued before its feed — \
                 the previous burst produced more frames than the protocol accounts for; \
                 the last-frame-is-the-target rule cannot be trusted",
                packet.pts_time, burst.stale_drained
            ));
        }
        if burst.frame.bytes.len() != frame_bytes {
            return Err(format!(
                "warm frame at t={:.3}s has {} bytes, expected {frame_bytes}",
                packet.pts_time,
                burst.frame.bytes.len()
            ));
        }
        warm_ms.push(burst.elapsed.as_secs_f64() * 1000.0);
        let warm_witness = fnv1a(&burst.frame.bytes);

        let (cold_elapsed, cold_frame) = cold_seek(ffmpeg, file, packet.pts_time)?;
        if cold_frame.len() != frame_bytes {
            return Err(format!(
                "cold frame at t={:.3}s has {} bytes, expected {frame_bytes}",
                packet.pts_time,
                cold_frame.len()
            ));
        }
        cold_ms.push(cold_elapsed.as_secs_f64() * 1000.0);
        let cold_witness = fnv1a(&cold_frame);
        let matched = warm_witness == cold_witness;
        if matched {
            matches += 1;
        }

        println!(
            "jump t={:.6}s warm={:.1}ms cold={:.1}ms frames_read={} witness={:08x} \
             cold_witness={:08x} match={}",
            packet.pts_time,
            burst.elapsed.as_secs_f64() * 1000.0,
            cold_elapsed.as_secs_f64() * 1000.0,
            burst.frames_read,
            warm_witness,
            cold_witness,
            if matched { "yes" } else { "NO" },
        );
        if !matched {
            return Err(format!(
                "pixel witness mismatch at t={:.3}s: warm {:08x} != cold {:08x} — \
                 the warm path is not delivering the same frame; do not trust these numbers",
                packet.pts_time, warm_witness, cold_witness
            ));
        }
    }

    let warm_p50 = percentile_f64(&warm_ms, 0.50);
    let warm_p95 = percentile_f64(&warm_ms, 0.95);
    let cold_p50 = percentile_f64(&cold_ms, 0.50);
    let cold_p95 = percentile_f64(&cold_ms, 0.95);
    let speedup = if warm_p50 > 0.0 {
        cold_p50 / warm_p50
    } else {
        0.0
    };
    println!(
        "summary jumps={} warm_p50={:.1}ms warm_p95={:.1}ms cold_p50={:.1}ms cold_p95={:.1}ms \
         speedup_p50={:.1}x matches={}/{}",
        chosen.len(),
        warm_p50,
        warm_p95,
        cold_p50,
        cold_p95,
        speedup,
        matches,
        chosen.len(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::percentile_f64;

    #[test]
    fn percentiles_are_boring_and_exact() {
        let values = [4.0, 1.0, 3.0, 2.0, 5.0];
        assert_eq!(percentile_f64(&values, 0.50), 3.0);
        assert_eq!(percentile_f64(&values, 0.95), 5.0);
        assert_eq!(percentile_f64(&[], 0.50), 0.0);
    }
}
