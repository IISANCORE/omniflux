//! The preview: the frame under the playhead, fitted into the window.
//!
//! One job: paint one texture, letterboxed, with a plain message when there
//! is nothing to paint. Not responsible for: decoding (the player does
//! that) or for which frame should be shown (the app decides that).
//!
//! This is deliberately the least styled part of the prototype: the picture
//! is the product, and a frame that is not the right frame should be
//! obvious, not decorated.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, TextureHandle, Vec2};

use crate::colors::{BG_VIEWPORT, TEXT_DISABLED, TEXT_PRIMARY, TEXT_SECONDARY};

/// Draws the preview area.
///
/// `has_clip` is about the timeline arrangement, not the file: a playhead
/// sitting in a gap has no picture even though the file is open.
pub fn show(
    ui: &mut egui::Ui,
    texture: Option<&TextureHandle>,
    has_media: bool,
    has_clip: bool,
    dropping: bool,
    ffmpeg_available: bool,
) {
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, BG_VIEWPORT);

    if !ffmpeg_available {
        centered(
            ui,
            rect,
            "FFmpeg was not found.\n\nSet OMNIFLUX_FFMPEG, put ffmpeg in tools/ffmpeg/bin,\nor install it on PATH.",
            TEXT_SECONDARY,
        );
    } else if !has_media {
        centered(
            ui,
            rect,
            "Drop a video here,\nor type a path in the bar above.",
            TEXT_SECONDARY,
        );
    } else if !has_clip {
        centered(ui, rect, "No clip at the playhead.", TEXT_SECONDARY);
    } else if let Some(texture) = texture {
        let image_size = texture.size_vec2();
        if image_size.x > 0.0 && image_size.y > 0.0 {
            let scale = (rect.width() / image_size.x).min(rect.height() / image_size.y);
            let drawn = Vec2::new(image_size.x * scale, image_size.y * scale);
            let image_rect = Rect::from_center_size(rect.center(), drawn);
            painter.image(
                texture.id(),
                image_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }

    // A mark that this is not the finished product, on every screenshot.
    painter.text(
        rect.right_bottom() + Vec2::new(-8.0, -6.0),
        Align2::RIGHT_BOTTOM,
        "UI prototype",
        FontId::proportional(10.0),
        TEXT_DISABLED,
    );

    if dropping {
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(170));
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Drop to open",
            FontId::proportional(24.0),
            TEXT_PRIMARY,
        );
    }
}

fn centered(ui: &egui::Ui, rect: Rect, text: &str, color: Color32) {
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(16.0),
        color,
    );
}
