//! The prototype's colour variables — the branding phase's colour-spec pass.
//!
//! One place for every colour the window draws, named after the owner's
//! spec. The rules the spec carries are kept in code:
//! - Every background neutral stays achromatic (0% saturation): the canvas
//!   must not shift the colour perception of the video it shows.
//! - The accent appears on surfaces the spec reserves for it, and the spec's
//!   "text on Acid Volt" is `TEXT_ON_ACCENT` — dark ink inside accent
//!   elements.
//!
//! Variables the prototype does not draw yet (hover fill, range tint, moss
//! borders) are kept here because they name widgets the prototype doesn't
//! have: when the trim bar lands, `ACCENT_HOVER` applies to its hover
//! states.

use eframe::egui::Color32;

/// Viewport surround & timeline well — the deepest obsidian canvas behind
/// the video preview and the timeline tracks.
pub const BG_VIEWPORT: Color32 = Color32::from_rgb(0x0A, 0x0B, 0x0A);

/// Primary dock / inspector panel surfaces — the timeline's label column.
pub const BG_PANEL: Color32 = Color32::from_rgb(0x14, 0x15, 0x14);

/// Elevated transport bars — the top toolbar.
pub const BG_TOOLBAR: Color32 = Color32::from_rgb(0x1E, 0x20, 0x1E);

/// Track lane — the stacked track rows (and the alternating stripe).
pub const TRACK_LANE: Color32 = Color32::from_rgb(0x18, 0x1A, 0x18);

/// 1px structural borders separating panels and track headers.
pub const BORDER: Color32 = Color32::from_rgb(0x2A, 0x2D, 0x2A);

/// Primary accent: playhead, selected clips, pressed states.
pub const ACCENT: Color32 = Color32::from_rgb(0xC8, 0xF9, 0x6D);

/// Hover fill of accent elements.
#[allow(dead_code)] // the prototype has no hover-only accent state yet
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0xB4, 0xE8, 0x54);

/// Selected-range tint. 0.16 alpha ≈ 41/255 (the spec's `rgba(200, 249, 109, 0.16)`).
#[allow(dead_code)] // the prototype has no range selection or marquee yet
pub const ACCENT_TINT: Color32 = Color32::from_rgba_unmultiplied_const(200, 249, 109, 41);

/// Secondary badges, muted accent borders, toggle tracks.
#[allow(dead_code)] // the prototype has no badges, audio clips or toggles yet
pub const MOSS_VOLT: Color32 = Color32::from_rgb(0x3D, 0x5A, 0x00);

/// Primary text and active icons.
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xF4, 0xF4, 0xF4);

/// Secondary text: clip metadata, ruler ticks, parameter labels.
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(0x9C, 0xA3, 0x9C);

/// Disabled / locked text.
pub const TEXT_DISABLED: Color32 = Color32::from_rgb(0x52, 0x57, 0x52);

/// Dark ink inside Acid Volt elements (the ▶ button while playing).
pub const TEXT_ON_ACCENT: Color32 = Color32::from_rgb(0x0A, 0x0B, 0x0A);

/// Video clip block fill — desaturated slate-sage.
pub const CLIP_BLOCK: Color32 = Color32::from_rgb(0x23, 0x2B, 0x26);

/// Video clip block's muted sage top edge.
pub const CLIP_TOP_EDGE: Color32 = Color32::from_rgb(0x4E, 0x6E, 0x58);

/// Destructive actions (the Delete button) — strictly reserved for them.
pub const ALERT_RED: Color32 = Color32::from_rgb(0xFF, 0x5A, 0x5F);
