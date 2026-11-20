//! The player: a decoder you can ask for the frame at a moment in time.
//!
//! One job: hold one open media file and answer "what does it look like at
//! `t`?" and "what comes next?". Not responsible for: which moment the user
//! wants (the timeline says that), or how pixels reach a screen (the window
//! says that).
//!
//! Two ways to ask, and both use the same persistent decoder:
//! - [`Player::seek`] — jump anywhere. It feeds the stream from the nearest
//!   keyframe up to the target, so the cost grows with the distance from that
//!   keyframe. On dense-keyframe footage (proxies) that is a few frames; on a
//!   camera original with 8-second GOPs it can be hundreds. That is the honest
//!   cost of an exact frame, and it is why proxies exist.
//! - [`Player::next_frame`] — step forward one frame. Usually one packet of
//!   work; frames the previous seek already decoded are handed over for free.
//!   Playback is this in a loop.
//!
//! The protocol is the one the seek spike proved (see [`crate::warm`]): the
//! decoder holds `L = has_b_frames` frames for reordering and emits in
//! PRESENTATION order. Feeding `N` packets produces `N` frames — L stale
//! frames from before the feed, then the feed's frames in presentation order.
//! A seek therefore feeds up to `target + L` and picks the target by its
//! presentation rank among the feed, not by "the last frame out".
//!
//! Known limit, stated plainly: the final `L` frames of a file cannot be
//! decoded by this protocol, because the decoder only releases them when its
//! input ends — and ending the input would end the persistent decoder. Seeks
//! clamp to the last reachable packet; at 30 fps that is the last ~67 ms.

use std::collections::VecDeque;
use std::fs::File;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::index::{build_index, locate_ffprobe, MediaIndex};
use crate::warm::{build_feed, Frame, WarmDecoder};

/// Everything the window needs to know about an open file before it has
/// decoded anything.
#[derive(Debug, Clone)]
pub struct PlayerInfo {
    pub width: u32,
    pub height: u32,
    /// Presentation length of the file, seconds.
    pub duration: f64,
    /// How long one frame is shown, seconds (the median packet spacing).
    pub frame_duration: f64,
    /// The decoder's reorder hold (`has_b_frames`): frames it keeps back
    /// before releasing them in presentation order.
    pub reorder_depth: usize,
    pub packets: usize,
    pub keyframes: usize,
}

/// One decoded frame, with the facts needed to display and to measure it.
pub struct DecodedFrame {
    /// Exactly `width * height * 3` bytes of rgb24.
    pub bytes: Vec<u8>,
    /// The presentation time this frame represents, seconds.
    pub time: f64,
    /// Wall time spent producing it (feed + decode + pipe).
    pub latency: Duration,
    /// Packets fed to the decoder for this frame: zero when a previous seek
    /// already decoded it, one for a live sequential step, more when jumping
    /// inside a GOP.
    pub packets_decoded: usize,
}

impl DecodedFrame {
    fn from_burst(burst: Frame, time: f64, latency: Duration, packets_decoded: usize) -> Self {
        DecodedFrame {
            bytes: burst.bytes,
            time,
            latency,
            packets_decoded,
        }
    }
}

/// One open file and the decoder attached to it.
pub struct Player {
    index: MediaIndex,
    decoder: WarmDecoder,
    file: File,
    reorder_depth: usize,
    /// The last packet index written to the decoder. The next sequential
    /// frame comes from the packet after it.
    fed_through: usize,
    frame_duration: f64,
    /// Frames the last seek already decoded and that follow its target, in
    /// presentation order. Stepping into them costs nothing.
    pending: VecDeque<Frame>,
    /// Every presentation time in the file, sorted: the film's presentation
    /// order. With B-frames this is not the packet order.
    pts_sorted: Vec<f64>,
    /// Index into `pts_sorted` of the frame currently on screen.
    presentation_index: usize,
}

impl Player {
    /// Opens a file and returns the player plus the frame it starts on.
    ///
    /// Opening pays the one-time costs: `ffprobe` builds the packet map,
    /// ffmpeg spawns, and the decoder is warmed up on the first keyframe.
    /// After this returns, every seek is steady-state work.
    pub fn open(ffmpeg: &Path, media: &Path) -> Result<(Player, DecodedFrame), String> {
        let ffprobe = locate_ffprobe(ffmpeg)?;
        let index = build_index(&ffprobe, media)?;
        let reorder_depth = index.has_b_frames as usize;

        // The protocol needs at least one packet before the target and L
        // after it. A file this short is not video in any useful sense.
        if index.packets.len() <= reorder_depth + 1 {
            return Err(format!(
                "{} holds {} packets, too few to decode with a reorder depth of {reorder_depth}",
                media.display(),
                index.packets.len()
            ));
        }

        let mut pts_sorted: Vec<f64> = index.packets.iter().map(|packet| packet.pts_time).collect();
        pts_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let file =
            File::open(media).map_err(|e| format!("could not open {}: {e}", media.display()))?;
        let decoder = WarmDecoder::spawn(ffmpeg, index.width, index.height, index.has_b_frames)?;
        let frame_duration = index.frame_duration();
        let first_keyframe = index.keyframe_indices().first().copied().unwrap_or(0);

        let mut player = Player {
            index,
            decoder,
            file,
            reorder_depth,
            fed_through: first_keyframe + reorder_depth,
            frame_duration,
            pending: VecDeque::new(),
            pts_sorted,
            presentation_index: 0,
        };

        // Warm-up: the first feed of L+1 packets makes the decoder emit
        // exactly one frame — the keyframe. From here on it holds L frames,
        // which is the steady state every seek assumes.
        let feed = build_feed(
            &mut player.file,
            &player.index,
            first_keyframe,
            reorder_depth + 1,
        )?;
        let started = Instant::now();
        let burst = player.decoder.warm_up(&feed)?;
        let first_time = player.index.packets[first_keyframe].pts_time;
        player.presentation_index = player.presentation_rank(first_time);
        let first = DecodedFrame::from_burst(
            burst.frame,
            first_time,
            started.elapsed(),
            reorder_depth + 1,
        );
        Ok((player, first))
    }

    pub fn info(&self) -> PlayerInfo {
        PlayerInfo {
            width: self.index.width,
            height: self.index.height,
            duration: self.index.duration_seconds(),
            frame_duration: self.frame_duration,
            reorder_depth: self.reorder_depth,
            packets: self.index.packets.len(),
            keyframes: self.index.keyframe_indices().len(),
        }
    }

    pub fn frame_duration(&self) -> f64 {
        self.frame_duration
    }

    /// The frame on screen at `seconds`, decoded exactly.
    ///
    /// The seek target is clamped to the last packet this protocol can
    /// release (see the module comment); the returned frame's `time` is the
    /// real presentation time of what came back, so the caller never has to
    /// guess where it landed.
    pub fn seek(&mut self, seconds: f64) -> Result<DecodedFrame, String> {
        let last_reachable = self.index.packets.len() - 1 - self.reorder_depth;
        let target = self.index.packet_at(seconds).min(last_reachable);
        let keyframe = self.index.keyframe_at_or_before(target);
        let target_pts = self.index.packets[target].pts_time;

        // Feed the run from the keyframe through target + L. The decoder
        // emits in presentation order, so the target is not the last frame
        // of the burst: `rank` frames of this feed are shown before it, and
        // the burst opens with L stale frames from the previous feed. The
        // target's output rank is therefore rank + L. Frames after it are
        // the film's next frames and are kept for stepping.
        let feed_end = target + self.reorder_depth;
        let packets = feed_end - keyframe + 1;
        let rank = self.index.packets[keyframe..=feed_end]
            .iter()
            .filter(|packet| packet.pts_time < target_pts)
            .count();
        let keep_from = rank + self.reorder_depth;
        if keep_from >= packets {
            return Err(format!(
                "seek to {seconds:.3}s maps to output rank {keep_from} of a {packets}-packet \
                 feed; the packet map and the decoder's reorder depth disagree"
            ));
        }
        let feed = build_feed(&mut self.file, &self.index, keyframe, packets)?;

        let started = Instant::now();
        let burst = self.decoder.feed_collect(&feed, packets, keep_from)?;
        let elapsed = started.elapsed();
        if burst.stale_drained != 0 {
            return Err(format!(
                "seek to {seconds:.3}s found {} frame(s) already queued before its feed — \
                 the previous feed produced more frames than the protocol accounts for",
                burst.stale_drained
            ));
        }
        self.fed_through = feed_end;

        let mut frames = burst.frames.into_iter();
        let frame = frames.next().ok_or_else(|| {
            format!(
                "seek to {seconds:.3}s asked for output rank {keep_from} of {packets} and the \
                 decoder returned nothing"
            )
        })?;
        self.pending = frames.collect();
        self.presentation_index = self.presentation_rank(target_pts);
        Ok(DecodedFrame::from_burst(
            frame, target_pts, elapsed, packets,
        ))
    }

    /// The next frame in presentation order, or `None` at the end.
    ///
    /// The frames a seek already decoded come first and cost nothing; after
    /// that this is one packet in, one frame out — what playback loops on.
    pub fn next_frame(&mut self) -> Result<Option<DecodedFrame>, String> {
        if let Some(frame) = self.pending.pop_front() {
            self.presentation_index += 1;
            let time = self.pts_sorted[self.presentation_index];
            return Ok(Some(DecodedFrame::from_burst(
                frame,
                time,
                Duration::ZERO,
                0,
            )));
        }

        let next_packet = self.fed_through + 1;
        // The final L frames stay in the decoder's reorder hold until its
        // input ends, and ending the input would end the persistent decoder.
        if next_packet >= self.index.packets.len()
            || self.presentation_index + 1 >= self.pts_sorted.len()
        {
            return Ok(None);
        }

        let feed = build_feed(&mut self.file, &self.index, next_packet, 1)?;
        let started = Instant::now();
        let burst = self.decoder.feed(&feed, 1)?;
        let elapsed = started.elapsed();
        self.fed_through = next_packet;
        self.presentation_index += 1;

        Ok(Some(DecodedFrame::from_burst(
            burst.frame,
            self.pts_sorted[self.presentation_index],
            elapsed,
            1,
        )))
    }

    /// How many frames are shown before `pts`: its position in the film's
    /// presentation order.
    fn presentation_rank(&self, pts: f64) -> usize {
        self.pts_sorted
            .partition_point(|&candidate| candidate < pts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decoded_frame_keeps_its_own_time_and_cost() {
        let frame = DecodedFrame::from_burst(
            Frame {
                arrived: Instant::now(),
                bytes: vec![0; 12],
            },
            3.5,
            Duration::from_millis(18),
            42,
        );
        assert_eq!(frame.time, 3.5);
        assert_eq!(frame.packets_decoded, 42);
        assert_eq!(frame.bytes.len(), 12);
    }
}
