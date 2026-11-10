//! The one error type this crate returns.
//!
//! Law 11: one error enum per crate, carrying context. A caller that gets an
//! error should be able to print it and know exactly what went wrong —
//! which clip, which track, which number — without re-deriving it.

use crate::time::Ticks;
use crate::timeline::{ClipId, MediaId, TrackId};

/// Everything that can go wrong in the timeline model or its edit commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    /// A timebase of zero ticks per second cannot describe time.
    InvalidTimebase { ticks_per_second: u32 },
    /// The edit referenced a media asset the timeline does not know.
    UnknownMedia { media: MediaId },
    /// The edit referenced a track the timeline does not have.
    UnknownTrack { track: TrackId },
    /// The edit referenced a clip that is not on the timeline (or not on the
    /// track it was expected to be on).
    UnknownClip { clip: ClipId },
    /// Two clips on the same track would cover the same time.
    OverlappingClip {
        track: TrackId,
        start: Ticks,
        end: Ticks,
    },
    /// A clip would run past the end of its source asset.
    ClipOutsideMedia {
        clip: ClipId,
        needed: Ticks,
        available: Ticks,
    },
    /// A clip must last longer than zero ticks to exist.
    EmptyClip { clip: ClipId },
    /// A split point has to be strictly inside the clip.
    SplitOutsideClip { clip: ClipId, at: Ticks },
    /// A media asset with this id was already registered.
    DuplicateMedia { media: MediaId },
    /// A clip with this id is already on the timeline.
    ///
    /// Clip ids are how the model, the commands and a saved project all
    /// point at the same clip, so two clips may never share one.
    DuplicateClip { clip: ClipId },
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreError::InvalidTimebase { ticks_per_second } => write!(
                f,
                "a timebase of {ticks_per_second} ticks per second cannot describe time"
            ),
            CoreError::UnknownMedia { media } => {
                write!(f, "no media asset with id {}", media.0)
            }
            CoreError::UnknownTrack { track } => {
                write!(f, "no track with id {}", track.0)
            }
            CoreError::UnknownClip { clip } => write!(f, "no clip with id {}", clip.0),
            CoreError::OverlappingClip { track, start, end } => write!(
                f,
                "a clip on track {} would cover {}..{}, which overlaps another clip",
                track.0, start.0, end.0
            ),
            CoreError::ClipOutsideMedia {
                clip,
                needed,
                available,
            } => write!(
                f,
                "clip {} needs {} ticks of source but its media only has {}",
                clip.0, needed.0, available.0
            ),
            CoreError::EmptyClip { clip } => {
                write!(f, "clip {} would last zero ticks", clip.0)
            }
            CoreError::SplitOutsideClip { clip, at } => write!(
                f,
                "split point {} is not strictly inside clip {}",
                at.0, clip.0
            ),
            CoreError::DuplicateMedia { media } => {
                write!(f, "media asset {} is already registered", media.0)
            }
            CoreError::DuplicateClip { clip } => {
                write!(f, "a clip with id {} is already on the timeline", clip.0)
            }
        }
    }
}

impl std::error::Error for CoreError {}
