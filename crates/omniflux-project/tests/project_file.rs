//! Integration test for the `.omni` project file: a project must survive
//! being written to disk and read back EXACTLY, and a file that is broken
//! must be refused with a complaint that names the problem.

use std::path::{Path, PathBuf};

use omniflux_core::time::{Ticks, Timebase};
use omniflux_core::timeline::{Clip, ClipId, MediaId, Timeline, TrackKind};
use omniflux_project::{Project, ProjectError};

/// A folder for this test to work in, removed when the test ends.
fn scratch_folder(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("omniflux-project-test-{name}"));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).expect("the scratch folder must be creatable");
    folder
}

fn project_with_clips(clip_count: u32) -> Project {
    let mut timeline = Timeline::new(Timebase::default_timebase());
    timeline.add_media(MediaId(0), Ticks(90_000)).unwrap();
    let track = timeline.add_track(TrackKind::Video, "Video 1");
    for index in 0..clip_count {
        timeline
            .insert_clip(
                track,
                Clip {
                    id: ClipId(index),
                    media: MediaId(0),
                    start: Ticks(i64::from(index) * 1_000),
                    duration: Ticks(900),
                    source_in: Ticks(0),
                },
            )
            .expect("the clips must not overlap");
    }
    let mut project = Project::new(timeline);
    // An absolute path on every platform: "/footage/a.mp4" is NOT absolute
    // on Windows (no drive letter), so it would resolve against the project
    // folder there — and this test would mean two different things on two
    // operating systems.
    project.set_location(
        MediaId(0),
        std::env::temp_dir().join("footage").join("a.mp4"),
    );
    project
}

#[test]
fn a_project_survives_being_written_and_read_back() {
    let folder = scratch_folder("round-trip");
    let file = folder.join("holiday.omni");
    let project = project_with_clips(12);

    project.save_to(&file).expect("the project must be saved");
    let loaded = Project::load_from(&file).expect("the project must load back");

    assert_eq!(loaded.timeline(), project.timeline(), "the model survives");
    assert_eq!(loaded.locations(), project.locations(), "the files survive");
    assert!(
        !folder.join("holiday.omni-tmp").exists(),
        "no temporary file is left behind"
    );
}

#[test]
fn the_saved_file_is_text_a_person_can_read() {
    let folder = scratch_folder("readable");
    let file = folder.join("holiday.omni");
    project_with_clips(3)
        .save_to(&file)
        .expect("the project must be saved");

    let text = std::fs::read_to_string(&file).expect("the file must be readable");
    assert!(
        text.contains("omniflux-project"),
        "the file names its format"
    );
    assert!(
        text.contains("\"ticks_per_second\": 90000"),
        "the clock is in it"
    );
    assert!(
        text.contains("\"kind\": \"video\""),
        "the track kind is in it"
    );
}

#[test]
fn a_file_that_is_not_a_project_is_refused() {
    let folder = scratch_folder("not-a-project");
    let file = folder.join("broken.omni");
    std::fs::write(&file, "{ \"format\": \"something-else\", \"version\": 1 }")
        .expect("the file must be writable");

    let error = Project::load_from(&file).expect_err("a foreign file must be refused");
    assert!(matches!(error, ProjectError::Malformed { .. }));
    assert!(
        error.to_string().contains("omniflux-project"),
        "the complaint says what was expected: {error}"
    );
}

#[test]
fn a_file_with_clips_that_overlap_is_refused() {
    let folder = scratch_folder("overlap");
    let file = folder.join("overlap.omni");
    let text = r#"{
      "format": "omniflux-project",
      "version": 1,
      "timebase": { "ticks_per_second": 90000 },
      "media": [ { "id": 0, "duration": 90000 } ],
      "tracks": [
        { "id": 0, "kind": "video", "name": "Video 1",
          "clips": [
            { "id": 0, "media": 0, "start": 0, "duration": 1000, "source_in": 0 },
            { "id": 1, "media": 0, "start": 500, "duration": 1000, "source_in": 0 }
          ] }
      ]
    }"#;
    std::fs::write(&file, text).expect("the file must be writable");

    let error = Project::load_from(&file).expect_err("overlapping clips must be refused");
    assert!(matches!(error, ProjectError::Model { .. }));
}

#[test]
fn a_media_file_inside_the_project_folder_is_stored_relatively() {
    let folder = scratch_folder("relative");
    std::fs::create_dir_all(folder.join("footage")).expect("the media folder must be creatable");
    let file = folder.join("holiday.omni");

    let mut project = project_with_clips(2);
    project.set_location(MediaId(0), folder.join("footage/a.mp4"));
    project.save_to(&file).expect("the project must be saved");

    let text = std::fs::read_to_string(&file).expect("the file must be readable");
    assert!(
        text.contains("footage/a.mp4") || text.contains("footage\\\\a.mp4"),
        "the media path is relative to the project: {text}"
    );

    let loaded = Project::load_from(&file).expect("the project must load back");
    let location = loaded
        .location(MediaId(0))
        .expect("the media must be located");
    assert_eq!(
        location.path,
        folder.join("footage/a.mp4"),
        "the relative path resolves back to the same file"
    );
}

#[test]
fn a_missing_file_says_which_file_is_missing() {
    let error = Project::load_from(Path::new("no-such-project.omni"))
        .expect_err("a missing file must be refused");
    assert!(matches!(error, ProjectError::Io { .. }));
    assert!(
        error.to_string().contains("no-such-project.omni"),
        "the complaint names the file: {error}"
    );
}
