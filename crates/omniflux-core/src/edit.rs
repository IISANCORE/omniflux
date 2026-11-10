//! Edits as commands, and the history that undoes them.
//!
//! Why commands: an editor's undo stack is the difference between "undo" and
//! "panic". Every change is a command that knows how to put itself back, and
//! the history keeps those inverse commands — so remembering an edit costs
//! the size of the edit, not the size of the project. That is what makes
//! *unlimited* undo affordable.
//!
//! The rule that makes the whole thing work: applying a command RETURNS the
//! command that undoes it. Undo and redo are then the same operation with the
//! two stacks swapped, and there is no second implementation of "undo" that
//! could drift out of step with the first.
//!
//! Every command here is one-shot (it is consumed when applied) and every
//! failure leaves the project exactly as it was — a refused edit is not an
//! edit, and must not leave half of itself behind.

use crate::error::CoreError;
use crate::time::Ticks;
use crate::timeline::{Clip, ClipId, Timeline, TrackId};

/// One change to the timeline.
pub trait Command: std::fmt::Debug {
    /// A short human label for the undo list ("Insert clip").
    fn label(&self) -> &'static str;

    /// Applies the edit and returns the command that undoes it.
    ///
    /// The command is applied by reference, not consumed: a command that
    /// fails to apply must still be there afterwards, or the history would
    /// quietly lose an entry the user is entitled to undo.
    fn apply(&self, timeline: &mut Timeline) -> Result<Box<dyn Command>, CoreError>;
}

/// Wraps an inverse command with the label of the edit it undoes.
///
/// Without this, the undo list would read "remove clip" where the user
/// expects "insert clip" — the inverse of an edit forgets what it was
/// undoing unless someone keeps that memory.
#[derive(Debug)]
pub struct Inverse {
    label: &'static str,
    inner: Box<dyn Command>,
}

impl Command for Inverse {
    fn label(&self) -> &'static str {
        self.label
    }

    fn apply(&self, timeline: &mut Timeline) -> Result<Box<dyn Command>, CoreError> {
        self.inner.apply(timeline)
    }
}

/// Puts a clip onto a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertClip {
    pub track: TrackId,
    pub clip: Clip,
}

impl Command for InsertClip {
    fn label(&self) -> &'static str {
        "Insert clip"
    }

    fn apply(&self, timeline: &mut Timeline) -> Result<Box<dyn Command>, CoreError> {
        let clip_id = self.clip.id;
        timeline.insert_clip(self.track, self.clip.clone())?;
        Ok(Box::new(Inverse {
            label: "Insert clip",
            inner: Box::new(RemoveClip {
                track: self.track,
                clip: clip_id,
            }),
        }))
    }
}

/// Takes a clip off a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveClip {
    pub track: TrackId,
    pub clip: ClipId,
}

impl Command for RemoveClip {
    fn label(&self) -> &'static str {
        "Remove clip"
    }

    fn apply(&self, timeline: &mut Timeline) -> Result<Box<dyn Command>, CoreError> {
        let removed = timeline.remove_clip(self.track, self.clip)?;
        Ok(Box::new(Inverse {
            label: "Remove clip",
            inner: Box::new(InsertClip {
                track: self.track,
                clip: removed,
            }),
        }))
    }
}

/// Moves a clip to another track and/or another start time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveClip {
    pub clip: ClipId,
    pub to_track: TrackId,
    pub to_start: Ticks,
}

impl Command for MoveClip {
    fn label(&self) -> &'static str {
        "Move clip"
    }

    fn apply(&self, timeline: &mut Timeline) -> Result<Box<dyn Command>, CoreError> {
        let (from_track, from_start) =
            timeline.move_clip(self.clip, self.to_track, self.to_start)?;
        Ok(Box::new(Inverse {
            label: "Move clip",
            inner: Box::new(MoveClip {
                clip: self.clip,
                to_track: from_track,
                to_start: from_start,
            }),
        }))
    }
}

/// Changes a clip's length and where it starts inside its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrimClip {
    pub clip: ClipId,
    pub new_source_in: Ticks,
    pub new_duration: Ticks,
}

impl Command for TrimClip {
    fn label(&self) -> &'static str {
        "Trim clip"
    }

    fn apply(&self, timeline: &mut Timeline) -> Result<Box<dyn Command>, CoreError> {
        let (old_source_in, old_duration) =
            timeline.trim_clip(self.clip, self.new_source_in, self.new_duration)?;
        Ok(Box::new(Inverse {
            label: "Trim clip",
            inner: Box::new(TrimClip {
                clip: self.clip,
                new_source_in: old_source_in,
                new_duration: old_duration,
            }),
        }))
    }
}

/// Cuts a clip in two at a tick.
///
/// The right-hand clip gets the id the caller supplies, so undo and redo
/// name the same clip every time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitClip {
    pub clip: ClipId,
    pub at: Ticks,
    pub right: ClipId,
}

impl Command for SplitClip {
    fn label(&self) -> &'static str {
        "Split clip"
    }

    fn apply(&self, timeline: &mut Timeline) -> Result<Box<dyn Command>, CoreError> {
        let (old_source_in, old_duration) = {
            let original = timeline
                .clip(self.clip)
                .ok_or(CoreError::UnknownClip { clip: self.clip })?;
            (original.source_in, original.duration)
        };
        let track = timeline
            .track_of(self.clip)
            .ok_or(CoreError::UnknownClip { clip: self.clip })?;

        timeline.split_clip(self.clip, self.at, self.right)?;

        // Undo is the reverse of the edit: take the right half away first,
        // then grow the left half back to what it was.
        Ok(Box::new(Inverse {
            label: "Split clip",
            inner: Box::new(Composite::new(vec![
                Box::new(RemoveClip {
                    track,
                    clip: self.right,
                }),
                Box::new(TrimClip {
                    clip: self.clip,
                    new_source_in: old_source_in,
                    new_duration: old_duration,
                }),
            ])),
        }))
    }
}

/// Several edits applied as one.
///
/// Its inverse is the same edits in reverse order, which is exactly what
/// undoing a compound edit means.
#[derive(Debug)]
pub struct Composite {
    commands: Vec<Box<dyn Command>>,
}

impl Composite {
    /// Groups commands into one edit.
    pub fn new(commands: Vec<Box<dyn Command>>) -> Self {
        Composite { commands }
    }
}

impl Command for Composite {
    fn label(&self) -> &'static str {
        "Several edits"
    }

    fn apply(&self, timeline: &mut Timeline) -> Result<Box<dyn Command>, CoreError> {
        let mut inverses = Vec::with_capacity(self.commands.len());
        for command in &self.commands {
            inverses.push(command.apply(timeline)?);
        }
        Ok(Box::new(Composite {
            commands: inverses.into_iter().rev().collect(),
        }))
    }
}

/// The undo/redo stack.
///
/// The stacks are unbounded on purpose: the spike measures what one entry
/// costs, so "unlimited undo" is a number rather than a hope.
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Box<dyn Command>>,
    redo: Vec<Box<dyn Command>>,
}

impl History {
    /// An empty history.
    pub fn new() -> Self {
        History {
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// Applies an edit and records how to undo it.
    pub fn execute(
        &mut self,
        timeline: &mut Timeline,
        command: Box<dyn Command>,
    ) -> Result<(), CoreError> {
        let inverse = command.apply(timeline)?;
        self.undo.push(inverse);
        // A new edit invalidates the redo stack: the future it described is
        // no longer the future.
        self.redo.clear();
        Ok(())
    }

    /// Undoes the most recent edit. Returns false when there is nothing to
    /// undo.
    pub fn undo(&mut self, timeline: &mut Timeline) -> Result<bool, CoreError> {
        let Some(inverse) = self.undo.pop() else {
            return Ok(false);
        };
        match inverse.apply(timeline) {
            Ok(forward) => {
                self.redo.push(forward);
                Ok(true)
            }
            Err(error) => {
                // The undo did not happen, so the entry must stay where it
                // was: losing it would quietly shorten the user's history.
                self.undo.push(inverse);
                Err(error)
            }
        }
    }

    /// Redoes the most recently undone edit. Returns false when there is
    /// nothing to redo.
    pub fn redo(&mut self, timeline: &mut Timeline) -> Result<bool, CoreError> {
        let Some(forward) = self.redo.pop() else {
            return Ok(false);
        };
        match forward.apply(timeline) {
            Ok(inverse) => {
                self.undo.push(inverse);
                Ok(true)
            }
            Err(error) => {
                self.redo.push(forward);
                Err(error)
            }
        }
    }

    /// How many edits can still be undone.
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// How many edits can still be redone.
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    /// The labels of the edits that can be undone, newest first — what an
    /// undo menu would show.
    pub fn undo_labels(&self) -> Vec<&'static str> {
        self.undo
            .iter()
            .rev()
            .map(|command| command.label())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Timebase;
    use crate::timeline::{MediaId, TrackKind};

    fn clip(id: u32, start: i64, duration: i64) -> Clip {
        Clip {
            id: ClipId(id),
            media: MediaId(1),
            start: Ticks(start),
            duration: Ticks(duration),
            source_in: Ticks(0),
        }
    }

    fn project() -> (Timeline, TrackId) {
        let mut timeline = Timeline::new(Timebase::default_timebase());
        timeline.add_media(MediaId(1), Ticks(90_000)).unwrap();
        let track = timeline.add_track(TrackKind::Video, "Video 1");
        (timeline, track)
    }

    #[test]
    fn an_insert_can_be_undone_and_redone() {
        let (mut timeline, track) = project();
        let before = timeline.clone();
        let mut history = History::new();

        history
            .execute(
                &mut timeline,
                Box::new(InsertClip {
                    track,
                    clip: clip(1, 0, 100),
                }),
            )
            .expect("the insert must succeed");
        assert_eq!(timeline.tracks()[0].clips().len(), 1);

        assert!(history.undo(&mut timeline).expect("undo must succeed"));
        assert_eq!(timeline, before, "undo restores the project exactly");

        assert!(history.redo(&mut timeline).expect("redo must succeed"));
        assert_eq!(timeline.tracks()[0].clips().len(), 1);
    }

    #[test]
    fn undoing_a_move_puts_the_clip_back_where_it_was() {
        let (mut timeline, track) = project();
        timeline.insert_clip(track, clip(1, 0, 100)).unwrap();
        let before = timeline.clone();
        let mut history = History::new();

        history
            .execute(
                &mut timeline,
                Box::new(MoveClip {
                    clip: ClipId(1),
                    to_track: track,
                    to_start: Ticks(500),
                }),
            )
            .expect("the move must succeed");
        assert_eq!(timeline.clip(ClipId(1)).unwrap().start, Ticks(500));

        history.undo(&mut timeline).expect("undo must succeed");
        assert_eq!(timeline, before, "undo restores the project exactly");
    }

    #[test]
    fn undoing_a_trim_restores_the_original_source_range() {
        let (mut timeline, track) = project();
        timeline.insert_clip(track, clip(1, 0, 100)).unwrap();
        let before = timeline.clone();
        let mut history = History::new();

        history
            .execute(
                &mut timeline,
                Box::new(TrimClip {
                    clip: ClipId(1),
                    new_source_in: Ticks(200),
                    new_duration: Ticks(50),
                }),
            )
            .expect("the trim must succeed");

        history.undo(&mut timeline).expect("undo must succeed");
        assert_eq!(timeline, before, "undo restores the project exactly");
    }

    #[test]
    fn undoing_a_split_removes_the_new_clip_and_grows_the_old_one_back() {
        let (mut timeline, track) = project();
        timeline.insert_clip(track, clip(1, 0, 100)).unwrap();
        let before = timeline.clone();
        let mut history = History::new();

        history
            .execute(
                &mut timeline,
                Box::new(SplitClip {
                    clip: ClipId(1),
                    at: Ticks(40),
                    right: ClipId(2),
                }),
            )
            .expect("the split must succeed");
        assert_eq!(timeline.tracks()[0].clips().len(), 2);

        history.undo(&mut timeline).expect("undo must succeed");
        assert_eq!(timeline, before, "undo restores the project exactly");
    }

    #[test]
    fn a_new_edit_invalidates_the_redo_stack() {
        let (mut timeline, track) = project();
        let mut history = History::new();

        history
            .execute(
                &mut timeline,
                Box::new(InsertClip {
                    track,
                    clip: clip(1, 0, 100),
                }),
            )
            .unwrap();
        history.undo(&mut timeline).unwrap();
        assert_eq!(history.redo_depth(), 1);

        history
            .execute(
                &mut timeline,
                Box::new(InsertClip {
                    track,
                    clip: clip(2, 200, 100),
                }),
            )
            .unwrap();
        assert_eq!(history.redo_depth(), 0, "the old future is gone");
        assert!(!history.redo(&mut timeline).expect("redo must not fail"));
    }

    #[test]
    fn a_refused_edit_leaves_the_history_and_the_project_untouched() {
        let (mut timeline, track) = project();
        timeline.insert_clip(track, clip(1, 0, 100)).unwrap();
        let before = timeline.clone();
        let mut history = History::new();

        let error = history
            .execute(
                &mut timeline,
                Box::new(InsertClip {
                    track,
                    clip: clip(2, 50, 100),
                }),
            )
            .expect_err("an overlapping insert must be refused");
        assert!(matches!(error, CoreError::OverlappingClip { .. }));
        assert_eq!(timeline, before, "a refused edit changes nothing");
        assert_eq!(history.undo_depth(), 0, "a refused edit is not an edit");
    }

    #[test]
    fn a_failed_undo_keeps_the_history_intact() {
        let (mut timeline, track) = project();
        let mut history = History::new();

        history
            .execute(
                &mut timeline,
                Box::new(InsertClip {
                    track,
                    clip: clip(1, 0, 100),
                }),
            )
            .unwrap();
        assert_eq!(history.undo_depth(), 1);

        // Something outside the history takes the clip away — the model's own
        // mutators are public, so this is possible — and the undo can no
        // longer be applied.
        timeline.remove_clip(track, ClipId(1)).unwrap();

        let error = history
            .undo(&mut timeline)
            .expect_err("the undo must fail, because the clip is gone");
        assert!(matches!(error, CoreError::UnknownClip { .. }));
        assert_eq!(
            history.undo_depth(),
            1,
            "a failed undo must not quietly shorten the history"
        );
    }

    #[test]
    fn an_empty_history_says_so_instead_of_failing() {
        let (mut timeline, _) = project();
        let mut history = History::new();

        assert!(!history
            .undo(&mut timeline)
            .expect("undo on an empty history must not fail"));
        assert!(!history
            .redo(&mut timeline)
            .expect("redo on an empty history must not fail"));
    }

    #[test]
    fn the_undo_list_still_names_the_edits_the_user_made() {
        let (mut timeline, track) = project();
        let mut history = History::new();

        history
            .execute(
                &mut timeline,
                Box::new(InsertClip {
                    track,
                    clip: clip(1, 0, 100),
                }),
            )
            .unwrap();
        history
            .execute(
                &mut timeline,
                Box::new(MoveClip {
                    clip: ClipId(1),
                    to_track: track,
                    to_start: Ticks(300),
                }),
            )
            .unwrap();

        assert_eq!(
            history.undo_labels(),
            vec!["Move clip", "Insert clip"],
            "the undo list names the edits, newest first"
        );
    }
}
