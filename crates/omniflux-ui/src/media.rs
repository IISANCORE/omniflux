//! One open source file and the frame currently on screen.
//!
//! One job: hold the playback crate's `Player` together with the facts the
//! window needs (where the file is, how big it is, how long it is). Not
//! responsible for: the timeline arrangement or for uploading pixels to the
//! GPU (the app does that).
//!
//! The decoder is the same warm decoder the engine ships; opening is the
//! one blocking operation in the prototype, and the app measures it rather
//! than hiding it.

use std::path::{Path, PathBuf};

use omniflux_playback::player::{DecodedFrame, Player, PlayerInfo};

/// One open media file.
pub struct MediaSession {
    pub path: PathBuf,
    pub player: Player,
    pub info: PlayerInfo,
}

impl MediaSession {
    /// Opens a file and decodes its first frame. Blocks until the decoder
    /// is warm; the caller should measure this, not pretend it is free.
    pub fn open(ffmpeg: &Path, path: PathBuf) -> Result<(Self, DecodedFrame), String> {
        let (player, first) = Player::open(ffmpeg, &path)?;
        let info = player.info();
        Ok((MediaSession { path, player, info }, first))
    }

    /// The frame at `seconds`, decoded exactly.
    pub fn seek(&mut self, seconds: f64) -> Result<DecodedFrame, String> {
        self.player.seek(seconds)
    }

    /// The next frame in presentation order, or `None` at the end.
    pub fn next_frame(&mut self) -> Result<Option<DecodedFrame>, String> {
        self.player.next_frame()
    }
}

/// The pixels of one frame, ready to become a texture.
pub struct FrameImage {
    pub width: u32,
    pub height: u32,
    /// Exactly `width * height * 3` bytes of rgb24.
    pub bytes: Vec<u8>,
}

impl FrameImage {
    pub fn from_decoded(frame: DecodedFrame, width: u32, height: u32) -> Self {
        FrameImage {
            width,
            height,
            bytes: frame.bytes,
        }
    }
}
