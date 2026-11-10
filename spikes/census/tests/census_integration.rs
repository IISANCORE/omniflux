//! Integration test for the RAM census: the census must run on a real
//! machine and report a picture that makes sense. A census that measures
//! nothing, or measures a decoder that has not started working yet, is
//! worse than no census — so these assertions are about the MEANING of the
//! numbers, not just their presence.

use census_spike::run_census;
use playback_spike::{locate_ffmpeg, probe_usable};

#[test]
fn the_census_reports_the_engine_footprint() {
    let ffmpeg = match locate_ffmpeg() {
        Ok(path) => path,
        Err(message) => {
            println!("SKIPPED: ffmpeg not locatable ({message})");
            return;
        }
    };
    if !probe_usable(&ffmpeg) {
        println!("SKIPPED: ffmpeg not usable on this machine — the census needs it");
        return;
    }

    let census = run_census(&ffmpeg).expect("the census must run");

    // The arrangement must stay cheap: a 500-clip project with 5 000 edits
    // of history is well under 100 MB, or the model is not the cheap part.
    if let Some(bytes) = census.model_and_history_bytes {
        assert!(
            bytes < 100 * 1024 * 1024,
            "the project and its history must stay small, measured {} bytes",
            bytes
        );
    }

    // Full resolution must cost MORE memory than proxy resolution — if it
    // does not, the decoder was sampled before it started working and the
    // number is a lie.
    if let (Some(full), Some(proxy)) = (census.decoder_full_bytes, census.decoder_proxy_bytes) {
        assert!(
            full > proxy,
            "a 1080p decoder must cost more than a 640x360 one (measured {full} vs {proxy})"
        );
    }

    // A queued frame costs what its size says.
    assert_eq!(census.full_frame_bytes, 6_220_800);
    assert_eq!(census.proxy_frame_bytes, 691_200);

    println!(
        "own_empty={:?} own_with_project={:?} model={:?} decoder_1080p={:?} decoder_proxy={:?} \
         file={} bytes",
        census.own_empty_bytes,
        census.own_with_project_bytes,
        census.model_and_history_bytes,
        census.decoder_full_bytes,
        census.decoder_proxy_bytes,
        census.project_file_bytes
    );
}
