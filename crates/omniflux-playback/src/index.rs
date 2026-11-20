//! The map-reader for the warm-decoder spike: turns a media file into a
//! packet index (decode order, with presentation times, byte positions and
//! keyframe flags), plus the H.264 parameter sets and the decoder's reorder
//! depth needed to feed it over stdin.
//!
//! Division of labour, stated honestly: ffprobe (same trusted build as our
//! ffmpeg) does the demux arithmetic — walking mp4 sample tables correctly
//! for every container variant is a library-sized job, and re-implementing
//! it inside a spike would add exactly the kind of half-tested parser
//! Codebreaker Law 1 warns about. Rust then does the part the decoder needs:
//! reading those exact byte ranges and converting them to Annex-B (see
//! `annexb.rs`). The per-file index costs two ffprobe runs at load time
//! (tens of ms, measured by the seek spike itself), once per opened file.
//!
//! Why the whole packet map: a warm decoder holds `has_b_frames` frames back
//! for reordering (measured across x264 bf=0..3: the hold equals
//! `has_b_frames` exactly), so a keyframe jump must feed the target keyframe
//! plus the L packets that follow it to push the target frame out. Keyframe
//! positions alone are not enough.
//!
//! Discovery chain for ffprobe mirrors ffmpeg's (documented in ONE place):
//! 1. `OMNIFLUX_FFPROBE` env var (explicit override)
//! 2. `ffprobe` sitting next to the located ffmpeg binary (our tools build)
//! 3. plain `ffprobe` on PATH

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::annexb::{parse_avc_config, AvcConfig};

/// One coded video packet (one mp4 sample) in decode order.
pub struct Packet {
    /// Presentation timestamp of the frame this packet produces, seconds.
    pub pts_time: f64,
    /// Absolute byte offset of the encoded sample in the file.
    pub pos: u64,
    /// Encoded sample size in bytes.
    pub size: u64,
    /// True when the container flags this packet as a keyframe (`flags` has K).
    pub keyframe: bool,
}

/// Everything the warm-feeding spike needs to jump around one file.
pub struct MediaIndex {
    pub width: u32,
    pub height: u32,
    /// ffprobe's `has_b_frames`: how many decoded frames the decoder holds
    /// back for reordering. This is the protocol's `L`: a jump feeds L+1
    /// packets and reads L+1 frames, taking the last as the target.
    pub has_b_frames: u32,
    /// Every video packet, in the order ffprobe lists them (decode order).
    pub packets: Vec<Packet>,
    pub avc: AvcConfig,
}

impl MediaIndex {
    /// Indices into `packets` of the keyframes, in decode order.
    pub fn keyframe_indices(&self) -> Vec<usize> {
        self.packets
            .iter()
            .enumerate()
            .filter(|(_, packet)| packet.keyframe)
            .map(|(index, _)| index)
            .collect()
    }
}

/// The stream facts one `ffprobe -show_streams -show_data` run provides.
pub struct StreamMeta {
    pub width: u32,
    pub height: u32,
    pub has_b_frames: u32,
    pub extradata: Vec<u8>,
}

pub fn locate_ffprobe(ffmpeg: &Path) -> Result<PathBuf, String> {
    if let Ok(env_path) = std::env::var("OMNIFLUX_FFPROBE") {
        let p = PathBuf::from(env_path);
        if p.is_file() {
            return Ok(p);
        }
        return Err(format!(
            "OMNIFLUX_FFPROBE is set but the file does not exist: {}",
            p.display()
        ));
    }
    if let Some(dir) = ffmpeg.parent() {
        let name = if cfg!(windows) {
            "ffprobe.exe"
        } else {
            "ffprobe"
        };
        let sibling = dir.join(name);
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    Ok(PathBuf::from(if cfg!(windows) {
        "ffprobe.exe"
    } else {
        "ffprobe"
    }))
}

fn ffprobe_stdout(ffprobe: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new(ffprobe)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "ffprobe not usable at {} — install ffprobe, put it next to ffmpeg, \
                     or set OMNIFLUX_FFPROBE=<path>",
                    ffprobe.display()
                )
            } else {
                format!("could not run ffprobe at {}: {e}", ffprobe.display())
            }
        })?;
    if !output.status.success() {
        return Err(format!(
            "ffprobe failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Builds the jump index for `file` using two ffprobe calls:
/// one for the per-packet map, one for stream size, reorder depth + `avcC`.
pub fn build_index(ffprobe: &Path, file: &Path) -> Result<MediaIndex, String> {
    let file_arg = file.to_string_lossy().into_owned();
    let packets = ffprobe_stdout(
        ffprobe,
        &[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time,size,pos,flags",
            "-of",
            "csv=p=0",
            &file_arg,
        ],
    )?;
    let packets = parse_packet_csv(&packets)?;

    let streams = ffprobe_stdout(
        ffprobe,
        &[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_streams",
            "-show_data",
            "-of",
            "default=noprint_wrappers=1",
            &file_arg,
        ],
    )?;
    let stream = parse_stream_meta(&streams)?;
    let avc = parse_avc_config(&stream.extradata)?;

    if packets.is_empty() {
        return Err(format!(
            "no video packets found in {} — is the file a supported video?",
            file.display()
        ));
    }
    if !packets.iter().any(|p| p.keyframe) {
        return Err(format!(
            "no video keyframes found in {} — the warm jump protocol needs them",
            file.display()
        ));
    }
    if stream.width == 0 || stream.height == 0 {
        return Err(format!(
            "ffprobe reported an unusable frame size {}x{} for {}",
            stream.width,
            stream.height,
            file.display()
        ));
    }
    Ok(MediaIndex {
        width: stream.width,
        height: stream.height,
        has_b_frames: stream.has_b_frames,
        packets,
        avc,
    })
}

/// Parses ffprobe `packet=pts_time,size,pos,flags` CSV lines (csv=p=0).
///
/// Field order is ffprobe's struct order, NOT the order typed in the
/// request — verified against this build: `pts_time,size,pos,flags` with
/// e.g. `0.000000,26318,48,K__` plus `0.033333,12859,26366,___` where
/// 48 + 26318 = 26366 bytes, confirming `size` precedes `pos`.
pub fn parse_packet_csv(text: &str) -> Result<Vec<Packet>, String> {
    let mut packets = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        if fields.len() < 4 {
            return Err(format!(
                "ffprobe packet line {} has {} fields, expected at least 4: {line:?}",
                line_no + 1,
                fields.len()
            ));
        }
        let pts_time = fields[0].parse::<f64>().map_err(|_| {
            format!(
                "bad pts_time {:?} on packet line {}",
                fields[0],
                line_no + 1
            )
        })?;
        let size = fields[1]
            .parse::<u64>()
            .map_err(|_| format!("bad size {:?} on packet line {}", fields[1], line_no + 1))?;
        let pos = fields[2]
            .parse::<u64>()
            .map_err(|_| format!("bad pos {:?} on packet line {}", fields[2], line_no + 1))?;
        packets.push(Packet {
            pts_time,
            pos,
            size,
            keyframe: fields[3].contains('K'),
        });
    }
    Ok(packets)
}

/// Extracts `width`, `height`, `has_b_frames`, and the hex-dumped
/// `extradata` blob from the `-show_streams -show_data` text output.
///
/// The extradata printout is three-space-indented hex rows:
/// `00000000: 0164 0028 ffe1 ...  .d.(....gd.(..@x`, with the ASCII column
/// separated by two spaces, and an `extradata_size=NN` line before or after.
/// We take only the hex region of each row (text before the double space) —
/// the ASCII column can itself contain hex-looking letters, so it must never
/// be read as data.
pub fn parse_stream_meta(text: &str) -> Result<StreamMeta, String> {
    let mut width: Option<u32> = None;
    let mut height: Option<u32> = None;
    let mut has_b_frames: Option<u32> = None;
    let mut declared_size: Option<usize> = None;
    let mut dump: Vec<u8> = Vec::new();
    let mut in_dump = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("width=") {
            if width.is_none() {
                width = rest.trim().parse::<u32>().ok();
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("height=") {
            if height.is_none() {
                height = rest.trim().parse::<u32>().ok();
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("has_b_frames=") {
            has_b_frames = rest.trim().parse::<u32>().ok();
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("extradata_size=") {
            declared_size = rest.trim().parse::<usize>().ok();
            continue;
        }
        if trimmed == "extradata=" {
            in_dump = true;
            continue;
        }
        if in_dump {
            match hex_dump_row(trimmed) {
                Some(hex_part) => {
                    let mut cleaned = String::with_capacity(hex_part.len());
                    for token in hex_part.split_whitespace() {
                        if token.len() % 2 != 0 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
                            return Err(format!("malformed hex token {token:?} in extradata dump"));
                        }
                        cleaned.push_str(token);
                    }
                    for pair in cleaned.as_bytes().chunks(2) {
                        let text = std::str::from_utf8(pair)
                            .map_err(|_| "non-ASCII byte in cleaned hex".to_string())?;
                        dump.push(
                            u8::from_str_radix(text, 16)
                                .map_err(|_| format!("bad hex pair {text:?}"))?,
                        );
                    }
                }
                None => in_dump = false,
            }
        }
    }

    if let Some(declared) = declared_size {
        if dump.len() != declared {
            return Err(format!(
                "extradata dump is {} bytes but ffprobe declared {declared}",
                dump.len()
            ));
        }
    }
    if dump.is_empty() {
        return Err("no extradata found in ffprobe stream output".to_string());
    }
    let width = width.ok_or_else(|| "no width= line in ffprobe stream output".to_string())?;
    let height = height.ok_or_else(|| "no height= line in ffprobe stream output".to_string())?;
    // Required, not defaulted: if this line vanished we would feed too few
    // packets and hang waiting for frames the decoder is still holding.
    let has_b_frames =
        has_b_frames.ok_or_else(|| "no has_b_frames= line in ffprobe stream output".to_string())?;
    Ok(StreamMeta {
        width,
        height,
        has_b_frames,
        extradata: dump,
    })
}

/// One hexdump row body, up to the ASCII column (which starts after a double
/// space). Returns None when the line is not a hexdump row at all.
fn hex_dump_row(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    if bytes.len() >= 10 && bytes[..8].iter().all(|b| b.is_ascii_hexdigit()) && bytes[8] == b':' {
        let body = &line[9..];
        return Some(body.split("  ").next().unwrap_or(""));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_packet_csv_from_the_real_probe_output() {
        // Exact lines observed from ffprobe N-128718 against our test clip.
        let csv = "0.000000,26318,48,K__\n0.033333,12859,26366,___\n0.500000,9001,999999,K__\n";
        let packets = parse_packet_csv(csv).expect("real-shaped csv must parse");
        assert_eq!(
            packets.len(),
            3,
            "every packet is indexed, not just keyframes"
        );
        assert!(packets[0].keyframe);
        assert_eq!(packets[0].pts_time, 0.0);
        assert_eq!(packets[0].size, 26318);
        assert_eq!(packets[0].pos, 48);
        assert!(!packets[1].keyframe);
        assert_eq!(packets[2].pts_time, 0.5);
        assert!(packets[2].keyframe);
    }

    #[test]
    fn rejects_malformed_csv() {
        assert!(parse_packet_csv("0.0,K__\n").is_err());
        assert!(parse_packet_csv("x,1,2,K__\n").is_err());
    }

    #[test]
    fn keyframe_indices_point_at_the_flagged_packets() {
        let packets =
            parse_packet_csv("0,10,0,K__\n0.033,5,10,___\n0.5,9,15,K__\n").expect("csv must parse");
        let index = MediaIndex {
            width: 2,
            height: 2,
            has_b_frames: 0,
            packets,
            avc: AvcConfig {
                nal_length_size: 4,
                sps: Vec::new(),
                pps: Vec::new(),
            },
        };
        assert_eq!(index.keyframe_indices(), vec![0, 2]);
    }

    /// The real `extradata=` hexdump ffprobe printed for our test clip.
    const REAL_STREAMS_TEXT: &str = "\
width=1920
height=1080
has_b_frames=2
extradata=
00000000: 0164 0028 ffe1 001b 6764 0028 acd9 4078  .d.(....gd.(..@x
00000010: 0227 e5c0 4400 0003 0004 0000 0300 f03c  .'..D..........<
00000020: 60c6 5801 0006 68eb e24b 22c0 fdf8 f800  `.X...h..K\".....
extradata_size=48
";

    #[test]
    fn parses_real_stream_meta_and_extradata() {
        let meta = parse_stream_meta(REAL_STREAMS_TEXT).expect("real text must parse");
        assert_eq!((meta.width, meta.height), (1920, 1080));
        assert_eq!(meta.has_b_frames, 2);
        assert_eq!(meta.extradata.len(), 48);
        assert_eq!(meta.extradata[0], 0x01);
        let cfg =
            parse_avc_config(&meta.extradata).expect("the dumped record must be a valid avcC");
        assert_eq!(cfg.nal_length_size, 4);
    }

    #[test]
    fn rejects_size_mismatch_and_missing_fields() {
        let bad_size = REAL_STREAMS_TEXT.replace("extradata_size=48", "extradata_size=47");
        assert!(parse_stream_meta(&bad_size).is_err());

        let no_b_frames = REAL_STREAMS_TEXT.replace("has_b_frames=2\n", "");
        assert!(parse_stream_meta(&no_b_frames).is_err());

        assert!(
            parse_stream_meta("extradata=\n00000000: 00\n").is_err(),
            "no width"
        );
    }

    #[test]
    fn ascii_column_cannot_leak_into_the_binary() {
        // The ASCII column here is pure hex characters — a naive parser would
        // swallow it. The double-space split must stop before the ASCII.
        let text = "width=2\nheight=2\nhas_b_frames=0\nextradata=
00000000: 01ff abcd  deadbeef
extradata_size=4
";
        let meta = parse_stream_meta(text).expect("must parse cleanly");
        assert_eq!(meta.extradata, [0x01, 0xFF, 0xAB, 0xCD]);
    }
}
