//! Integration test: the spike must decode a real generated clip and report
//! exact frame counts. Skips with an honest message when ffmpeg isn't
//! available on the machine — skipping is stated, never silent (Law 3).

use playback_spike::{generate_test_clip, locate_ffmpeg, probe_usable, run_pipeline, PipelineSpec};

#[test]
fn spike_decodes_generated_clip_exactly() {
    let ffmpeg = match locate_ffmpeg() {
        Ok(ff) => ff,
        Err(msg) => {
            println!("SKIPPED: ffmpeg not locatable ({msg})");
            return;
        }
    };
    // locate_ffmpeg's PATH fallback cannot prove existence; probe for real.
    if !probe_usable(&ffmpeg) {
        println!("SKIPPED: ffmpeg not usable on this machine — expected until tools/ build lands");
        return;
    }

    let clip = std::env::temp_dir().join("omniflux-spike-clip.mp4");
    generate_test_clip(&ffmpeg, &clip, 60, (1280, 720))
        .expect("ffmpeg must generate the deterministic test clip");

    let spec = PipelineSpec {
        input: clip.clone(),
        scale: (1280, 720),
        max_frames: Some(40),
    };
    let summary = run_pipeline(&spec, &ffmpeg).expect("spike decode must succeed");

    assert_eq!(summary.frames, 40, "max_frames cap must be exact");
    assert!(
        summary.time_to_first_frame_ms < 5_000.0,
        "first frame must arrive promptly (got {} ms)",
        summary.time_to_first_frame_ms
    );
    // Same input, same scale → same first-frame bytes. Determinism is the
    // pipeline's promise (the test asserts the witness isn't zero/degenerate).
    assert_ne!(summary.first_frame_fnv1a, 0, "frame 1 must be real bytes");

    // determinism: rerun and compare witnesses
    let summary2 = run_pipeline(&spec, &ffmpeg).expect("second decode must succeed");
    assert_eq!(
        summary.first_frame_fnv1a, summary2.first_frame_fnv1a,
        "two runs of the same input must witness identical frame-1 bytes"
    );

    let _ = std::fs::remove_file(&clip); // best-effort cleanup
}
