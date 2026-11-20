//! The warm-vs-cold seek experiment — the measurement that decided the
//! engine's decode design, kept runnable so the decision can be re-checked on
//! any machine and any footage.
//!
//! The decoder it measures lives in `omniflux-playback` (the real engine
//! crate). This file only drives it: pick keyframes, feed them, time each
//! jump, and compare every warm frame against the cold spawn-per-jump
//! baseline pixel-for-pixel. A performance claim whose frames do not match
//! the reference is not a performance claim (Law 6).
//!
//! Re-exported for the integration test, which drives the same decoder the
//! app does: [`WarmDecoder`], [`build_feed`].

pub use omniflux_playback::warm::{build_feed, Burst, Frame, WarmDecoder};

use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::fnv1a;
use crate::index::{build_index, locate_ffprobe};

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
