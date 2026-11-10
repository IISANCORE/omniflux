//! The `.omni` project file: what OmniFlux writes when you save, and what
//! it reads when you open.
//!
//! One job: turn a project into a file on disk and back again, without
//! losing anything and without destroying a file that already exists.
//! Not responsible for: decoding, rendering, or deciding what a project
//! should contain — that is the engine's business, not the file's.
//!
//! A project is two things: the timeline model (what is arranged where) and
//! the locations of the source files (where the footage lives). The model
//! itself knows only lengths and ids; a project file knows paths, because
//! that is what a file is for.
//!
//! Known limit: the file remembers where the footage WAS, it does not search
//! for it. A project whose files have moved opens with the arrangement
//! intact and the missing files to relink — relinking is not built yet.
//!
//! ```no_run
//! use omniflux_project::Project;
//! use std::path::Path;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let project = Project::load_from(Path::new("holiday.omni"))?;
//! println!("{} tracks", project.timeline().tracks().len());
//! project.save_to(Path::new("holiday-copy.omni"))?;
//! # Ok(())
//! # }
//! ```

pub mod error;
mod format;

use std::path::{Path, PathBuf};

use omniflux_core::timeline::{MediaId, Timeline};

pub use error::ProjectError;
pub use format::{FORMAT_NAME, FORMAT_VERSION};

/// The file extension OmniFlux project files use.
pub const PROJECT_FILE_EXTENSION: &str = "omni";

/// Where one source asset's file lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaLocation {
    /// Which source asset this is the file for.
    pub media: MediaId,
    /// The media file, absolute. A project file stores this relative to
    /// itself when it can, so a project folder can be moved or shared.
    pub path: PathBuf,
}

/// A project: the timeline model, plus where each source file lives.
#[derive(Debug, Clone)]
pub struct Project {
    timeline: Timeline,
    locations: Vec<MediaLocation>,
}

impl Project {
    /// A project with no source files located yet.
    pub fn new(timeline: Timeline) -> Self {
        Project {
            timeline,
            locations: Vec::new(),
        }
    }

    /// The timeline model.
    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    /// The timeline model, for editing.
    pub fn timeline_mut(&mut self) -> &mut Timeline {
        &mut self.timeline
    }

    /// Where every located source file lives.
    pub fn locations(&self) -> &[MediaLocation] {
        &self.locations
    }

    /// Where one source asset's file lives, if it is known.
    pub fn location(&self, media: MediaId) -> Option<&MediaLocation> {
        self.locations
            .iter()
            .find(|location| location.media == media)
    }

    /// Records where a source asset's file lives.
    pub fn set_location(&mut self, media: MediaId, path: PathBuf) {
        match self
            .locations
            .iter_mut()
            .find(|location| location.media == media)
        {
            Some(location) => location.path = path,
            None => self.locations.push(MediaLocation { media, path }),
        }
    }

    /// The text this project would be written as.
    ///
    /// `project_dir` is the folder the file would sit in: media inside that
    /// folder is written as a relative path, so the project stays portable.
    pub fn to_text(&self, project_dir: &Path) -> Result<String, ProjectError> {
        let value = format::to_value(self, project_dir);
        serde_json::to_string_pretty(&value).map_err(|error| ProjectError::Malformed {
            detail: format!("could not write the project as JSON: {error}"),
        })
    }

    /// Reads a project from the text of a project file.
    ///
    /// Relative media paths are resolved against `project_dir`, which is the
    /// folder the file sits in.
    pub fn from_text(text: &str, project_dir: &Path) -> Result<Project, ProjectError> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|error| ProjectError::Malformed {
                detail: format!("not valid JSON: {error}"),
            })?;
        format::from_value(&value, project_dir)
    }

    /// Writes the project to a file.
    ///
    /// The file is written to a temporary name first and then moved into
    /// place, so a crash halfway through cannot leave the user's project
    /// half-written.
    pub fn save_to(&self, path: &Path) -> Result<(), ProjectError> {
        let project_dir = project_folder(path);
        let text = self.to_text(project_dir)?;
        let temporary = path.with_extension("omni-tmp");

        std::fs::write(&temporary, text).map_err(|source| ProjectError::Io {
            path: temporary.clone(),
            source,
        })?;
        if let Err(source) = std::fs::rename(&temporary, path) {
            // The move failed; do not leave the temporary file behind.
            let _ = std::fs::remove_file(&temporary);
            return Err(ProjectError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
        Ok(())
    }

    /// Reads a project from a file.
    pub fn load_from(path: &Path) -> Result<Project, ProjectError> {
        let text = std::fs::read_to_string(path).map_err(|source| ProjectError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Project::from_text(&text, project_folder(path))
    }
}

/// The folder a project file sits in — the base for relative media paths.
fn project_folder(path: &Path) -> &Path {
    path.parent()
        .filter(|folder| !folder.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniflux_core::time::{Ticks, Timebase};
    use omniflux_core::timeline::{Clip, TrackKind};

    fn sample_project() -> Project {
        let mut timeline = Timeline::new(Timebase::default_timebase());
        timeline.add_media(MediaId(0), Ticks(90_000)).unwrap();
        let track = timeline.add_track(TrackKind::Video, "Video 1");
        timeline
            .insert_clip(
                track,
                Clip {
                    id: omniflux_core::timeline::ClipId(0),
                    media: MediaId(0),
                    start: Ticks::zero(),
                    duration: Ticks(1_000),
                    source_in: Ticks::zero(),
                },
            )
            .unwrap();
        let mut project = Project::new(timeline);
        // An absolute path on every platform: "/footage/a.mp4" is NOT
        // absolute on Windows (no drive letter), which would make this test
        // mean two different things on two operating systems.
        project.set_location(
            MediaId(0),
            std::env::temp_dir().join("footage").join("a.mp4"),
        );
        project
    }

    #[test]
    fn a_project_survives_a_round_trip_through_text() {
        let project = sample_project();
        let text = project.to_text(Path::new("/projects")).unwrap();
        let loaded = Project::from_text(&text, Path::new("/projects")).unwrap();

        assert_eq!(loaded.timeline(), project.timeline());
        assert_eq!(loaded.locations(), project.locations());
    }

    #[test]
    fn the_file_says_what_it_is() {
        let text = sample_project().to_text(Path::new(".")).unwrap();
        assert!(text.contains(FORMAT_NAME), "the file names its format");
        assert!(
            text.contains("\"version\": 1"),
            "the file names its version"
        );
    }

    #[test]
    fn a_file_that_is_not_json_is_refused_with_a_reason() {
        let error = Project::from_text("this is not json", Path::new(".")).unwrap_err();
        assert!(matches!(error, ProjectError::Malformed { .. }));
        assert!(error.to_string().contains("not valid JSON"));
    }

    #[test]
    fn a_file_from_a_newer_omniflux_is_refused_rather_than_half_read() {
        let text = r#"{"format": "omniflux-project", "version": 99}"#;
        let error = Project::from_text(text, Path::new(".")).unwrap_err();
        assert!(matches!(
            error,
            ProjectError::NewerFormat {
                found: 99,
                understood: FORMAT_VERSION
            }
        ));
    }

    #[test]
    fn a_missing_field_names_itself() {
        let text = r#"{"format": "omniflux-project", "version": 1}"#;
        let error = Project::from_text(text, Path::new(".")).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("timebase"),
            "the message names the place: {message}"
        );
    }

    #[test]
    fn media_inside_the_project_folder_is_stored_relative_to_it() {
        let mut project = sample_project();
        project.set_location(MediaId(0), PathBuf::from("/projects/holiday/footage/a.mp4"));
        let text = project.to_text(Path::new("/projects/holiday")).unwrap();
        assert!(
            text.contains("footage/a.mp4"),
            "the media path is relative to the project: {text}"
        );
    }
}
