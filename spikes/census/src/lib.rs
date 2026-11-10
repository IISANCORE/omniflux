//! The RAM census: what does OmniFlux actually use?
//!
//! One table, one run, every part of the engine's memory:
//! - our own process, empty and holding a real project with a real edit
//!   history;
//! - the decoder process, at full resolution and at proxy resolution;
//! - what one queued frame of pixels costs (the only part of the pipeline
//!   that can grow without limit);
//! - the project file on disk;
//! - and a projected total for the editor, with the renderer marked as the
//!   part that does not exist yet.
//!
//! This is the BASELINE for the 4 GB optimisation: every number here can be
//! re-measured later and compared against this table. It runs on the dev
//! machine, so every row says which machine produced it (Law 6).

pub mod rss;

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use omniflux_core::edit::{Command as EditCommand, History, TrimClip};
use omniflux_core::time::{Ticks, Timebase};
use omniflux_core::timeline::{Clip, ClipId, MediaId, Timeline, TrackKind};
use omniflux_project::Project;

/// The frame size the full-resolution preview would use.
const FULL_RESOLUTION: (u32, u32) = (1920, 1080);
/// The frame size the proxy preview would use.
const PROXY_RESOLUTION: (u32, u32) = (640, 360);
/// How many clips the sample project holds.
const PROJECT_CLIPS: usize = 500;
/// How many edits the sample project's history holds.
const PROJECT_EDITS: usize = 5_000;
/// How long the decoder is watched while it works.
const DECODER_WATCH: Duration = Duration::from_secs(3);
/// How long the decoder is given to start working before the first sample.
///
/// Without this pause the first sample measures a process that has not
/// allocated anything yet — 14.8 MB instead of the 226 MB a working 1080p
/// decoder actually uses. A number that is too low because it was taken too
/// early is worse than no number.
const DECODER_WARMUP: Duration = Duration::from_millis(1_200);
/// How often the decoder's memory is sampled while it works.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(200);
/// The fewest samples that make a decoder number believable.
const MINIMUM_SAMPLES: usize = 3;

/// How long to wait for the decoder's output reader to finish once the
/// decoder is gone.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Stops the decoder AND anything it started.
///
/// On Windows the binary we spawn may be a WRAPPER — Chocolatey's shim, for
/// one — and killing the wrapper leaves the real ffmpeg running behind it,
/// still holding the output pipe open. That is the same trap that hung the
/// warm decoder's shutdown. `taskkill /T` kills the tree; everywhere else
/// the child we started is the process we mean.
fn kill_process_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

/// What the census measured.
#[derive(Debug, Clone)]
pub struct Census {
    /// The machine the numbers came from.
    pub machine: String,
    /// Our process before it holds anything.
    pub own_empty_bytes: Option<u64>,
    /// Our process holding the sample project and its edit history.
    pub own_with_project_bytes: Option<u64>,
    /// What the project and the history cost on their own.
    pub model_and_history_bytes: Option<u64>,
    /// The decoder process at full resolution.
    pub decoder_full_bytes: Option<u64>,
    /// The decoder process at proxy resolution.
    pub decoder_proxy_bytes: Option<u64>,
    /// One frame of pixels at full resolution.
    pub full_frame_bytes: usize,
    /// One frame of pixels at proxy resolution.
    pub proxy_frame_bytes: usize,
    /// The project file on disk.
    pub project_file_bytes: u64,
    /// What the editor would use: our process plus a proxy decoder. The
    /// renderer is not built yet, so this is a floor, not the total.
    pub projected_editor_bytes: Option<u64>,
}

/// Runs the census and returns what it measured.
pub fn run_census(ffmpeg: &Path) -> Result<Census, String> {
    let machine = rss::machine_description();
    let own_empty_bytes = rss::own_resident_bytes();

    // ---- our own process, holding a real project -------------------------
    let (project, file_bytes) = build_sample_project()?;
    let own_with_project_bytes = rss::own_resident_bytes();
    let model_and_history_bytes = match (own_empty_bytes, own_with_project_bytes) {
        (Some(before), Some(after)) if after > before => Some(after - before),
        _ => None,
    };

    // ---- the decoder, at both preview resolutions -------------------------
    let decoder_full_bytes = measure_decoder(ffmpeg, FULL_RESOLUTION)?;
    let decoder_proxy_bytes = measure_decoder(ffmpeg, PROXY_RESOLUTION)?;

    let full_frame_bytes = frame_bytes(FULL_RESOLUTION);
    let proxy_frame_bytes = frame_bytes(PROXY_RESOLUTION);

    // The editor's floor: our process with a project, plus a decoder working
    // at proxy resolution. The renderer does not exist yet.
    let projected_editor_bytes = match (own_with_project_bytes, decoder_proxy_bytes) {
        (Some(ours), Some(decoder)) => Some(ours + decoder),
        _ => None,
    };

    // The project is kept alive until here on purpose: the point of the
    // measurement is what the process costs WHILE it holds the work.
    drop(project);

    Ok(Census {
        machine,
        own_empty_bytes,
        own_with_project_bytes,
        model_and_history_bytes,
        decoder_full_bytes,
        decoder_proxy_bytes,
        full_frame_bytes,
        proxy_frame_bytes,
        project_file_bytes: file_bytes,
        projected_editor_bytes,
    })
}

/// Builds a sample project, gives it an edit history, and writes it to disk.
fn build_sample_project() -> Result<(Project, u64), String> {
    let timebase = Timebase::default_timebase();
    let mut timeline = Timeline::new(timebase);
    for id in 0..8u32 {
        timeline
            .add_media(MediaId(id), timebase.seconds(60.0))
            .map_err(|error| format!("adding media failed: {error}"))?;
    }
    let mut tracks = Vec::new();
    for index in 0..4 {
        let kind = if index < 2 {
            TrackKind::Video
        } else {
            TrackKind::Audio
        };
        tracks.push(timeline.add_track(kind, format!("Track {index}")));
    }
    for index in 0..PROJECT_CLIPS {
        let clip = Clip {
            id: ClipId(index as u32),
            media: MediaId((index % 8) as u32),
            start: timebase.seconds((index / 4 * 30) as f64),
            duration: timebase.seconds(29.0),
            source_in: Ticks::zero(),
        };
        timeline
            .insert_clip(tracks[index % 4], clip)
            .map_err(|error| format!("building the project failed: {error}"))?;
    }

    // An edit history, so the census includes what "the user has been
    // working" costs: trims only, because they change numbers without
    // changing the shape of the project.
    let mut history = History::new();
    for index in 0..PROJECT_EDITS {
        let clip = &timeline.tracks()[index % 4].clips()[index % 100];
        let command: Box<dyn EditCommand> = Box::new(TrimClip {
            clip: clip.id,
            new_source_in: clip.source_in,
            new_duration: Ticks((clip.duration.get() / 2).max(1)),
        });
        let _ = history.execute(&mut timeline, command);
    }

    let mut project = Project::new(timeline);
    project.set_location(MediaId(0), PathBuf::from("footage/a.mp4"));
    let file = std::env::temp_dir().join("omniflux-census-project.omni");
    project
        .save_to(&file)
        .map_err(|error| format!("saving the project failed: {error}"))?;
    let file_bytes = std::fs::metadata(&file).map(|meta| meta.len()).unwrap_or(0);
    let _ = std::fs::remove_file(&file);

    Ok((project, file_bytes))
}

/// Watches a decoder working at the given resolution and reports its peak
/// resident memory.
///
/// The decoder is run the way the app runs it: rgb24 frames over a pipe,
/// which somebody drains. (Writing the frames to the platform's null device
/// instead would be simpler, but it is not the shape the app uses, and on
/// one platform it silently measured a process that had already died.)
fn measure_decoder(ffmpeg: &Path, resolution: (u32, u32)) -> Result<Option<u64>, String> {
    let clip = std::env::temp_dir().join(format!(
        "omniflux-census-clip-{}x{}.mp4",
        resolution.0, resolution.1
    ));
    make_clip(ffmpeg, &clip, resolution)?;

    let stderr_path = std::env::temp_dir().join(format!(
        "omniflux-census-decoder-{}x{}.log",
        resolution.0, resolution.1
    ));
    let stderr_file = File::create(&stderr_path)
        .map_err(|error| format!("could not create the decoder log: {error}"))?;

    let mut child = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-stream_loop",
            "-1",
            "-i",
            &clip.to_string_lossy(),
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .map_err(|error| format!("could not start the decoder: {error}"))?;

    // Drain the frames: the decoder writes them far faster than we sample
    // its memory, and a decoder blocked on a full pipe is not working.
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "the decoder's stdout pipe vanished".to_string())?;
    let (drained_tx, drained_rx) = mpsc::channel::<()>();
    let drain = std::thread::spawn(move || {
        let mut stdout = stdout;
        let mut buffer = vec![0u8; 64 * 1024];
        while let Ok(read) = stdout.read(&mut buffer) {
            if read == 0 {
                break;
            }
        }
        let _ = drained_tx.send(());
    });

    let pid = child.id();
    // The binary we spawned may be a wrapper (Chocolatey's shim), so the
    // memory that matters is the whole tree: the wrapper plus the real
    // decoder behind it. Ask once, after the decoder has started.
    std::thread::sleep(DECODER_WARMUP);
    let mut pids = vec![pid];
    pids.extend(rss::child_pids(pid));

    let mut peak = 0u64;
    let mut samples = 0usize;
    let watch_until = Instant::now() + DECODER_WATCH;
    while Instant::now() < watch_until {
        let bytes: u64 = pids
            .iter()
            .filter_map(|member| rss::resident_bytes_of(*member))
            .sum();
        if std::env::var_os("CENSUS_DEBUG").is_some() {
            eprintln!("  sample pids {pids:?}: {bytes} bytes");
        }
        if bytes > 0 {
            peak = peak.max(bytes);
            samples += 1;
        }
        std::thread::sleep(SAMPLE_INTERVAL);
    }

    let exited = child.try_wait().ok().flatten();
    kill_process_tree(&mut child);
    let _ = child.wait();
    // The drain thread ends when the pipe closes — which needs the decoder
    // GONE, not just our handle on it. Wait for it, but never forever: a
    // thread that outlives the measurement is better than a hung run, as
    // long as we say so.
    let drained = drained_rx.recv_timeout(DRAIN_TIMEOUT).is_ok();
    if drained {
        let _ = drain.join();
    } else {
        eprintln!(
            "warning: the decoder's output reader did not finish within {} s; detaching it",
            DRAIN_TIMEOUT.as_secs()
        );
    }
    let _ = std::fs::remove_file(&clip);

    // A decoder that died leaves a zombie behind, and a zombie's memory
    // reads as zero. Zero is not a measurement — it is the absence of one,
    // and reporting it as a number would be a lie.
    if peak == 0 {
        let log = std::fs::read_to_string(&stderr_path).unwrap_or_default();
        let _ = std::fs::remove_file(&stderr_path);
        return Err(format!(
            "the {}x{} decoder did not stay alive long enough to measure (exit: {:?}); \
             its own words: {}",
            resolution.0,
            resolution.1,
            exited.map(|status| status.to_string()),
            log.trim()
        ));
    }
    let _ = std::fs::remove_file(&stderr_path);

    // Too few samples means we did not really watch it work; saying "not
    // measurable" is honest, a low number would not be.
    Ok(if samples >= MINIMUM_SAMPLES {
        Some(peak)
    } else {
        None
    })
}

/// Makes a short clip at the given resolution, so the decoder has real work.
fn make_clip(ffmpeg: &Path, path: &Path, resolution: (u32, u32)) -> Result<(), String> {
    let status = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc2=size={}x{}:rate=30", resolution.0, resolution.1),
            "-frames:v",
            "60",
            "-c:v",
            "libx264",
            "-crf",
            "28",
            "-g",
            "15",
            "-keyint_min",
            "15",
            "-sc_threshold",
            "0",
            "-y",
            &path.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("could not make a test clip: {error}"))?;
    if !status.success() {
        return Err(format!("could not make a test clip ({status})"));
    }
    Ok(())
}

/// How many bytes one frame of pixels takes at a resolution (rgb24).
fn frame_bytes(resolution: (u32, u32)) -> usize {
    resolution.0 as usize * resolution.1 as usize * 3
}

/// Prints the census table and what it says.
pub fn print_census(census: &Census) {
    println!("census: what OmniFlux actually uses");
    println!("measured on: {}", census.machine);
    println!();
    println!(
        "  {:<46}{}",
        "our process, empty",
        mb(census.own_empty_bytes)
    );
    println!(
        "  {:<46}{}",
        "our process, 500-clip project + 5 000 edits",
        mb(census.own_with_project_bytes)
    );
    println!(
        "  {:<46}{}",
        "of which the project and the history",
        mb(census.model_and_history_bytes)
    );
    println!(
        "  {:<46}{}",
        "decoder working, 1920x1080",
        mb(census.decoder_full_bytes)
    );
    println!(
        "  {:<46}{}",
        "decoder working, 640x360 (proxy)",
        mb(census.decoder_proxy_bytes)
    );
    println!(
        "  {:<46}{}",
        "project file on disk (500 clips)",
        mb(Some(census.project_file_bytes))
    );
    println!();
    println!(
        "  {:<46}{}",
        "one queued frame, 1920x1080",
        mb(Some(census.full_frame_bytes as u64))
    );
    println!(
        "  {:<46}{}",
        "one queued frame, 640x360",
        mb(Some(census.proxy_frame_bytes as u64))
    );
    println!(
        "  {:<46}{}",
        "100 queued frames, 1920x1080",
        mb(Some(census.full_frame_bytes as u64 * 100))
    );
    println!(
        "  {:<46}{}",
        "100 queued frames, 640x360",
        mb(Some(census.proxy_frame_bytes as u64 * 100))
    );
    println!();
    match census.projected_editor_bytes {
        Some(bytes) => println!(
            "  projected editor total (ours + a proxy decoder, no renderer yet): {}",
            mb(Some(bytes))
        ),
        None => println!("  projected editor total: not measurable on this platform"),
    }
    println!();
    println!("  read: the arrangement is cheap and the DECODE is the budget.");
    println!("        the one part that can grow without limit is the frame queue —");
    println!("        bounding it, and previewing at proxy resolution, is the work");
    println!("        that takes this down to a 4 GB machine.");
}

/// A number in megabytes, or a plain "not measurable".
fn mb(bytes: Option<u64>) -> String {
    match bytes {
        Some(bytes) if bytes >= 1024 * 1024 => {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        }
        Some(bytes) => format!("{bytes} B"),
        None => "not measurable here".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_can_describe_itself() {
        let description = rss::machine_description();
        assert!(!description.is_empty(), "the census must say where it ran");
    }

    #[test]
    fn a_frame_of_pixels_is_the_size_it_says() {
        assert_eq!(frame_bytes((1920, 1080)), 6_220_800);
        assert_eq!(frame_bytes((640, 360)), 691_200);
    }
}
