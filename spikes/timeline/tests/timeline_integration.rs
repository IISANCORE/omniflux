//! Integration test for the timeline spike: the command history must put the
//! project back exactly after every edit is undone — checked at the scale
//! the engine will actually see, not only in unit tests with three clips.

use timeline_spike::{run_spike, SpikeConfig};

#[test]
fn undoing_every_edit_returns_the_project_exactly() {
    let config = SpikeConfig {
        clips: 200,
        edits: 500,
        seed: 2026,
    };
    let report = run_spike(&config).expect("the spike must run");

    assert!(
        report.undo_restored_project,
        "undoing every edit must restore the project exactly"
    );
    assert!(
        report.redo_restored_project,
        "redoing every edit must restore the project exactly"
    );
    assert_eq!(
        report.edits_refused, 0,
        "the harness only makes legal edits; a refusal means the model and \
         the harness disagree"
    );
    assert!(
        report.edit_p50_ms < 16.0,
        "an edit must stay far below a frame (16 ms); measured p50 {} ms",
        report.edit_p50_ms
    );

    println!(
        "clips={} edits={} edit_p50={:.3}ms undo_p50={:.3}ms redo_p50={:.3}ms \
         bytes_per_undo_entry={:?} bytes_per_clip={:?}",
        report.clips,
        report.edits,
        report.edit_p50_ms,
        report.undo_p50_ms,
        report.redo_p50_ms,
        report.bytes_per_undo_entry,
        report.bytes_per_clip
    );
}
