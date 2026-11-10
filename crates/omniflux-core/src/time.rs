//! Exact time for the timeline.
//!
//! Why integers: an editor that stores positions as floating-point seconds
//! drifts — and "the clip does not line up with the audio" is a miserable
//! bug to chase. Every position in OmniFlux is a count of TICKS on a project
//! timebase, so arithmetic is exact and a frame boundary is a number, not a
//! rounding decision.
//!
//! Why 90 000 ticks per second: it divides exactly by every frame rate an
//! editor meets (24, 25, 30, 50, 60), so one frame is always a whole number
//! of ticks and no frame boundary is ever a fraction.

use crate::error::CoreError;

/// A count of ticks on the project timebase.
///
/// Ticks are signed: a clip may legally start before the timeline's zero
/// (an editor can place material before the origin), and durations are
/// always positive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Ticks(pub i64);

impl Ticks {
    /// The zero point of any timeline.
    pub const fn zero() -> Self {
        Ticks(0)
    }

    /// The raw tick count.
    pub const fn get(self) -> i64 {
        self.0
    }

    /// The first tick after `self` — the end of a half-open time range.
    pub const fn next(self) -> Ticks {
        Ticks(self.0 + 1)
    }
}

impl std::ops::Add for Ticks {
    type Output = Ticks;
    fn add(self, other: Ticks) -> Ticks {
        Ticks(self.0 + other.0)
    }
}

impl std::ops::Sub for Ticks {
    type Output = Ticks;
    fn sub(self, other: Ticks) -> Ticks {
        Ticks(self.0 - other.0)
    }
}

impl std::ops::AddAssign for Ticks {
    fn add_assign(&mut self, other: Ticks) {
        self.0 += other.0;
    }
}

impl std::ops::SubAssign for Ticks {
    fn sub_assign(&mut self, other: Ticks) {
        self.0 -= other.0;
    }
}

/// The project's timebase: how many ticks make one second.
///
/// The timebase belongs to the timeline, not to the media: one project has
/// one clock, and every clip in it is measured on that clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timebase {
    ticks_per_second: u32,
}

impl Timebase {
    /// The default project timebase: 90 000 ticks per second, which divides
    /// exactly by 24, 25, 30, 50 and 60 frames per second.
    pub const DEFAULT_TICKS_PER_SECOND: u32 = 90_000;

    /// Builds a timebase. A timebase of zero ticks per second cannot
    /// describe time, so it is rejected rather than silently accepted.
    pub fn new(ticks_per_second: u32) -> Result<Self, CoreError> {
        if ticks_per_second == 0 {
            return Err(CoreError::InvalidTimebase { ticks_per_second });
        }
        Ok(Timebase { ticks_per_second })
    }

    /// The default timebase (90 000 ticks per second).
    pub fn default_timebase() -> Self {
        Timebase {
            ticks_per_second: Self::DEFAULT_TICKS_PER_SECOND,
        }
    }

    /// How many ticks make one second.
    pub fn ticks_per_second(self) -> u32 {
        self.ticks_per_second
    }

    /// Converts seconds to ticks, rounding to the nearest tick.
    pub fn seconds(self, seconds: f64) -> Ticks {
        let scaled = seconds * f64::from(self.ticks_per_second);
        Ticks(scaled.round() as i64)
    }

    /// Converts ticks to seconds, for display only.
    pub fn to_seconds(self, ticks: Ticks) -> f64 {
        ticks.0 as f64 / f64::from(self.ticks_per_second)
    }

    /// The whole frame index a tick falls in, at the given frame rate.
    ///
    /// Exact when the frame rate divides the timebase evenly (all the rates
    /// an editor meets do); otherwise it truncates toward negative infinity,
    /// which keeps the mapping monotonic.
    pub fn frame_index(self, ticks: Ticks, frames_per_second: u32) -> i64 {
        if frames_per_second == 0 {
            return 0;
        }
        let ticks_per_frame = f64::from(self.ticks_per_second) / f64::from(frames_per_second);
        (ticks.0 as f64 / ticks_per_frame).floor() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_timebase_divides_by_every_frame_rate_an_editor_meets() {
        let timebase = Timebase::default_timebase();
        let ticks = timebase.ticks_per_second();

        for frames_per_second in [24u32, 25, 30, 50, 60] {
            assert_eq!(
                ticks % frames_per_second,
                0,
                "{frames_per_second} fps must land on whole ticks"
            );
        }
    }

    #[test]
    fn seconds_round_to_the_nearest_tick() {
        let timebase = Timebase::default_timebase();

        assert_eq!(timebase.seconds(0.0), Ticks(0));
        assert_eq!(timebase.seconds(1.0), Ticks(90_000));
        assert_eq!(timebase.seconds(0.5), Ticks(45_000));
        // 1/30 s at 90 kHz is exactly 3000 ticks.
        assert_eq!(timebase.seconds(1.0 / 30.0), Ticks(3000));
        // A frame duration at 24 fps is 3750 ticks — no fraction anywhere.
        assert_eq!(timebase.seconds(1.0 / 24.0), Ticks(3750));
    }

    #[test]
    fn zero_ticks_per_second_is_rejected() {
        assert!(Timebase::new(0).is_err());
        assert!(Timebase::new(90_000).is_ok());
    }

    #[test]
    fn frame_index_counts_frames_exactly_at_whole_frame_rates() {
        let timebase = Timebase::default_timebase();

        assert_eq!(timebase.frame_index(Ticks(0), 30), 0);
        assert_eq!(
            timebase.frame_index(Ticks(2999), 30),
            0,
            "still inside frame 0"
        );
        assert_eq!(
            timebase.frame_index(Ticks(3000), 30),
            1,
            "frame 1 starts at tick 3000"
        );
        assert_eq!(timebase.frame_index(Ticks(89_999), 30), 29);
        assert_eq!(timebase.frame_index(Ticks(3750), 24), 1);
    }
}
