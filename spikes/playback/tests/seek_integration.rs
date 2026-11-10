//! Integration test for the warm-seek spike: one long-lived decoder must
//! deliver keyframes whose pixels are IDENTICAL to a fresh cold ffmpeg seek
//! at the same timestamps — and the run must print its real latency numbers.
//! Skips with an honest message when ffmpeg/ffprobe are unavailable (stated,
//! never silent — Law 3).

use std::fs::File;

use playback_spike::index::{build_index, locate_ffprobe};
use playback_spike::warm::{build_feed, cold_seek, WarmDecoder};
use playback_spike::{fnv1a, generate_test_clip_gop, locate_ffmpeg, probe_usable};

#[test]
fn warm_decoder_keyframe_jumps_match_cold_pixels() {
    let ffmpeg = match locate_ffmpeg() {
        Ok(ff) => ff,
        Err(msg) => {
            println!("SKIPPED: ffmpeg not locatable ({msg})");
            return;
        }
    };
    if !probe_usable(&ffmpeg) {
        println!("SKIPPED: ffmpeg not usable on this machine — expected until tools/ build lands");
        return;
    }
    let ffprobe = match locate_ffprobe(&ffmpeg) {
        Ok(p) => p,
        Err(msg) => {
            println!("SKIPPED: ffprobe not locatable ({msg})");
            return;
        }
    };
    if !probe_usable(&ffprobe) {
        println!("SKIPPED: ffprobe not usable on this machine — seek spike needs it for the index");
        return;
    }

    // 60 frames @ 30 fps, keyframe every 10 frames → 6 dense checkpoints.
    let clip = std::env::temp_dir().join("omniflux-seek-clip.mp4");
    generate_test_clip_gop(&ffmpeg, &clip, 60, (1280, 720), Some(10))
        .expect("ffmpeg must generate the deterministic seek clip");

    let index = build_index(&ffprobe, &clip).expect("ffprobe must build the jump index");
    assert_eq!((index.width, index.height), (1280, 720));
    let reorder = index.has_b_frames as usize;
    let feed_packets = reorder + 1;
    println!("index: reorder_depth={reorder} feed_packets={feed_packets}");

    // A keyframe needs `reorder` packets after it for the protocol's push.
    let usable: Vec<usize> = index
        .keyframe_indices()
        .into_iter()
        .filter(|&i| i + reorder < index.packets.len())
        .collect();
    assert!(
        usable.len() >= 3,
        "dense-GOP clip must expose several jumpable keyframes (got {})",
        usable.len()
    );

    let frame_bytes = index.width as usize * index.height as usize * 3;
    let mut warm = WarmDecoder::spawn(&ffmpeg, index.width, index.height, index.has_b_frames)
        .expect("the warm decoder process must start");
    let mut file = File::open(&clip).expect("the clip we just wrote must open");

    // Warm-up feeds the first keyframe + its followers and reads one frame;
    // after it the decoder holds `reorder` frames back.
    let warmup_feed = build_feed(&mut file, &index, usable[0], feed_packets)
        .expect("the warm-up feed must build from indexed packets");
    let warmup = warm
        .warm_up(&warmup_feed)
        .expect("the warm-up feed must deliver one frame");
    assert_eq!(warmup.frames_read, 1);
    assert_eq!(warmup.frame.bytes.len(), frame_bytes);
    assert_eq!(warmup.stale_drained, 0);

    let (_, warmup_cold) = cold_seek(&ffmpeg, &clip, index.packets[usable[0]].pts_time)
        .expect("cold seek at the warm-up keyframe must deliver a frame");
    assert_eq!(
        fnv1a(&warmup.frame.bytes),
        fnv1a(&warmup_cold),
        "warm-up frame must be pixel-identical to a cold seek"
    );
    println!(
        "warmup t={:.3}s warm={:.1}ms witness={:08x}",
        index.packets[usable[0]].pts_time,
        warmup.elapsed.as_secs_f64() * 1000.0,
        fnv1a(&warmup.frame.bytes)
    );

    let mut picks: Vec<usize> = Vec::new();
    for pos in [1usize, usable.len() / 2, usable.len() - 1] {
        if !picks.contains(&pos) {
            picks.push(pos);
        }
    }

    for pos in picks {
        let kf_idx = usable[pos];
        let packet = &index.packets[kf_idx];
        let feed = build_feed(&mut file, &index, kf_idx, feed_packets)
            .expect("a jump feed must build from indexed packets");

        let burst = warm
            .jump(&feed)
            .expect("a warm jump must deliver its whole burst");
        assert_eq!(burst.frames_read, feed_packets);
        assert_eq!(burst.stale_drained, 0);
        assert_eq!(burst.frame.bytes.len(), frame_bytes);
        let warm_witness = fnv1a(&burst.frame.bytes);

        let (cold_elapsed, cold_frame) =
            cold_seek(&ffmpeg, &clip, packet.pts_time).expect("cold seek must deliver a frame");
        assert_eq!(cold_frame.len(), frame_bytes);
        let cold_witness = fnv1a(&cold_frame);

        // The core promise: the warm path is not a lookalike — same decoder,
        // same bytes in, byte-identical pixels out.
        assert_eq!(
            warm_witness, cold_witness,
            "warm and cold decodes must be pixel-identical at t={:.3}s",
            packet.pts_time
        );
        assert_ne!(warm_witness, 0, "a frame of real pixels cannot hash to 0");
        println!(
            "kf#{} t={:.3}s warm={:.1}ms cold={:.1}ms frames_read={} witness={:08x}",
            kf_idx,
            packet.pts_time,
            burst.elapsed.as_secs_f64() * 1000.0,
            cold_elapsed.as_secs_f64() * 1000.0,
            burst.frames_read,
            warm_witness
        );
    }

    drop(warm); // shut the decoder down before deleting the clip
    let _ = std::fs::remove_file(&clip); // best-effort cleanup
}

/// The decoder must finish on its own when its input closes — no kill
/// needed. This is the mechanism the shutdown relies on, and it is the
/// mechanism that survives a WRAPPER process: on Windows CI, the ffmpeg on
/// PATH is Chocolatey's shim, and killing the shim leaves the real ffmpeg
/// alive behind it, still holding the decoder's stdout pipe open. That
/// topology hung the first CI run for 15 minutes; closing the input is what
/// reaches the real binary, whatever sits in front of it.
#[test]
fn the_decoder_exits_when_its_input_closes() {
    let ffmpeg = match locate_ffmpeg() {
        Ok(ff) => ff,
        Err(msg) => {
            println!("SKIPPED: ffmpeg not locatable ({msg})");
            return;
        }
    };
    if !probe_usable(&ffmpeg) {
        println!("SKIPPED: ffmpeg not usable on this machine — expected until tools/ build lands");
        return;
    }
    let ffprobe = match locate_ffprobe(&ffmpeg) {
        Ok(p) => p,
        Err(msg) => {
            println!("SKIPPED: ffprobe not locatable ({msg})");
            return;
        }
    };
    if !probe_usable(&ffprobe) {
        println!("SKIPPED: ffprobe not usable on this machine — seek spike needs it for the index");
        return;
    }

    let clip = std::env::temp_dir().join("omniflux-shutdown-clip.mp4");
    generate_test_clip_gop(&ffmpeg, &clip, 30, (320, 180), Some(10))
        .expect("ffmpeg must generate the deterministic clip");

    let index = build_index(&ffprobe, &clip).expect("ffprobe must build the jump index");
    let reorder = index.has_b_frames as usize;
    let keyframe = index
        .keyframe_indices()
        .first()
        .copied()
        .expect("the generated clip must have a keyframe");

    let mut warm = WarmDecoder::spawn(&ffmpeg, index.width, index.height, index.has_b_frames)
        .expect("the warm decoder process must start");
    let mut file = File::open(&clip).expect("the clip we just wrote must open");
    let feed = build_feed(&mut file, &index, keyframe, reorder + 1)
        .expect("a feed must build from indexed packets");

    // One feed proves the decoder is alive and decoding (it waits for more
    // input afterwards) — that is the state in which EOF must end it.
    let burst = warm
        .warm_up(&feed)
        .expect("the warm-up feed must deliver one frame");
    assert_eq!(burst.frames_read, 1);
    assert!(
        !warm.has_exited(),
        "the decoder must still be running while it waits for more input"
    );

    warm.close_input();

    // Bounded wait for an external process to notice EOF and leave. The
    // deadline only bounds the FAILURE case; the outcome itself is binary.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !warm.has_exited() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        warm.has_exited(),
        "the decoder must exit by itself once its input closes — shutdown \
         depends on EOF, not on killing the process (a wrapper process would \
         survive the kill)"
    );

    drop(warm);
    let _ = std::fs::remove_file(&clip); // best-effort cleanup
}
