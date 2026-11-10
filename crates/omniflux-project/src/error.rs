//! The one error type this crate returns.
//!
//! Every variant says what went wrong AND where: a project file that
//! cannot be loaded must tell the user which file and which field, or the
//! report is useless at 2 AM.

use std::path::PathBuf;

/// Everything that can go wrong reading or writing a project file.
#[derive(Debug)]
pub enum ProjectError {
    /// The file could not be read or written.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The file is not a project file this build understands: it is not
    /// JSON, or a field is missing or the wrong shape.
    ///
    /// `detail` names the place precisely — "tracks[0].clips[2].start:
    /// expected a number, found null".
    Malformed { detail: String },
    /// The file is a project, but a newer version than this build knows.
    NewerFormat { found: u64, understood: u64 },
    /// The file parsed, but describes a project the model refuses — for
    /// example clips that overlap on one track.
    Model { detail: String },
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::Io { path, source } => {
                write!(f, "could not use {}: {source}", path.display())
            }
            ProjectError::Malformed { detail } => write!(f, "malformed project file: {detail}"),
            ProjectError::NewerFormat { found, understood } => write!(
                f,
                "this project was written in format version {found}, but this build \
                 understands version {understood} — update OmniFlux to open it"
            ),
            ProjectError::Model { detail } => {
                write!(
                    f,
                    "this project file describes something impossible: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for ProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProjectError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
