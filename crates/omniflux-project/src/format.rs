//! The `.omni` file format: what is written, and how it is read back.
//!
//! A project file is JSON text with a header that says what it is:
//!
//! ```json
//! {
//!   "format": "omniflux-project",
//!   "version": 1,
//!   "timebase": { "ticks_per_second": 90000 },
//!   "media": [ { "id": 0, "duration": 5400000, "path": "footage/a.mp4" } ],
//!   "tracks": [
//!     { "id": 0, "kind": "video", "name": "Video 1",
//!       "clips": [ { "id": 0, "media": 0, "start": 0, "duration": 2700000, "source_in": 0 } ] }
//!   ]
//! }
//! ```
//!
//! Why JSON text and not a binary blob: the owner of a video project should
//! be able to open the file in a text editor, see what it says, and fix it
//! with their own hands if they have to. Git can show what changed between
//! two versions of a project. A binary format would be smaller and faster,
//! and would be the right answer once projects get big enough for that to
//! matter — but then, and not before.
//!
//! The rules the format keeps:
//! - `format` and `version` say what this file is; a file from a newer
//!   OmniFlux is refused rather than half-read.
//! - Media paths are stored relative to the project file when the media
//!   lives inside the project folder, so a project can be moved as a whole.
//! - Tracks are listed in id order, so reading a file gives back the same
//!   track ids it was written with.
//! - Every number is an integer: positions are ticks, never seconds.
//! - The bytes are deterministic: saving the same project twice produces the
//!   same file, byte for byte (the JSON object keys come out in one fixed
//!   order). That is what makes a git diff of a project file meaningful.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use omniflux_core::time::{Ticks, Timebase};
use omniflux_core::timeline::{Clip, ClipId, MediaId, Timeline, TrackKind};

use crate::error::ProjectError;
use crate::Project;

/// The name that says "this is an OmniFlux project file".
pub const FORMAT_NAME: &str = "omniflux-project";

/// The version of the format this build writes and understands.
pub const FORMAT_VERSION: u64 = 1;

/// Turns a project into the JSON that gets written to disk.
pub fn to_value(project: &Project, project_dir: &Path) -> Value {
    let media: Vec<Value> = project
        .timeline()
        .media()
        .iter()
        .map(|asset| {
            let mut entry = Map::new();
            entry.insert("id".into(), json!(asset.id.0));
            entry.insert("duration".into(), json!(asset.duration.0));
            if let Some(location) = project.location(asset.id) {
                entry.insert(
                    "path".into(),
                    json!(path_for_file(project_dir, &location.path)),
                );
            }
            Value::Object(entry)
        })
        .collect();

    let tracks: Vec<Value> = project
        .timeline()
        .tracks()
        .iter()
        .map(|track| {
            let clips: Vec<Value> = track
                .clips()
                .iter()
                .map(|clip| {
                    json!({
                        "id": clip.id.0,
                        "media": clip.media.0,
                        "start": clip.start.0,
                        "duration": clip.duration.0,
                        "source_in": clip.source_in.0,
                    })
                })
                .collect();
            json!({
                "id": track.id.0,
                "kind": match track.kind {
                    TrackKind::Video => "video",
                    TrackKind::Audio => "audio",
                },
                "name": track.name.clone(),
                "clips": clips,
            })
        })
        .collect();

    json!({
        "format": FORMAT_NAME,
        "version": FORMAT_VERSION,
        "timebase": {
            "ticks_per_second": project.timeline().timebase().ticks_per_second(),
        },
        "media": media,
        "tracks": tracks,
    })
}

/// Reads a project back out of the JSON in a file.
pub fn from_value(value: &Value, project_dir: &Path) -> Result<Project, ProjectError> {
    let format = as_str(field(value, "format", "the file")?, "format")?;
    if format != FORMAT_NAME {
        return Err(ProjectError::Malformed {
            detail: format!("format: expected {FORMAT_NAME:?}, found {format:?}"),
        });
    }
    let version = as_u64(field(value, "version", "the file")?, "version")?;
    if version > FORMAT_VERSION {
        return Err(ProjectError::NewerFormat {
            found: version,
            understood: FORMAT_VERSION,
        });
    }

    let timebase_value = field(value, "timebase", "the file")?;
    let ticks_per_second = as_u32(
        field(timebase_value, "ticks_per_second", "timebase")?,
        "timebase.ticks_per_second",
    )?;
    let timebase = Timebase::new(ticks_per_second).map_err(|error| ProjectError::Model {
        detail: error.to_string(),
    })?;
    let mut timeline = Timeline::new(timebase);
    let mut locations = Vec::new();

    for (index, entry) in as_array(field(value, "media", "the file")?, "media")?
        .iter()
        .enumerate()
    {
        let place = format!("media[{index}]");
        let id = MediaId(as_u32(field(entry, "id", &place)?, &place)?);
        let duration = Ticks(as_i64(field(entry, "duration", &place)?, &place)?);
        timeline
            .add_media(id, duration)
            .map_err(|error| ProjectError::Model {
                detail: format!("{place}: {error}"),
            })?;
        if let Some(path_value) = entry.get("path") {
            let stored = as_str(path_value, &format!("{place}.path"))?;
            locations.push((id, resolve_path(project_dir, stored)));
        }
    }

    for (index, entry) in as_array(field(value, "tracks", "the file")?, "tracks")?
        .iter()
        .enumerate()
    {
        let place = format!("tracks[{index}]");
        let id = as_u32(field(entry, "id", &place)?, &place)?;
        if id as usize != index {
            return Err(ProjectError::Malformed {
                detail: format!(
                    "{place}.id: tracks must be listed in order (expected id {index}, found {id})"
                ),
            });
        }
        let kind = match as_str(field(entry, "kind", &place)?, &format!("{place}.kind"))? {
            "video" => TrackKind::Video,
            "audio" => TrackKind::Audio,
            other => {
                return Err(ProjectError::Malformed {
                    detail: format!(
                        "{place}.kind: expected \"video\" or \"audio\", found {other:?}"
                    ),
                })
            }
        };
        let name = as_str(field(entry, "name", &place)?, &format!("{place}.name"))?;
        let track = timeline.add_track(kind, name);

        for (clip_index, clip_value) in
            as_array(field(entry, "clips", &place)?, &format!("{place}.clips"))?
                .iter()
                .enumerate()
        {
            let clip_place = format!("{place}.clips[{clip_index}]");
            let clip = Clip {
                id: ClipId(as_u32(field(clip_value, "id", &clip_place)?, &clip_place)?),
                media: MediaId(as_u32(
                    field(clip_value, "media", &clip_place)?,
                    &clip_place,
                )?),
                start: Ticks(as_i64(
                    field(clip_value, "start", &clip_place)?,
                    &clip_place,
                )?),
                duration: Ticks(as_i64(
                    field(clip_value, "duration", &clip_place)?,
                    &clip_place,
                )?),
                source_in: Ticks(as_i64(
                    field(clip_value, "source_in", &clip_place)?,
                    &clip_place,
                )?),
            };
            timeline
                .insert_clip(track, clip)
                .map_err(|error| ProjectError::Model {
                    detail: format!("{clip_place}: {error}"),
                })?;
        }
    }

    timeline.validate().map_err(|error| ProjectError::Model {
        detail: error.to_string(),
    })?;

    let mut project = Project::new(timeline);
    for (media, path) in locations {
        project.set_location(media, path);
    }
    Ok(project)
}

/// The path to store in the file: relative to the project file when the
/// media lives inside the project folder, absolute otherwise.
fn path_for_file(project_dir: &Path, path: &Path) -> String {
    match path.strip_prefix(project_dir) {
        Ok(relative) => relative.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// The path to use: absolute, resolved against the project file's folder.
fn resolve_path(project_dir: &Path, stored: &str) -> PathBuf {
    let path = PathBuf::from(stored);
    if path.is_absolute() {
        path
    } else {
        project_dir.join(path)
    }
}

/// One field of a JSON object, or a precise complaint about it.
fn field<'a>(value: &'a Value, name: &str, place: &str) -> Result<&'a Value, ProjectError> {
    value.get(name).ok_or_else(|| ProjectError::Malformed {
        detail: format!("{place}: missing field {name:?}"),
    })
}

fn as_u64(value: &Value, place: &str) -> Result<u64, ProjectError> {
    value.as_u64().ok_or_else(|| ProjectError::Malformed {
        detail: format!(
            "{place}: expected a whole number, found {}",
            describe(value)
        ),
    })
}

fn as_u32(value: &Value, place: &str) -> Result<u32, ProjectError> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| ProjectError::Malformed {
            detail: format!(
                "{place}: expected a whole number, found {}",
                describe(value)
            ),
        })
}

fn as_i64(value: &Value, place: &str) -> Result<i64, ProjectError> {
    value.as_i64().ok_or_else(|| ProjectError::Malformed {
        detail: format!("{place}: expected a number, found {}", describe(value)),
    })
}

fn as_str<'a>(value: &'a Value, place: &str) -> Result<&'a str, ProjectError> {
    value.as_str().ok_or_else(|| ProjectError::Malformed {
        detail: format!("{place}: expected text, found {}", describe(value)),
    })
}

fn as_array<'a>(value: &'a Value, place: &str) -> Result<&'a Vec<Value>, ProjectError> {
    value.as_array().ok_or_else(|| ProjectError::Malformed {
        detail: format!("{place}: expected a list, found {}", describe(value)),
    })
}

/// What was found instead of what was expected, for the error message.
fn describe(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => format!("{text:?}"),
        Value::Array(_) => "a list".to_string(),
        Value::Object(_) => "an object".to_string(),
    }
}
