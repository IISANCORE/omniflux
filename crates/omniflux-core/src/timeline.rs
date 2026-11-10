//! The timeline model: the media, the tracks, the clips on them, and the
//! rules they must obey.
//!
//! What this model is: the *arrangement* of material — which piece of which
//! source sits where, for how long. It knows nothing about pixels, codecs,
//! files or windows; that separation is the point (Law 11: core never learns
//! that a window exists).
//!
//! The rules it enforces, on every change:
//! - clips on one track never cover the same time (no accidental overlap),
//! - a clip always stays inside its source asset,
//! - a clip always lasts more than zero ticks.
//!
//! Everything that changes the model goes through these methods, so a
//! timeline that exists is a timeline whose rules hold. `validate()` re-checks
//! the whole model, which is what the spike uses to prove that undoing every
//! edit really did return the project to where it started.
//!
//! Known limit of this first model: clips map 1:1 onto their source (no
//! speed changes, no freeze frames, no transitions), and a timeline is a flat
//! list of tracks with no nesting. Both are deliberate — the spike is about
//! the edit/undo machinery, not about effects.

use crate::error::CoreError;
use crate::time::{Ticks, Timebase};

/// Identifies a source asset (something the project has imported).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MediaId(pub u32);

/// Identifies a track within one timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrackId(pub u32);

/// Identifies a clip within one timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClipId(pub u32);

/// What a track carries. The model treats video and audio the same way; the
/// difference matters to the renderer, not to the arrangement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
}

/// A source asset and how long it is.
///
/// The model needs a source's length to know whether a clip may run past its
/// end. It does not need (and does not have) the file itself — the length is
/// the one fact about a source the arrangement depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaAsset {
    pub id: MediaId,
    pub duration: Ticks,
}

/// One placed piece of media: a range of a source asset, sitting somewhere
/// on a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    /// This clip's identity, unique within the timeline.
    pub id: ClipId,
    /// Which source asset this clip shows.
    pub media: MediaId,
    /// Where the clip starts on the timeline.
    pub start: Ticks,
    /// How long the clip lasts on the timeline.
    pub duration: Ticks,
    /// Where the clip starts inside its source asset.
    pub source_in: Ticks,
}

impl Clip {
    /// The first tick after this clip ends.
    pub fn end(&self) -> Ticks {
        self.start + self.duration
    }

    /// The tick in the source where this clip's content ends.
    pub fn source_end(&self) -> Ticks {
        self.source_in + self.duration
    }
}

/// A row of clips. Clips are kept sorted by start time and never overlap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    /// A human label ("Video 1", "Audio 2"). Names are display data, never
    /// used to look anything up — identity is the id.
    pub name: String,
    clips: Vec<Clip>,
}

impl Track {
    /// The clips on this track, in time order.
    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }
}

/// One project: a clock, the source assets it draws on, and the tracks that
/// arrange them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    timebase: Timebase,
    media: Vec<MediaAsset>,
    tracks: Vec<Track>,
}

impl Timeline {
    /// An empty project on the given timebase.
    pub fn new(timebase: Timebase) -> Self {
        Timeline {
            timebase,
            media: Vec::new(),
            tracks: Vec::new(),
        }
    }

    /// The project's clock.
    pub fn timebase(&self) -> Timebase {
        self.timebase
    }

    /// Registers a source asset and how long it is.
    pub fn add_media(&mut self, id: MediaId, duration: Ticks) -> Result<(), CoreError> {
        if self.media.iter().any(|asset| asset.id == id) {
            return Err(CoreError::DuplicateMedia { media: id });
        }
        self.media.push(MediaAsset { id, duration });
        Ok(())
    }

    /// How long a source asset is, if the project knows it.
    pub fn media_duration(&self, media: MediaId) -> Option<Ticks> {
        self.media
            .iter()
            .find(|asset| asset.id == media)
            .map(|asset| asset.duration)
    }

    /// Adds a track and returns its id.
    pub fn add_track(&mut self, kind: TrackKind, name: impl Into<String>) -> TrackId {
        let id = TrackId(self.tracks.len() as u32);
        self.tracks.push(Track {
            id,
            kind,
            name: name.into(),
            clips: Vec::new(),
        });
        id
    }

    /// Every track, in order.
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// One track by id.
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|track| track.id == id)
    }

    /// One clip by id, wherever it sits.
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .find(|clip| clip.id == id)
    }

    /// The clips covering a given tick, in track order.
    pub fn clips_at(&self, tick: Ticks) -> Vec<&Clip> {
        self.tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .filter(|clip| clip.start <= tick && tick < clip.end())
            .collect()
    }

    /// How long the project is: the furthest point any clip reaches.
    pub fn duration(&self) -> Ticks {
        self.tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .map(|clip| clip.end())
            .max()
            .unwrap_or_else(Ticks::zero)
    }

    /// Places a clip on a track.
    ///
    /// Fails if the track or the source is unknown, if the clip would last
    /// no time at all, if it would run past the end of its source, or if it
    /// would cover time another clip on that track already covers.
    pub fn insert_clip(&mut self, track_id: TrackId, clip: Clip) -> Result<(), CoreError> {
        self.check_clip_fits_media(&clip)?;
        self.check_no_overlap_at(track_id, &clip, None)?;

        let Timeline { tracks, .. } = self;
        let track = tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or(CoreError::UnknownTrack { track: track_id })?;
        let index = track
            .clips
            .partition_point(|other| other.start <= clip.start);
        track.clips.insert(index, clip);
        Ok(())
    }

    /// Takes a clip off a track and hands it back.
    pub fn remove_clip(&mut self, track_id: TrackId, clip_id: ClipId) -> Result<Clip, CoreError> {
        let track = self
            .tracks
            .iter_mut()
            .find(|track| track.id == track_id)
            .ok_or(CoreError::UnknownTrack { track: track_id })?;
        let index = track
            .clips
            .iter()
            .position(|clip| clip.id == clip_id)
            .ok_or(CoreError::UnknownClip { clip: clip_id })?;
        Ok(track.clips.remove(index))
    }

    /// Moves a clip to another track and/or another start time, and reports
    /// where it used to sit.
    ///
    /// Fails if the clip or the destination track is unknown, or if the
    /// destination would overlap another clip there.
    pub fn move_clip(
        &mut self,
        clip_id: ClipId,
        to_track: TrackId,
        to_start: Ticks,
    ) -> Result<(TrackId, Ticks), CoreError> {
        let from_track = self
            .track_of(clip_id)
            .ok_or(CoreError::UnknownClip { clip: clip_id })?;
        let original = self
            .clip(clip_id)
            .expect("the track was found, so the clip is on it")
            .clone();

        let mut moved = original.clone();
        moved.start = to_start;

        // The destination is checked BEFORE anything moves. A refused move
        // therefore cannot lose the clip or leave it half-moved.
        self.check_no_overlap_at(to_track, &moved, Some(clip_id))?;

        let removed = self.remove_clip(from_track, clip_id)?;
        debug_assert_eq!(removed.id, clip_id);
        // The destination was checked above and the clip already fits its
        // source, so this insert cannot fail.
        self.insert_clip(to_track, moved)
            .expect("the destination was checked above, so this insert cannot fail");

        Ok((from_track, original.start))
    }

    /// Changes where a clip starts inside its source and how long it lasts,
    /// and reports the range it used to cover.
    ///
    /// Fails if the clip is unknown, if the new length is zero or negative,
    /// if the new range would run past the end of the source, or if the
    /// longer clip would reach over the next clip on its track.
    pub fn trim_clip(
        &mut self,
        clip_id: ClipId,
        new_source_in: Ticks,
        new_duration: Ticks,
    ) -> Result<(Ticks, Ticks), CoreError> {
        if new_duration <= Ticks::zero() {
            return Err(CoreError::EmptyClip { clip: clip_id });
        }
        let (media, start, old_source_in, old_duration) = {
            let clip = self
                .clip(clip_id)
                .ok_or(CoreError::UnknownClip { clip: clip_id })?;
            (clip.media, clip.start, clip.source_in, clip.duration)
        };

        let trimmed = Clip {
            id: clip_id,
            media,
            start,
            duration: new_duration,
            source_in: new_source_in,
        };
        self.check_clip_fits_media(&trimmed)?;

        {
            let clip = self
                .clip_mut(clip_id)
                .expect("the clip was found a moment ago");
            clip.source_in = new_source_in;
            clip.duration = new_duration;
        }
        self.check_no_overlap_after_change(clip_id)?;
        Ok((old_source_in, old_duration))
    }

    /// Cuts a clip in two at a tick, and returns the new right-hand clip.
    ///
    /// The left half keeps the original clip's id; the right half gets the
    /// id the caller supplies, so undo and redo can name the same clip twice.
    pub fn split_clip(
        &mut self,
        clip_id: ClipId,
        at: Ticks,
        right_id: ClipId,
    ) -> Result<Clip, CoreError> {
        let original = self
            .clip(clip_id)
            .ok_or(CoreError::UnknownClip { clip: clip_id })?;

        if at <= original.start || at >= original.end() {
            return Err(CoreError::SplitOutsideClip { clip: clip_id, at });
        }

        let left_duration = at - original.start;
        let right = Clip {
            id: right_id,
            media: original.media,
            start: at,
            duration: original.end() - at,
            source_in: original.source_in + left_duration,
        };

        {
            let left = self
                .clip_mut(clip_id)
                .expect("the clip was found a moment ago");
            left.duration = left_duration;
        }
        self.insert_clip(
            self.track_of(clip_id).expect("the clip is on a track"),
            right.clone(),
        )?;

        Ok(right)
    }

    /// Re-checks every rule the model promises.
    ///
    /// Used by the spike to prove that undoing every edit restored the
    /// project exactly; also useful as a cheap self-check after a risky
    /// change.
    pub fn validate(&self) -> Result<(), CoreError> {
        for track in &self.tracks {
            let mut previous_end: Option<Ticks> = None;
            for clip in &track.clips {
                if clip.duration <= Ticks::zero() {
                    return Err(CoreError::EmptyClip { clip: clip.id });
                }
                self.check_clip_fits_media(clip)?;
                if let Some(end) = previous_end {
                    if clip.start < end {
                        return Err(CoreError::OverlappingClip {
                            track: track.id,
                            start: clip.start,
                            end: clip.end(),
                        });
                    }
                }
                previous_end = Some(clip.end());
            }
        }
        Ok(())
    }

    /// The tick a clip starts at, in its source.
    fn check_clip_fits_media(&self, clip: &Clip) -> Result<(), CoreError> {
        if clip.duration <= Ticks::zero() {
            return Err(CoreError::EmptyClip { clip: clip.id });
        }
        let available = self
            .media_duration(clip.media)
            .ok_or(CoreError::UnknownMedia { media: clip.media })?;
        let needed = clip.source_end();
        if needed > available {
            return Err(CoreError::ClipOutsideMedia {
                clip: clip.id,
                needed,
                available,
            });
        }
        Ok(())
    }

    /// Mutable access to one clip — private so that the rules above cannot
    /// be broken behind the model's back.
    fn clip_mut(&mut self, clip_id: ClipId) -> Option<&mut Clip> {
        self.tracks
            .iter_mut()
            .flat_map(|track| track.clips.iter_mut())
            .find(|clip| clip.id == clip_id)
    }

    /// The track a clip sits on.
    pub fn track_of(&self, clip_id: ClipId) -> Option<TrackId> {
        self.tracks
            .iter()
            .find(|track| track.clips.iter().any(|clip| clip.id == clip_id))
            .map(|track| track.id)
    }

    /// Checks that a clip that just changed still does not cover time that
    /// belongs to a neighbour on its track.
    fn check_no_overlap_after_change(&self, clip_id: ClipId) -> Result<(), CoreError> {
        let clip = self
            .clip(clip_id)
            .ok_or(CoreError::UnknownClip { clip: clip_id })?;
        let track = self
            .track_of(clip_id)
            .ok_or(CoreError::UnknownClip { clip: clip_id })?;
        self.check_no_overlap_at(track, clip, Some(clip_id))
    }

    /// Checks whether a clip could sit at this place on this track.
    ///
    /// `exclude` is the clip being moved or resized: it is allowed to
    /// overlap itself.
    fn check_no_overlap_at(
        &self,
        track_id: TrackId,
        candidate: &Clip,
        exclude: Option<ClipId>,
    ) -> Result<(), CoreError> {
        let track = self
            .tracks
            .iter()
            .find(|track| track.id == track_id)
            .ok_or(CoreError::UnknownTrack { track: track_id })?;
        let conflicts = track
            .clips
            .iter()
            .any(|other| Some(other.id) != exclude && overlaps(other, candidate));
        if conflicts {
            return Err(CoreError::OverlappingClip {
                track: track_id,
                start: candidate.start,
                end: candidate.end(),
            });
        }
        Ok(())
    }
}

/// Do two half-open time ranges share any time?
fn overlaps(left: &Clip, right: &Clip) -> bool {
    left.start < right.end() && right.start < left.end()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(id: u32, media: MediaId, start: i64, duration: i64) -> Clip {
        Clip {
            id: ClipId(id),
            media,
            start: Ticks(start),
            duration: Ticks(duration),
            source_in: Ticks(0),
        }
    }

    fn timeline() -> Timeline {
        let mut timeline = Timeline::new(Timebase::default_timebase());
        timeline.add_media(MediaId(1), Ticks(90_000)).unwrap();
        timeline
    }

    #[test]
    fn clips_are_kept_in_time_order_whatever_order_they_arrive_in() {
        let mut timeline = timeline();
        let track = timeline.add_track(TrackKind::Video, "Video 1");

        timeline
            .insert_clip(track, clip(2, MediaId(1), 200, 100))
            .unwrap();
        timeline
            .insert_clip(track, clip(1, MediaId(1), 0, 100))
            .unwrap();

        let starts: Vec<i64> = timeline
            .track(track)
            .unwrap()
            .clips()
            .iter()
            .map(|c| c.start.0)
            .collect();
        assert_eq!(starts, vec![0, 200], "clips are sorted by start time");
    }

    #[test]
    fn a_clip_may_not_cover_time_another_clip_already_covers() {
        let mut timeline = timeline();
        let track = timeline.add_track(TrackKind::Video, "Video 1");
        timeline
            .insert_clip(track, clip(1, MediaId(1), 0, 100))
            .unwrap();

        let error = timeline
            .insert_clip(track, clip(2, MediaId(1), 50, 100))
            .expect_err("overlapping clips must be refused");
        assert_eq!(
            error,
            CoreError::OverlappingClip {
                track,
                start: Ticks(50),
                end: Ticks(150),
            }
        );
    }

    #[test]
    fn a_clip_may_touch_another_clip_without_overlapping_it() {
        let mut timeline = timeline();
        let track = timeline.add_track(TrackKind::Video, "Video 1");
        timeline
            .insert_clip(track, clip(1, MediaId(1), 0, 100))
            .unwrap();

        timeline
            .insert_clip(track, clip(2, MediaId(1), 100, 100))
            .expect("a clip may start exactly where the previous one ends");
    }

    #[test]
    fn a_clip_may_not_run_past_the_end_of_its_source() {
        let mut timeline = timeline();
        let track = timeline.add_track(TrackKind::Video, "Video 1");

        let error = timeline
            .insert_clip(track, clip(1, MediaId(1), 0, 90_001))
            .expect_err("a clip longer than its source must be refused");
        assert!(matches!(error, CoreError::ClipOutsideMedia { .. }));
    }

    #[test]
    fn a_clip_from_an_unknown_source_is_refused() {
        let mut timeline = timeline();
        let track = timeline.add_track(TrackKind::Video, "Video 1");

        let error = timeline
            .insert_clip(track, clip(1, MediaId(99), 0, 100))
            .expect_err("an unknown source must be refused");
        assert_eq!(error, CoreError::UnknownMedia { media: MediaId(99) });
    }

    #[test]
    fn a_clip_may_not_last_no_time_at_all() {
        let mut timeline = timeline();
        let track = timeline.add_track(TrackKind::Video, "Video 1");

        let error = timeline
            .insert_clip(track, clip(1, MediaId(1), 0, 0))
            .expect_err("a zero-length clip must be refused");
        assert_eq!(error, CoreError::EmptyClip { clip: ClipId(1) });
    }

    #[test]
    fn a_failed_move_leaves_the_project_untouched() {
        let mut timeline = timeline();
        let track = timeline.add_track(TrackKind::Video, "Video 1");
        timeline
            .insert_clip(track, clip(1, MediaId(1), 0, 100))
            .unwrap();
        timeline
            .insert_clip(track, clip(2, MediaId(1), 200, 100))
            .unwrap();
        let before = timeline.clone();

        let error = timeline
            .move_clip(ClipId(1), track, Ticks(250))
            .expect_err("moving onto another clip must be refused");
        assert!(matches!(error, CoreError::OverlappingClip { .. }));
        assert_eq!(timeline, before, "a refused move changes nothing");
    }

    #[test]
    fn a_split_leaves_two_clips_that_touch_without_overlapping() {
        let mut timeline = timeline();
        let track = timeline.add_track(TrackKind::Video, "Video 1");
        timeline
            .insert_clip(track, clip(1, MediaId(1), 0, 100))
            .unwrap();

        let right = timeline
            .split_clip(ClipId(1), Ticks(40), ClipId(2))
            .expect("splitting inside the clip must work");

        assert_eq!(right.source_in, Ticks(40));
        assert_eq!(right.duration, Ticks(60));
        assert_eq!(right.start, Ticks(40));
        let clips = timeline.track(track).unwrap().clips();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].duration, Ticks(40));
        timeline.validate().expect("a split leaves a valid project");
    }

    #[test]
    fn a_split_point_must_be_inside_the_clip() {
        let mut timeline = timeline();
        let track = timeline.add_track(TrackKind::Video, "Video 1");
        timeline
            .insert_clip(track, clip(1, MediaId(1), 100, 100))
            .unwrap();

        let error = timeline
            .split_clip(ClipId(1), Ticks(100), ClipId(2))
            .expect_err("splitting at the very start must be refused");
        assert_eq!(
            error,
            CoreError::SplitOutsideClip {
                clip: ClipId(1),
                at: Ticks(100)
            }
        );
    }

    #[test]
    fn the_project_length_is_the_furthest_point_any_clip_reaches() {
        let mut timeline = timeline();
        let track = timeline.add_track(TrackKind::Video, "Video 1");
        assert_eq!(timeline.duration(), Ticks(0));

        timeline
            .insert_clip(track, clip(1, MediaId(1), 100, 50))
            .unwrap();
        assert_eq!(timeline.duration(), Ticks(150));
    }

    #[test]
    fn clips_at_finds_the_material_under_the_playhead() {
        let mut timeline = timeline();
        let video = timeline.add_track(TrackKind::Video, "Video 1");
        let audio = timeline.add_track(TrackKind::Audio, "Audio 1");
        timeline
            .insert_clip(video, clip(1, MediaId(1), 0, 100))
            .unwrap();
        timeline
            .insert_clip(audio, clip(2, MediaId(1), 50, 100))
            .unwrap();

        assert_eq!(timeline.clips_at(Ticks(25)).len(), 1);
        assert_eq!(timeline.clips_at(Ticks(75)).len(), 2);
        assert_eq!(timeline.clips_at(Ticks(150)).len(), 0);
    }
}
