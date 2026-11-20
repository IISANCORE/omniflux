//! The prototype window: one video, one track, and the engine underneath.
//!
//! One job: wire the playback crate and the timeline model to buttons a
//! person can press, so the engine can be tested in a real window. Not
//! responsible for: final UI, branding, audio, multiple tracks, effects, or
//! project opening — none of that is decided yet, and this window must not
//! pretend otherwise.
//!
//! What is real here: every frame comes from the shipped warm decoder, every
//! edit goes through the core history (so undo is the engine's undo), and
//! every number in the status bar is measured at runtime rather than
//! estimated. What is prototype: the layout, the colors, the single track,
//! and the blocking open.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Key, Modifiers, RichText, TextureHandle, TextureOptions};
use omniflux_core::edit::History;
use omniflux_core::time::{Ticks, Timebase};
use omniflux_core::timeline::{Clip, ClipId, MediaId, Timeline, TrackKind};
use omniflux_playback::player::DecodedFrame;
use omniflux_project::Project;

use crate::edit_actions;
use crate::media::{FrameImage, MediaSession};
use crate::preview;
use crate::time_text;
use crate::timeline_view;

/// The one source asset id this prototype uses.
const MEDIA: MediaId = MediaId(1);

/// The prototype's application state.
pub struct OmniFluxApp {
    /// Where FFmpeg was found, or `None` with `error` saying why.
    ffmpeg: Option<PathBuf>,
    media: Option<MediaSession>,
    project: Project,
    history: History,
    /// The pixels of the frame currently on screen.
    current: Option<FrameImage>,
    texture: Option<TextureHandle>,
    texture_dirty: bool,
    playhead: Ticks,
    /// The furthest point the ruler draws. Kept separately because
    /// `Timeline::duration()` drops to zero when the last clip is deleted,
    /// and a ruler that collapses to nothing is not useful.
    timeline_span: Ticks,
    playing: bool,
    last_step_at: Instant,
    fps_window_start: Instant,
    fps_frames: u32,
    measured_fps: Option<f64>,
    selected: Option<ClipId>,
    project_path: Option<PathBuf>,
    media_path_input: String,
    last_scrub_x: Option<f32>,
    open_ms: Option<f64>,
    last_decode_ms: Option<f64>,
    last_decode_packets: usize,
    status: String,
    error: Option<String>,
}

impl OmniFluxApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_media: Option<PathBuf>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (ffmpeg, error) = resolve_ffmpeg();
        let mut app = OmniFluxApp {
            ffmpeg,
            media: None,
            project: Project::new(Timeline::new(Timebase::default_timebase())),
            history: History::new(),
            current: None,
            texture: None,
            texture_dirty: false,
            playhead: Ticks::zero(),
            timeline_span: Ticks::zero(),
            playing: false,
            last_step_at: Instant::now(),
            fps_window_start: Instant::now(),
            fps_frames: 0,
            measured_fps: None,
            selected: None,
            project_path: None,
            media_path_input: String::new(),
            last_scrub_x: None,
            open_ms: None,
            last_decode_ms: None,
            last_decode_packets: 0,
            status: "Open a video to begin. Drag and drop works, and so does the path bar."
                .to_string(),
            error,
        };
        if let Some(path) = initial_media {
            app.media_path_input = path.display().to_string();
            if path
                .extension()
                .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("omni"))
            {
                app.error = Some(
                    "This prototype saves .omni projects but does not open them yet. Open the \
                     source video instead."
                        .to_string(),
                );
            } else {
                app.open_media(path);
            }
        }
        app
    }

    fn open_from_input(&mut self) {
        let raw = self.media_path_input.trim().trim_matches('"').to_string();
        if raw.is_empty() {
            self.error = Some("Type a path to a video file first.".to_string());
            return;
        }
        self.open_media(PathBuf::from(raw));
    }

    fn open_media(&mut self, path: PathBuf) {
        if !path.is_file() {
            self.error = Some(format!("There is no file at {}", path.display()));
            return;
        }
        let Some(ffmpeg) = self.ffmpeg.clone() else {
            self.error = Some(
                "FFmpeg is not available. Set OMNIFLUX_FFMPEG, put it in tools/ffmpeg/bin, or \
                 install it on PATH."
                    .to_string(),
            );
            return;
        };

        self.playing = false;
        self.status = format!("Opening {} …", file_label(&path));
        let started = Instant::now();
        match MediaSession::open(&ffmpeg, path.clone()) {
            Ok((media, first)) => {
                let open_ms = started.elapsed().as_secs_f64() * 1000.0;
                let timebase = Timebase::default_timebase();
                let duration = timebase.seconds(media.info.duration);
                if duration <= Ticks::zero() {
                    self.error = Some(format!(
                        "{} has no measurable video duration.",
                        path.display()
                    ));
                    return;
                }

                let mut timeline = Timeline::new(timebase);
                if let Err(error) = timeline.add_media(MEDIA, duration) {
                    self.error = Some(error.to_string());
                    return;
                }
                let track = timeline.add_track(TrackKind::Video, "Video 1");
                let first_clip = Clip {
                    id: ClipId(1),
                    media: MEDIA,
                    start: Ticks::zero(),
                    duration,
                    source_in: Ticks::zero(),
                };
                if let Err(error) = timeline.insert_clip(track, first_clip) {
                    self.error = Some(error.to_string());
                    return;
                }

                let mut project = Project::new(timeline);
                project.set_location(MEDIA, path.clone());

                self.project = project;
                self.history = History::new();
                self.project_path = None;
                self.selected = Some(ClipId(1));
                self.playhead = Ticks::zero();
                self.timeline_span = duration;
                self.last_scrub_x = None;
                self.open_ms = Some(open_ms);
                self.last_decode_ms = None;
                self.last_decode_packets = 0;
                self.measured_fps = None;
                self.fps_frames = 0;
                self.fps_window_start = Instant::now();
                self.last_step_at = Instant::now();
                self.texture = None;
                self.current = None;
                self.media_path_input = path.display().to_string();
                self.media = Some(media);
                self.accept_frame(first);
                self.status = format!(
                    "Opened {} · {:.0} ms to first frame",
                    file_label(&path),
                    open_ms
                );
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// Puts a decoded frame on screen and moves the playhead to its exact
    /// presentation time. The playhead is the frame, not a pixel estimate.
    fn accept_frame(&mut self, frame: DecodedFrame) {
        let (width, height) = self
            .media
            .as_ref()
            .map(|media| (media.info.width, media.info.height))
            .unwrap_or((0, 0));
        if frame.packets_decoded > 0 {
            self.last_decode_ms = Some(frame.latency.as_secs_f64() * 1000.0);
            self.last_decode_packets = frame.packets_decoded;
        }
        let timebase = self.project.timeline().timebase();
        self.playhead = timebase
            .seconds(frame.time)
            .clamp(Ticks::zero(), self.timeline_span);
        self.current = Some(FrameImage::from_decoded(frame, width, height));
        self.texture_dirty = true;
        self.error = None;
    }

    fn seek_to(&mut self, ticks: Ticks) {
        let clamped = ticks.clamp(Ticks::zero(), self.timeline_span);
        let seconds = self.project.timeline().timebase().to_seconds(clamped);
        let Some(media) = self.media.as_mut() else {
            return;
        };
        match media.seek(seconds) {
            Ok(frame) => self.accept_frame(frame),
            Err(error) => self.error = Some(error),
        }
    }

    fn step(&mut self, direction: i32) {
        self.playing = false;
        let Some(frame_duration) = self.media.as_ref().map(|media| media.info.frame_duration)
        else {
            return;
        };
        if direction > 0 {
            let next = self.media.as_mut().map(|media| media.next_frame());
            match next {
                Some(Ok(Some(frame))) => self.accept_frame(frame),
                Some(Ok(None)) => self.status = "Already at the last reachable frame.".to_string(),
                Some(Err(error)) => self.error = Some(error),
                None => {}
            }
        } else {
            let step = self.project.timeline().timebase().seconds(frame_duration);
            self.seek_to((self.playhead - step).max(Ticks::zero()));
        }
    }

    fn toggle_play(&mut self) {
        if self.media.is_none() {
            self.status = "Open a video first.".to_string();
            return;
        }
        self.playing = !self.playing;
        if self.playing {
            self.last_step_at = Instant::now();
            self.fps_window_start = Instant::now();
            self.fps_frames = 0;
            self.measured_fps = None;
            self.status = "Playing.".to_string();
        } else {
            self.status = "Paused.".to_string();
        }
    }

    fn advance_playback(&mut self, ctx: &egui::Context) {
        if !self.playing {
            return;
        }
        let Some(frame_duration) = self.media.as_ref().map(|media| media.info.frame_duration)
        else {
            self.playing = false;
            return;
        };
        let frame_duration = frame_duration.max(1.0 / 240.0);
        let now = Instant::now();
        if now.duration_since(self.last_step_at).as_secs_f64() < frame_duration {
            ctx.request_repaint_after(Duration::from_secs_f64((frame_duration / 2.0).max(0.001)));
            return;
        }

        let next = self.media.as_mut().map(|media| media.next_frame());
        match next {
            Some(Ok(Some(frame))) => {
                self.accept_frame(frame);
                self.last_step_at = now;
                self.fps_frames += 1;
                let elapsed = now.duration_since(self.fps_window_start).as_secs_f64();
                if elapsed >= 0.5 {
                    self.measured_fps = Some(f64::from(self.fps_frames) / elapsed);
                    self.fps_frames = 0;
                    self.fps_window_start = now;
                }
            }
            Some(Ok(None)) => {
                self.playing = false;
                self.status = "Reached the end of the file.".to_string();
            }
            Some(Err(error)) => {
                self.playing = false;
                self.error = Some(error);
            }
            None => self.playing = false,
        }
        ctx.request_repaint_after(Duration::from_secs_f64((frame_duration / 2.0).max(0.001)));
    }

    fn split(&mut self) {
        self.playing = false;
        let Some(id) = self.selected else {
            self.status = "Select a clip first.".to_string();
            return;
        };
        let plan = match edit_actions::split_command(self.project.timeline(), id, self.playhead) {
            Ok(plan) => plan,
            Err(reason) => {
                self.status = reason;
                return;
            }
        };
        let right = plan.right;
        match self
            .history
            .execute(self.project.timeline_mut(), plan.command)
        {
            Ok(()) => {
                self.selected = Some(right);
                let seconds = self.project.timeline().timebase().to_seconds(self.playhead);
                self.status = format!(
                    "Split clip {} at {}.",
                    id.0,
                    time_text::format_clock(seconds)
                );
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn delete_selected(&mut self) {
        self.playing = false;
        let Some(id) = self.selected else {
            self.status = "Select a clip first.".to_string();
            return;
        };
        let command = match edit_actions::delete_command(self.project.timeline(), id) {
            Ok(command) => command,
            Err(reason) => {
                self.status = reason;
                return;
            }
        };
        match self.history.execute(self.project.timeline_mut(), command) {
            Ok(()) => {
                self.selected = None;
                self.status = format!("Deleted clip {}.", id.0);
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn undo(&mut self) {
        self.playing = false;
        let label = self
            .history
            .undo_labels()
            .first()
            .copied()
            .unwrap_or("edit");
        match self.history.undo(self.project.timeline_mut()) {
            Ok(true) => {
                self.prune_selection();
                self.status = format!("Undid: {label}.");
                self.error = None;
            }
            Ok(false) => self.status = "Nothing to undo.".to_string(),
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn redo(&mut self) {
        self.playing = false;
        match self.history.redo(self.project.timeline_mut()) {
            Ok(true) => {
                self.prune_selection();
                self.status = "Redid the last edit.".to_string();
                self.error = None;
            }
            Ok(false) => self.status = "Nothing to redo.".to_string(),
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn prune_selection(&mut self) {
        if self
            .selected
            .is_some_and(|id| self.project.timeline().clip(id).is_none())
        {
            self.selected = None;
        }
    }

    fn save(&mut self) {
        if self.media.is_none() {
            self.status = "Open a video before saving.".to_string();
            return;
        }
        let path = self.project_path.clone().or_else(|| {
            self.media
                .as_ref()
                .map(|media| media.path.with_extension("omni"))
        });
        let Some(path) = path else {
            return;
        };
        let started = Instant::now();
        match self.project.save_to(&path) {
            Ok(()) => {
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                self.project_path = Some(path.clone());
                self.status = format!("Saved {} · {:.1} ms", path.display(), elapsed_ms);
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn selected_clip_contains_playhead(&self) -> bool {
        self.selected
            .and_then(|id| self.project.timeline().clip(id))
            .is_some_and(|clip| clip.start < self.playhead && self.playhead < clip.end())
    }

    fn clip_count(&self) -> usize {
        self.project
            .timeline()
            .tracks()
            .iter()
            .map(|track| track.clips().len())
            .sum()
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .first()
                .map(|file| file.path().to_path_buf())
        });
        if let Some(path) = dropped {
            self.media_path_input = path.display().to_string();
            if path
                .extension()
                .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("omni"))
            {
                self.error = Some(
                    "This prototype saves .omni projects but does not open them yet. Open the \
                     source video instead."
                        .to_string(),
                );
            } else {
                self.open_media(path);
            }
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let ctrl_shift = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        };
        // Ctrl+Shift+Z first: `consume_key` treats extra modifiers as a
        // match, so checking Ctrl+Z first would swallow the redo shortcut.
        if ctx.input_mut(|input| input.consume_key(ctrl_shift, Key::Z)) {
            self.redo();
            return;
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::Z)) {
            self.undo();
            return;
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::Y)) {
            self.redo();
            return;
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::S)) {
            self.save();
            return;
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::K)) {
            self.split();
            return;
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Space)) {
            self.toggle_play();
            return;
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Delete)) {
            self.delete_selected();
            return;
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::ArrowRight)) {
            self.step(1);
            return;
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::ArrowLeft)) {
            self.step(-1);
        }
    }

    fn upload_frame(&mut self, ctx: &egui::Context) {
        if !self.texture_dirty {
            return;
        }
        let Some(frame) = &self.current else {
            return;
        };
        let image =
            egui::ColorImage::from_rgb([frame.width as usize, frame.height as usize], &frame.bytes);
        match &mut self.texture {
            Some(texture) => texture.set(image, TextureOptions::LINEAR),
            None => {
                self.texture = Some(ctx.load_texture("preview", image, TextureOptions::LINEAR));
            }
        }
        self.texture_dirty = false;
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("OmniFlux").strong());
            ui.label(RichText::new("UI prototype").small().weak());
            ui.separator();
            if ui.button("Open video").clicked() {
                self.open_from_input();
            }
            let path_width = (ui.available_width() - 240.0).clamp(160.0, 560.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.media_path_input)
                    .hint_text("path to an H.264 MP4")
                    .desired_width(path_width),
            );
            if ui
                .add_enabled(self.media.is_some(), egui::Button::new("Save .omni"))
                .on_hover_text("Ctrl+S")
                .clicked()
            {
                self.save();
            }
            if ui
                .add_enabled(self.history.undo_depth() > 0, egui::Button::new("Undo"))
                .on_hover_text("Ctrl+Z")
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(self.history.redo_depth() > 0, egui::Button::new("Redo"))
                .on_hover_text("Ctrl+Shift+Z")
                .clicked()
            {
                self.redo();
            }
        });
        ui.horizontal(|ui| {
            let has_media = self.media.is_some();
            let play_label = if self.playing { "Pause" } else { "Play" };
            if ui
                .add_enabled(has_media, egui::Button::new(play_label))
                .on_hover_text("Space")
                .clicked()
            {
                self.toggle_play();
            }
            if ui
                .add_enabled(has_media, egui::Button::new("Frame −"))
                .on_hover_text("Left arrow")
                .clicked()
            {
                self.step(-1);
            }
            if ui
                .add_enabled(has_media, egui::Button::new("Frame +"))
                .on_hover_text("Right arrow")
                .clicked()
            {
                self.step(1);
            }
            ui.separator();
            let can_split = self.selected_clip_contains_playhead();
            if ui
                .add_enabled(can_split, egui::Button::new("Split"))
                .on_hover_text("Ctrl+K")
                .clicked()
            {
                self.split();
            }
            if ui
                .add_enabled(self.selected.is_some(), egui::Button::new("Delete clip"))
                .on_hover_text("Delete")
                .clicked()
            {
                self.delete_selected();
            }
            ui.separator();
            if let Some(media) = &self.media {
                let name = media
                    .path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("media");
                ui.label(RichText::new(name).small().weak());
            } else {
                ui.label(RichText::new("no video open").small().weak());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new("PROTOTYPE — not final UI or branding")
                        .small()
                        .weak(),
                );
            });
        });
        ui.add_space(2.0);
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.status.as_str()).small());
        });
        ui.horizontal(|ui| {
            let timebase = self.project.timeline().timebase();
            ui.monospace(format!(
                "{} / {}",
                time_text::format_clock(timebase.to_seconds(self.playhead)),
                time_text::format_clock(timebase.to_seconds(self.timeline_span))
            ));
            if let Some(media) = &self.media {
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "{}×{} · {:.2} fps · reorder {}",
                        media.info.width,
                        media.info.height,
                        1.0 / media.info.frame_duration.max(1e-9),
                        media.info.reorder_depth
                    ))
                    .small(),
                );
            }
            if let Some(ms) = self.last_decode_ms {
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "decode {ms:.1} ms · {} pkt",
                        self.last_decode_packets
                    ))
                    .small(),
                );
            }
            if let Some(ms) = self.open_ms {
                ui.separator();
                ui.label(RichText::new(format!("open {ms:.0} ms")).small());
            }
            if let Some(fps) = self.measured_fps {
                ui.separator();
                ui.label(RichText::new(format!("play {fps:.1} fps")).small());
            }
            ui.separator();
            ui.label(
                RichText::new(format!(
                    "{} clips · undo {} / redo {}",
                    self.clip_count(),
                    self.history.undo_depth(),
                    self.history.redo_depth()
                ))
                .small(),
            );
        });
    }

    fn timeline_panel(&mut self, ui: &mut egui::Ui) {
        let outcome = timeline_view::show(
            ui,
            self.project.timeline(),
            self.timeline_span,
            self.playhead,
            self.selected,
        );
        if let Some(clip) = outcome.select {
            self.selected = Some(clip);
        }
        match outcome.pointer_x {
            Some(x) => {
                if self.last_scrub_x != Some(x) {
                    self.last_scrub_x = Some(x);
                    if let Some(ticks) = outcome.seek_to {
                        self.seek_to(ticks);
                    }
                }
            }
            None => self.last_scrub_x = None,
        }
    }

    fn preview_panel(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = self.error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(Color32::from_rgb(240, 130, 120), error);
                if ui.small_button("Dismiss").clicked() {
                    self.error = None;
                }
            });
            ui.add_space(4.0);
        }
        let has_clip = !self.project.timeline().clips_at(self.playhead).is_empty();
        let dropping = ui.ctx().input(|input| !input.raw.hovered_files.is_empty());
        preview::show(
            ui,
            self.texture.as_ref(),
            self.media.is_some(),
            has_clip,
            dropping,
            self.ffmpeg.is_some(),
        );
    }
}

impl eframe::App for OmniFluxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_dropped_files(&ctx);
        self.handle_keys(&ctx);
        self.advance_playback(&ctx);
        self.upload_frame(&ctx);

        egui::Panel::top("omniflux-toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("omniflux-status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::bottom("omniflux-timeline")
            .exact_size(150.0)
            .resizable(false)
            .show(ui, |ui| self.timeline_panel(ui));
        egui::CentralPanel::default().show(ui, |ui| self.preview_panel(ui));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Color32::from_gray(8).to_normalized_gamma_f32()
    }
}

fn resolve_ffmpeg() -> (Option<PathBuf>, Option<String>) {
    match omniflux_playback::locate_ffmpeg() {
        Ok(path) => {
            if omniflux_playback::probe_usable(&path) {
                (Some(path), None)
            } else {
                (
                    None,
                    Some(format!(
                        "FFmpeg was found at {} but it would not run. Set OMNIFLUX_FFMPEG to a \
                         working binary.",
                        path.display()
                    )),
                )
            }
        }
        Err(error) => (
            None,
            Some(format!(
                "{error}. Set OMNIFLUX_FFMPEG, put ffmpeg in tools/ffmpeg/bin, or install it on \
                 PATH."
            )),
        ),
    }
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("media")
        .to_string()
}
