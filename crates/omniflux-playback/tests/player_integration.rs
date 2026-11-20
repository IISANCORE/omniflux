//! Integration test for the player: it must decode the frame that is really
//! there, not merely *a* frame.
//!
//! The pixel witness is the point. A seek that returns a frame from the wrong
//! place still looks like success — right size, plausible image — and only a
//! comparison against an independent decode catches it. The reference is a
//! cold `ffmpeg -ss` run, exactly as the seek spike does.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use omniflux_playback::player::Player;
use omniflux_playback::{fnv1a, locate_ffmpeg, probe_usable};

/// A clip whose GOP is short enough to keep the test quick but long enough
/// that a mid-GOP seek really has frames to decode.
const CLIP_FRAMES: u32 = 90;
const CLIP_GOP: u32 = 30;
const CLIP_SIZE: (u32, u32) = (320, 180);

fn make_clip(ffmpeg: &Path, path: &Path) {
    let status = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc2=size={}x{}:rate=30", CLIP_SIZE.0, CLIP_SIZE.1),
            "-frames:v",
            &CLIP_FRAMES.to_string(),
            "-c:v",
            "libx264",
            "-crf",
            "28",
            "-g",
            &CLIP_GOP.to_string(),
            "-keyint_min",
            &CLIP_GOP.to_string(),
            "-sc_threshold",
            "0",
            "-y",
            &path.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("ffmpeg must run");
    assert!(status.success(), "could not generate the test clip");
}

/// The reference: one fresh ffmpeg process asked for the frame at `seconds`.
fn cold_frame(ffmpeg: &Path, clip: &Path, seconds: f64) -> Vec<u8> {
    let output = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-ss",
            &format!("{seconds:.6}"),
            "-i",
            &clip.to_string_lossy(),
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
        .expect("the reference decode must run");
    assert!(
        output.status.success() && !output.stdout.is_empty(),
        "the reference decode produced no frame at {seconds:.3}s: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn clip_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("omniflux-player-{name}.mp4"))
}

#[test]
fn a_mid_gop_seek_returns_the_frame_that_belongs_there() {
    let ffmpeg = match locate_ffmpeg() {
        Ok(path) => path,
        Err(message) => {
            println!("SKIPPED: ffmpeg not locatable ({message})");
            return;
        }
    };
    if !probe_usable(&ffmpeg) {
        println!("SKIPPED: ffmpeg not usable on this machine");
        return;
    }

    let clip = clip_path("mid-gop");
    make_clip(&ffmpeg, &clip);

    let (mut player, first) = Player::open(&ffmpeg, &clip).expect("the player must open");
    let info = player.info();
    assert_eq!((info.width, info.height), CLIP_SIZE);
    assert_eq!(first.bytes.len(), (CLIP_SIZE.0 * CLIP_SIZE.1 * 3) as usize);

    // A keyframe seek is the cheap case; a seek 0.2 s into a GOP has to
    // decode its way there, and must say so in its packet count.
    let on_keyframe = player.seek(1.0).expect("the keyframe seek must succeed");
    let mid_gop = player.seek(1.2).expect("the seek must succeed");
    assert_eq!(
        mid_gop.bytes.len(),
        (CLIP_SIZE.0 * CLIP_SIZE.1 * 3) as usize,
        "a frame is exactly width*height*3 bytes"
    );
    assert!(
        mid_gop.packets_decoded > on_keyframe.packets_decoded,
        "a mid-GOP seek must decode more than a keyframe seek ({} vs {})",
        mid_gop.packets_decoded,
        on_keyframe.packets_decoded
    );

    let reference = cold_frame(&ffmpeg, &clip, mid_gop.time);
    assert_eq!(
        fnv1a(&mid_gop.bytes),
        fnv1a(&reference),
        "the player's frame at {:.3}s must be pixel-identical to a cold decode of that time",
        mid_gop.time
    );

    let _ = std::fs::remove_file(&clip);
}

#[test]
fn stepping_forward_walks_one_frame_at_a_time() {
    let ffmpeg = match locate_ffmpeg() {
        Ok(path) => path,
        Err(message) => {
            println!("SKIPPED: ffmpeg not locatable ({message})");
            return;
        }
    };
    if !probe_usable(&ffmpeg) {
        println!("SKIPPED: ffmpeg not usable on this machine");
        return;
    }

    let clip = clip_path("stepping");
    make_clip(&ffmpeg, &clip);

    let (mut player, _) = Player::open(&ffmpeg, &clip).expect("the player must open");
    let start = player.seek(0.5).expect("the seek must succeed");
    let expected = player.frame_duration();

    // A seek decodes the frames that follow its target and keeps them, so
    // the first steps can be free. Every step must still advance exactly one
    // frame, cost at most one packet, and stepping must eventually reach the
    // live decoder rather than only replaying the seek's frames.
    let mut last = start.time;
    let mut live_steps = 0usize;
    let limit = player.info().reorder_depth + 2;
    // ffprobe prints timestamps in microseconds, so consecutive gaps wobble
    // by a microsecond or two. A tenth of a frame is far below the bug this
    // test exists to catch (a step of five frames) and far above the noise.
    let tolerance = expected / 10.0;
    for step_number in 0..limit {
        let next = player
            .next_frame()
            .expect("stepping must succeed")
            .expect("there are frames after 0.5 s");
        let step = next.time - last;
        assert!(
            (step - expected).abs() < tolerance,
            "step {step_number} must advance one frame ({expected:.6}s), measured {step:.6}s"
        );
        assert!(
            next.packets_decoded <= 1,
            "step {step_number} costs at most one packet, measured {}",
            next.packets_decoded
        );
        if next.packets_decoded == 1 {
            live_steps += 1;
        }
        last = next.time;
    }
    assert!(
        live_steps > 0,
        "stepping must reach the live decoder, not just replay the seek's frames"
    );

    let _ = std::fs::remove_file(&clip);
}
