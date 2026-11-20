//! The edits the prototype's buttons perform, built as engine commands.
//!
//! One job: turn "split here" and "delete this" into the commands the core
//! history applies, and say plainly why an edit cannot happen. Not
//! responsible for: applying them (the history does), or for drawing them
//! (the timeline view does).
//!
//! Keeping this out of the window code means the rules ("the cut must be
//! strictly inside the clip", "ids must be unique") are unit-tested without
//! opening a window.

use omniflux_core::edit::{Command, RemoveClip, SplitClip};
use omniflux_core::time::Ticks;
use omniflux_core::timeline::{ClipId, Timeline};

/// The smallest clip id not currently in use.
///
/// Scanning is cheap at prototype scale and survives undo/redo: an id that
/// was undone is free again, and the next edit can use it without colliding
/// with anything on screen.
pub fn next_clip_id(timeline: &Timeline) -> ClipId {
    let highest = timeline
        .tracks()
        .iter()
        .flat_map(|track| track.clips().iter())
        .map(|clip| clip.id.0)
        .max();
    ClipId(highest.map_or(0, |id| id + 1))
}

/// A split, plus the id its right-hand half will get.
pub struct SplitPlan {
    pub command: Box<dyn Command>,
    pub right: ClipId,
}

/// The command that cuts `clip` at `at`.
///
/// A cut exactly on a clip boundary is refused rather than silently
/// producing a zero-length clip: the user asked for a cut, and a cut that
/// changes nothing is a bug to show, not an edit to record.
pub fn split_command(timeline: &Timeline, clip: ClipId, at: Ticks) -> Result<SplitPlan, String> {
    let Some(found) = timeline.clip(clip) else {
        return Err(format!("clip {} is not on the timeline", clip.0));
    };
    if at <= found.start || at >= found.end() {
        return Err(format!(
            "the playhead must be strictly inside clip {} ({} to {})",
            clip.0,
            found.start.0,
            found.end().0
        ));
    }
    let right = next_clip_id(timeline);
    Ok(SplitPlan {
        command: Box::new(SplitClip { clip, at, right }),
        right,
    })
}

/// The command that removes `clip` from whichever track holds it.
pub fn delete_command(timeline: &Timeline, clip: ClipId) -> Result<Box<dyn Command>, String> {
    let Some(track) = timeline.track_of(clip) else {
        return Err(format!("clip {} is not on the timeline", clip.0));
    };
    Ok(Box::new(RemoveClip { track, clip }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniflux_core::time::Timebase;
    use omniflux_core::timeline::{Clip, MediaId, TrackId, TrackKind};

    fn project() -> (Timeline, TrackId) {
        let mut timeline = Timeline::new(Timebase::default_timebase());
        timeline.add_media(MediaId(1), Ticks(90_000)).unwrap();
        let track = timeline.add_track(TrackKind::Video, "Video 1");
        (timeline, track)
    }

    fn clip(id: u32, start: i64, duration: i64) -> Clip {
        Clip {
            id: ClipId(id),
            media: MediaId(1),
            start: Ticks(start),
            duration: Ticks(duration),
            source_in: Ticks(start),
        }
    }

    #[test]
    fn a_new_id_is_one_past_the_highest_in_use() {
        let (mut timeline, track) = project();
        assert_eq!(next_clip_id(&timeline), ClipId(0));
        timeline.insert_clip(track, clip(1, 0, 1000)).unwrap();
        timeline.insert_clip(track, clip(7, 1000, 1000)).unwrap();
        assert_eq!(next_clip_id(&timeline), ClipId(8));
    }

    #[test]
    fn a_split_inside_a_clip_produces_two_clips_at_the_cut() {
        let (mut timeline, track) = project();
        timeline.insert_clip(track, clip(1, 0, 90_000)).unwrap();

        let plan = split_command(&timeline, ClipId(1), Ticks(45_000))
            .expect("a cut in the middle must be allowed");
        assert_eq!(plan.right, ClipId(2));
        plan.command
            .apply(&mut timeline)
            .expect("the split must apply");

        let clips = timeline.tracks()[0].clips();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].end(), Ticks(45_000));
        assert_eq!(clips[1].start, Ticks(45_000));
        assert_eq!(clips[1].source_in, Ticks(45_000));
    }

    #[test]
    fn a_split_on_a_boundary_is_refused_with_a_reason() {
        let (mut timeline, track) = project();
        timeline.insert_clip(track, clip(1, 0, 90_000)).unwrap();

        assert!(split_command(&timeline, ClipId(1), Ticks(0)).is_err());
        assert!(split_command(&timeline, ClipId(1), Ticks(90_000)).is_err());
        assert!(split_command(&timeline, ClipId(99), Ticks(1)).is_err());
        assert_eq!(timeline.tracks()[0].clips().len(), 1);
    }

    #[test]
    fn deleting_the_selected_clip_removes_it() {
        let (mut timeline, track) = project();
        timeline.insert_clip(track, clip(1, 0, 90_000)).unwrap();

        let command = delete_command(&timeline, ClipId(1)).expect("the clip exists");
        command.apply(&mut timeline).expect("the delete must apply");
        assert!(timeline.clips_at(Ticks(0)).is_empty());
    }
}
