//! Turning time into text a person can read.
//!
//! One job: format a number of seconds for the status bar and the timeline
//! ruler. Not responsible for: deciding which moment to show (the timeline
//! does that) or for exact arithmetic (ticks are integers; these functions
//! are for display only).
//!
//! Milliseconds are always shown in the status bar because an editor's
//! whole job is placing things on exact frames; a timecode that rounds to
//! the second hides the thing the user is looking at. Ruler labels drop the
//! decimals when the ruler's own step is a whole second or more.

/// `M:SS.mmm`, or `H:MM:SS.mmm` past an hour. Always three decimals, so
/// the width of the text does not jump while scrubbing.
pub fn format_clock(seconds: f64) -> String {
    if !seconds.is_finite() {
        return "—".to_string();
    }
    let negative = seconds < 0.0;
    let total_ms = (seconds.abs() * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    let body = if h > 0 {
        format!("{h}:{m:02}:{s:02}.{ms:03}")
    } else {
        format!("{m}:{s:02}.{ms:03}")
    };
    if negative {
        format!("-{body}")
    } else {
        body
    }
}

/// A ruler label: whole seconds when the ruler steps by a second or more,
/// the full clock otherwise (a half-second ruler needs to say which half).
pub fn format_ruler(seconds: f64, step_seconds: f64) -> String {
    if step_seconds >= 1.0 {
        let total = seconds.round() as i64;
        let s = total.rem_euclid(60);
        let m = (total / 60).rem_euclid(60);
        let h = total / 3600;
        if h > 0 {
            format!("{h}:{m:02}:{s:02}")
        } else {
            format!("{m}:{s:02}")
        }
    } else {
        format_clock(seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clock_reads_to_the_millisecond() {
        assert_eq!(format_clock(0.0), "0:00.000");
        assert_eq!(format_clock(3.5), "0:03.500");
        assert_eq!(format_clock(65.25), "1:05.250");
        assert_eq!(format_clock(3661.0), "1:01:01.000");
    }

    #[test]
    fn rounding_carries_into_the_next_minute_instead_of_printing_60() {
        assert_eq!(format_clock(59.9996), "1:00.000");
        assert_eq!(format_clock(3599.9996), "1:00:00.000");
    }

    #[test]
    fn negative_times_keep_their_sign() {
        assert_eq!(format_clock(-1.5), "-0:01.500");
    }

    #[test]
    fn ruler_labels_drop_decimals_on_whole_second_steps() {
        assert_eq!(format_ruler(3.0, 1.0), "0:03");
        assert_eq!(format_ruler(65.0, 5.0), "1:05");
        assert_eq!(format_ruler(3725.0, 60.0), "1:02:05");
        assert_eq!(format_ruler(0.5, 0.5), "0:00.500");
    }
}
