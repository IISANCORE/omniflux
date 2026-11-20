//! Warm decoder: keep ONE ffmpeg process alive and hand it a fresh slice of
//! the stream over stdin for every seek, instead of paying process spawn +
//! file open on every jump. The playback spike measured the difference
//! (96.6 ms cold vs 17.8 ms warm at 1080p, pixel-identical); this module is
//! the part that survived the experiment.
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
use crate::index::{MediaIndex, Packet};
use crate::read_stderr_tail;

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

    /// Closes the decoder's input. The decoder sees EOF and finishes on its
    /// own — the one signal that reaches the real binary even when a wrapper
    /// process stands in front of it (Chocolatey's shim, for one). Closing
    /// twice is harmless.
    pub fn close_input(&mut self) {
        self.stdin = None;
    }

    /// True once the decoder process has exited on its own.
    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
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
        self.close_input();
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

/// Reads packets `[start, start + count)` and assembles one self-contained
/// decoder feed: parameter sets + AUD + Annex-B sample for every packet,
/// plus a trailing AUD to close the final access unit. Shared by the seek
/// spike and the app, so both feed byte-identical data.
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
