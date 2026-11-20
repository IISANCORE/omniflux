//! The timeline strip: a ruler, one video track, its clips, and the
//! playhead.
//!
//! One job: draw the arrangement and report what the pointer asked for
//! (a new playhead position, a selected clip). Not responsible for: moving
//! the playhead or seeking the decoder — the app does that, because those
//! are engine actions, not drawing.
//!
//! The prototype fits the whole project into the available width. Zooming
//! and scrolling are deliberately absent: the point of this window is to
//! test the engine in a real one, and a fit-to-window ruler already does
//! that honestly.

use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind};
use omniflux_core::time::{Ticks, Timebase};
use omniflux_core::timeline::{ClipId, Timeline};

use crate::time_text;

const LABEL_WIDTH: f32 = 96.0;
const RULER_HEIGHT: f32 = 22.0;
const TRACK_HEIGHT: f32 = 58.0;
const PAD: f32 = 6.0;
const MIN_LABEL_GAP: f64 = 70.0;

/// What the pointer asked for this frame.
#[derive(Default)]
pub struct Outcome {
    /// The moment under the pointer, when the pointer is scrubbing.
    pub seek_to: Option<Ticks>,
    /// A clip the user clicked or started dragging on.
    pub select: Option<ClipId>,
    /// Where the pointer was, so the app can avoid re-seeking at a standstill.
    pub pointer_x: Option<f32>,
}

/// Draws the strip and reports the interaction.
pub fn show(
    ui: &mut egui::Ui,
    timeline: &Timeline,
    span: Ticks,
    playhead: Ticks,
    selected: Option<ClipId>,
) -> Outcome {
    let width = ui.available_width().max(200.0);
    let height = RULER_HEIGHT + TRACK_HEIGHT + PAD * 2.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_gray(18));

    let timebase = timeline.timebase();
    let span = span.max(Ticks(1));
    let track_left = rect.left() + LABEL_WIDTH;
    let track_right = (rect.right() - PAD).max(track_left + 1.0);
    let track_width = track_right - track_left;
    let span_seconds = timebase.to_seconds(span).max(0.000_001);
    let pixels_per_second = (track_width / span_seconds as f32).max(0.001);

    let row_top = rect.top() + RULER_HEIGHT;
    let row_rect = Rect::from_min_max(
        pos2(track_left, row_top),
        pos2(track_right, rect.bottom() - PAD),
    );

    // The label column: one track in this prototype, but the strip should
    // still say what the row is.
    painter.rect_filled(
        Rect::from_min_max(rect.left_top(), pos2(track_left, rect.bottom())),
        0.0,
        Color32::from_gray(21),
    );
    let track_name = timeline
        .tracks()
        .first()
        .map(|track| track.name.as_str())
        .unwrap_or("No track");
    painter.text(
        pos2(rect.left() + PAD, row_rect.center().y),
        Align2::LEFT_CENTER,
        track_name,
        FontId::proportional(12.0),
        Color32::from_gray(200),
    );

    // The ruler.
    painter.rect_filled(
        Rect::from_min_max(pos2(track_left, rect.top()), pos2(track_right, row_top)),
        0.0,
        Color32::from_gray(27),
    );
    let step = ruler_step(pixels_per_second);
    let tick_count = (span_seconds / step).floor() as usize;
    for index in 0..=tick_count {
        let seconds = index as f64 * step;
        let x = track_left + (seconds as f32) * pixels_per_second;
        painter.line_segment(
            [pos2(x, rect.top() + 3.0), pos2(x, row_top)],
            Stroke::new(1.0, Color32::from_gray(45)),
        );
        painter.text(
            pos2(x + 4.0, rect.top() + 2.0),
            Align2::LEFT_TOP,
            time_text::format_ruler(seconds, step),
            FontId::proportional(10.0),
            Color32::from_gray(150),
        );
    }

    // The track bed.
    painter.rect_filled(row_rect, 0.0, Color32::from_gray(23));
    painter.rect_stroke(
        row_rect,
        0.0,
        Stroke::new(1.0, Color32::from_gray(40)),
        StrokeKind::Inside,
    );

    // The clips.
    for clip in timeline
        .tracks()
        .iter()
        .flat_map(|track| track.clips().iter())
    {
        let x0 = ticks_to_x(clip.start, timebase, pixels_per_second, track_left);
        let x1 = ticks_to_x(clip.end(), timebase, pixels_per_second, track_left);
        let clip_rect = Rect::from_min_max(
            pos2(x0 + 1.0, row_rect.top() + 4.0),
            pos2((x1 - 1.0).max(x0 + 2.0), row_rect.bottom() - 4.0),
        );
        let is_selected = Some(clip.id) == selected;
        let fill = if is_selected {
            Color32::from_rgb(62, 108, 168)
        } else {
            Color32::from_rgb(50, 68, 94)
        };
        painter.rect_filled(clip_rect, 4.0, fill);
        painter.rect_stroke(
            clip_rect,
            4.0,
            Stroke::new(
                1.0,
                if is_selected {
                    Color32::from_rgb(150, 190, 240)
                } else {
                    Color32::from_rgb(90, 120, 160)
                },
            ),
            StrokeKind::Inside,
        );
        let clip_painter = painter.with_clip_rect(clip_rect);
        clip_painter.text(
            clip_rect.left_top() + vec2(6.0, 5.0),
            Align2::LEFT_TOP,
            format!("Clip {}", clip.id.0),
            FontId::proportional(12.0),
            Color32::WHITE,
        );
        clip_painter.text(
            clip_rect.left_bottom() + vec2(6.0, -5.0),
            Align2::LEFT_BOTTOM,
            format!(
                "{} – {}",
                time_text::format_clock(timebase.to_seconds(clip.start)),
                time_text::format_clock(timebase.to_seconds(clip.end()))
            ),
            FontId::monospace(10.0),
            Color32::from_gray(190),
        );
    }

    // The playhead.
    let playhead_x = ticks_to_x(playhead, timebase, pixels_per_second, track_left)
        .clamp(track_left, track_right);
    painter.line_segment(
        [
            pos2(playhead_x, rect.top()),
            pos2(playhead_x, rect.bottom() - PAD),
        ],
        Stroke::new(2.0, Color32::from_rgb(235, 92, 72)),
    );
    painter.circle_filled(
        pos2(playhead_x, rect.top() + 4.0),
        4.0,
        Color32::from_rgb(235, 92, 72),
    );

    let mut outcome = Outcome::default();
    if response.clicked() || response.dragged() {
        if let Some(pointer) = response.interact_pointer_pos() {
            outcome.pointer_x = Some(pointer.x);
            if pointer.x >= track_left {
                let ticks = x_to_ticks(pointer.x, timebase, pixels_per_second, track_left)
                    .clamp(Ticks::zero(), span);
                outcome.seek_to = Some(ticks);
            }
            if (response.clicked() || response.drag_started()) && pointer.y >= row_top {
                outcome.select = timeline
                    .tracks()
                    .iter()
                    .flat_map(|track| track.clips().iter())
                    .find(|clip| {
                        let x0 = ticks_to_x(clip.start, timebase, pixels_per_second, track_left);
                        let x1 = ticks_to_x(clip.end(), timebase, pixels_per_second, track_left);
                        pointer.x >= x0 && pointer.x < x1
                    })
                    .map(|clip| clip.id);
            }
        }
    }
    outcome
}

/// The smallest ruler step whose labels do not collide at this zoom.
pub fn ruler_step(pixels_per_second: f32) -> f64 {
    const STEPS: [f64; 13] = [
        0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0,
    ];
    STEPS
        .iter()
        .copied()
        .find(|step| step * f64::from(pixels_per_second) >= MIN_LABEL_GAP)
        .unwrap_or(600.0)
}

/// Where a tick sits, in pixels from the left edge of the strip.
pub fn ticks_to_x(ticks: Ticks, timebase: Timebase, pixels_per_second: f32, left: f32) -> f32 {
    left + timebase.to_seconds(ticks) as f32 * pixels_per_second
}

/// The tick under a pixel position. The caller clamps to the project.
pub fn x_to_ticks(x: f32, timebase: Timebase, pixels_per_second: f32, left: f32) -> Ticks {
    timebase.seconds(f64::from((x - left) / pixels_per_second))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruler_steps_grow_as_the_timeline_gets_longer() {
        assert_eq!(ruler_step(1000.0), 0.1);
        assert_eq!(ruler_step(100.0), 1.0);
        assert_eq!(ruler_step(10.0), 10.0);
    }

    #[test]
    fn a_tick_maps_to_a_pixel_and_back() {
        let timebase = Timebase::default_timebase();

        let x = ticks_to_x(Ticks(90_000), timebase, 100.0, 96.0);
        assert!((x - 196.0).abs() < 1e-3);
        assert_eq!(x_to_ticks(x, timebase, 100.0, 96.0), Ticks(90_000));

        let x = ticks_to_x(Ticks(225_000), timebase, 50.0, 10.0);
        assert_eq!(x_to_ticks(x, timebase, 50.0, 10.0), Ticks(225_000));
    }
}
