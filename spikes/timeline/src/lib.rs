//! The timeline spike: the edit/undo machinery under load.
//!
//! What it measures, and why each number decides something:
//! - **cost of one edit** — the user feels this on every action, so it must
//!   stay far below a frame (16 ms) to be invisible;
//! - **cost of one undoable edit in memory** — this is what "unlimited undo"
//!   actually means. If one entry costs a kilobyte, a hundred thousand undo
//!   steps cost a hundred megabytes, and the design must change;
//! - **whether undoing everything puts the project back** — the promise the
//!   whole command design rests on, checked at scale rather than in a unit
//!   test with three clips;
//! - **how heavy a project with hundreds of clips is** — the first real
//!   number for the RAM census the 4 GB promise rests on.
//!
//! Every number printed here is measured by this run on this machine. The
//! harness is deterministic: the same seed produces the same edits, so a
//! number from last week can be compared with a number from today.

use std::time::Instant;

use omniflux_core::edit::{Command, History, InsertClip, MoveClip, SplitClip, TrimClip};
use omniflux_core::time::{Ticks, Timebase};
use omniflux_core::timeline::{Clip, ClipId, MediaId, Timeline, TrackId, TrackKind};
use omniflux_project::Project;

/// How many source assets the project has.
const MEDIA_COUNT: usize = 8;
/// How many tracks the project has.
const TRACK_COUNT: usize = 4;
/// How long one source asset is, in seconds.
const MEDIA_DURATION_SECONDS: i64 = 60;
/// How long a built clip is, in seconds.
const BUILT_CLIP_SECONDS: i64 = 30;
/// How long an inserted clip is, in seconds.
const INSERTED_CLIP_SECONDS: i64 = 10;
/// The gap left between clips when one is moved to the end of a track.
const MOVE_GAP_SECONDS: i64 = 1;
/// How many edits the "price of one undoable edit" measurement makes.
const UNDO_COST_EDITS: usize = 1_000;

/// What the spike should build and measure.
#[derive(Debug, Clone, Copy)]
pub struct SpikeConfig {
    /// How many clips the project starts with.
    pub clips: usize,
    /// How many edits to perform.
    pub edits: usize,
    /// The seed for the edit sequence — the same seed means the same edits.
    pub seed: u64,
}

impl Default for SpikeConfig {
    fn default() -> Self {
        SpikeConfig {
            clips: 500,
            edits: 5_000,
            seed: 0x5EED_1234_ABCD_0001,
        }
    }
}

/// What the spike measured.
#[derive(Debug, Clone)]
pub struct Report {
    pub clips: usize,
    pub tracks: usize,
    pub edits: usize,
    pub edits_refused: usize,
    pub build_ms: f64,
    pub edit_p50_ms: f64,
    pub edit_p95_ms: f64,
    pub edit_max_ms: f64,
    pub edits_per_second: f64,
    pub undo_p50_ms: f64,
    pub undo_total_ms: f64,
    pub redo_p50_ms: f64,
    pub redo_total_ms: f64,
    /// Bytes of resident memory per clip, measured while building the project.
    pub bytes_per_clip: Option<u64>,
    /// Bytes of resident memory per undoable edit — the price of "unlimited
    /// undo".
    pub bytes_per_undo_entry: Option<u64>,
    /// Resident memory the whole project costs.
    pub project_rss_bytes: Option<u64>,
    /// Whether undoing every edit returned the project exactly.
    pub undo_restored_project: bool,
    /// Whether redoing every edit returned the project exactly.
    pub redo_restored_project: bool,
    /// How long writing the project file took, in milliseconds.
    pub save_ms: f64,
    /// How long reading it back took, in milliseconds.
    pub load_ms: f64,
    /// How big the project file is, in bytes.
    pub file_bytes: u64,
    /// Whether the project came back from the file exactly as it went in.
    pub file_round_trip_exact: bool,
}

/// Builds a project, edits it, undoes everything, and measures the lot.
pub fn run_spike(config: &SpikeConfig) -> Result<Report, String> {
    let timebase = Timebase::default_timebase();
    let mut timeline = Timeline::new(timebase);
    let mut rng = Rng::new(config.seed);

    // ---- the project ------------------------------------------------------
    let rss_before_build = resident_bytes();
    let build_started = Instant::now();
    for id in 0..MEDIA_COUNT {
        timeline
            .add_media(
                MediaId(id as u32),
                timebase.seconds(MEDIA_DURATION_SECONDS as f64),
            )
            .map_err(|error| format!("adding media failed: {error}"))?;
    }
    let track_count = TRACK_COUNT.min(config.clips.max(1));
    let mut tracks = Vec::with_capacity(track_count);
    for index in 0..TRACK_COUNT {
        let kind = if index < TRACK_COUNT / 2 {
            TrackKind::Video
        } else {
            TrackKind::Audio
        };
        tracks.push(timeline.add_track(kind, format!("Track {index}")));
    }

    // Clips are laid out one after another on each track, so none of them
    // overlap: the project starts valid and stays valid.
    let clip_duration = timebase.seconds(BUILT_CLIP_SECONDS as f64);
    for index in 0..config.clips {
        let track = tracks[index % TRACK_COUNT];
        let slot = (index / TRACK_COUNT) as i64;
        let clip = Clip {
            id: ClipId(index as u32),
            media: MediaId((index % MEDIA_COUNT) as u32),
            start: timebase.seconds((slot * BUILT_CLIP_SECONDS) as f64),
            duration: clip_duration,
            source_in: Ticks::zero(),
        };
        timeline
            .insert_clip(track, clip)
            .map_err(|error| format!("building the project failed: {error}"))?;
    }
    let build_elapsed = build_started.elapsed();
    let rss_after_build = resident_bytes();

    // The state the project must return to.
    let initial_state = timeline.clone();

    // ---- the edits --------------------------------------------------------
    let mut history = History::new();
    let mut next_clip_id = config.clips as u32;
    let mut edit_millis: Vec<f64> = Vec::with_capacity(config.edits);
    let mut refused = 0usize;

    for _ in 0..config.edits {
        let command = pick_command(&mut rng, &timeline, &mut next_clip_id, timebase);
        let started = Instant::now();
        let outcome = history.execute(&mut timeline, command);
        let elapsed = started.elapsed();
        match outcome {
            Ok(()) => edit_millis.push(elapsed.as_secs_f64() * 1000.0),
            Err(_) => refused += 1,
        }
    }

    timeline
        .validate()
        .map_err(|error| format!("the edited project is not valid: {error}"))?;
    let edited_state = timeline.clone();

    // ---- the price of one undoable edit -----------------------------------
    // Measured with trims only: they change two numbers and add no clips, so
    // the resident memory that grows is the undo stack and nothing else.
    let mut cost_timeline = timeline.clone();
    let mut cost_history = History::new();
    let mut cost_rng = Rng::new(config.seed ^ 0xFFFF);
    let rss_before_undo_cost = resident_bytes();
    for _ in 0..UNDO_COST_EDITS {
        let command = pick_trim(&mut cost_rng, &cost_timeline);
        let _ = cost_history.execute(&mut cost_timeline, command);
    }
    let rss_after_undo_cost = resident_bytes();
    let bytes_per_undo_entry = match (rss_before_undo_cost, rss_after_undo_cost) {
        (Some(before), Some(after)) if after > before => {
            Some((after - before) / UNDO_COST_EDITS as u64)
        }
        _ => None,
    };

    // ---- undo everything, redo everything ---------------------------------
    let mut undo_millis: Vec<f64> = Vec::with_capacity(config.edits);
    let undo_started = Instant::now();
    loop {
        let started = Instant::now();
        let did_undo = history
            .undo(&mut timeline)
            .map_err(|error| format!("undo failed: {error}"))?;
        let elapsed = started.elapsed();
        if !did_undo {
            break;
        }
        undo_millis.push(elapsed.as_secs_f64() * 1000.0);
    }
    let undo_total = undo_started.elapsed();
    let undo_restored_project = timeline == initial_state;

    let mut redo_millis: Vec<f64> = Vec::with_capacity(config.edits);
    let redo_started = Instant::now();
    loop {
        let started = Instant::now();
        let did_redo = history
            .redo(&mut timeline)
            .map_err(|error| format!("redo failed: {error}"))?;
        let elapsed = started.elapsed();
        if !did_redo {
            break;
        }
        redo_millis.push(elapsed.as_secs_f64() * 1000.0);
    }
    let redo_total = redo_started.elapsed();
    let redo_restored_project = timeline == edited_state;

    let edits_total: f64 = edit_millis.iter().sum();

    // ---- the project file -------------------------------------------------
    // The same project, written to disk and read back: how long that takes,
    // how big the file is, and whether the project survives the trip
    // unchanged. This is what a user feels when they press Save and Open.
    let file_path = std::env::temp_dir().join("omniflux-timeline-spike.omni");
    let mut project = Project::new(timeline.clone());
    project.set_location(
        MediaId(0),
        file_path.with_file_name("footage").join("a.mp4"),
    );

    let save_started = Instant::now();
    project
        .save_to(&file_path)
        .map_err(|error| format!("saving the project failed: {error}"))?;
    let save_elapsed = save_started.elapsed();
    let file_bytes = std::fs::metadata(&file_path)
        .map(|meta| meta.len())
        .unwrap_or(0);

    let load_started = Instant::now();
    let loaded = Project::load_from(&file_path)
        .map_err(|error| format!("loading the project failed: {error}"))?;
    let load_elapsed = load_started.elapsed();
    let file_round_trip_exact =
        loaded.timeline() == project.timeline() && loaded.locations() == project.locations();
    let _ = std::fs::remove_file(&file_path);

    Ok(Report {
        clips: config.clips,
        tracks: TRACK_COUNT,
        edits: config.edits,
        edits_refused: refused,
        build_ms: build_elapsed.as_secs_f64() * 1000.0,
        edit_p50_ms: percentile(&edit_millis, 0.50),
        edit_p95_ms: percentile(&edit_millis, 0.95),
        edit_max_ms: edit_millis.iter().copied().fold(0.0_f64, f64::max),
        edits_per_second: if edits_total > 0.0 {
            config.edits as f64 / (edits_total / 1000.0)
        } else {
            0.0
        },
        undo_p50_ms: percentile(&undo_millis, 0.50),
        undo_total_ms: undo_total.as_secs_f64() * 1000.0,
        redo_p50_ms: percentile(&redo_millis, 0.50),
        redo_total_ms: redo_total.as_secs_f64() * 1000.0,
        bytes_per_clip: match (rss_before_build, rss_after_build) {
            (Some(before), Some(after)) if after > before && config.clips > 0 => {
                Some((after - before) / config.clips as u64)
            }
            _ => None,
        },
        bytes_per_undo_entry,
        project_rss_bytes: rss_after_build,
        undo_restored_project,
        redo_restored_project,
        save_ms: save_elapsed.as_secs_f64() * 1000.0,
        load_ms: load_elapsed.as_secs_f64() * 1000.0,
        file_bytes,
        file_round_trip_exact,
    })
}

/// Picks the next edit for the harness to make.
///
/// The kinds are weighted the way an editing session roughly goes: lots of
/// trims, a fair number of moves, some splits, the occasional insert.
fn pick_command(
    rng: &mut Rng,
    timeline: &Timeline,
    next_clip_id: &mut u32,
    timebase: Timebase,
) -> Box<dyn Command> {
    match rng.below(100) {
        0..=39 => pick_trim(rng, timeline),
        40..=64 => pick_move(rng, timeline, timebase),
        65..=84 => pick_split(rng, timeline, next_clip_id),
        _ => pick_insert(rng, timeline, next_clip_id, timebase),
    }
}

/// Shortens a clip. Shrinking is always legal, so this edit never fails.
fn pick_trim(rng: &mut Rng, timeline: &Timeline) -> Box<dyn Command> {
    let clip = pick_random_clip(rng, timeline);
    Box::new(TrimClip {
        clip: clip.id,
        new_source_in: clip.source_in,
        new_duration: Ticks((clip.duration.get() / 2).max(1)),
    })
}

/// Moves a clip to the free space at the end of its own track.
fn pick_move(rng: &mut Rng, timeline: &Timeline, timebase: Timebase) -> Box<dyn Command> {
    let clip = pick_random_clip(rng, timeline);
    let track = timeline
        .track_of(clip.id)
        .expect("a clip from the model is on a track");
    Box::new(MoveClip {
        clip: clip.id,
        to_track: track,
        to_start: track_end(timeline, track) + timebase.seconds(MOVE_GAP_SECONDS as f64),
    })
}

/// Cuts a clip in two.
fn pick_split(rng: &mut Rng, timeline: &Timeline, next_clip_id: &mut u32) -> Box<dyn Command> {
    let clip = pick_random_clip(rng, timeline);
    let right = *next_clip_id;
    *next_clip_id += 1;
    Box::new(SplitClip {
        clip: clip.id,
        at: clip.start + Ticks(clip.duration.get() / 2),
        right: ClipId(right),
    })
}

/// Adds a new clip at the end of a track.
fn pick_insert(
    rng: &mut Rng,
    timeline: &Timeline,
    next_clip_id: &mut u32,
    timebase: Timebase,
) -> Box<dyn Command> {
    let tracks = timeline.tracks();
    let track = tracks[rng.below(tracks.len() as u64) as usize].id;
    let id = *next_clip_id;
    *next_clip_id += 1;
    Box::new(InsertClip {
        track,
        clip: Clip {
            id: ClipId(id),
            media: MediaId(rng.below(MEDIA_COUNT as u64) as u32),
            start: track_end(timeline, track) + timebase.seconds(MOVE_GAP_SECONDS as f64),
            duration: timebase.seconds(INSERTED_CLIP_SECONDS as f64),
            source_in: Ticks::zero(),
        },
    })
}

/// The point where a track's material ends.
fn track_end(timeline: &Timeline, track: TrackId) -> Ticks {
    timeline
        .track(track)
        .map(|track| {
            track
                .clips()
                .iter()
                .map(|clip| clip.end())
                .max()
                .unwrap_or_else(Ticks::zero)
        })
        .unwrap_or_else(Ticks::zero)
}

/// A clip chosen at random from the whole project.
///
/// Only tracks that hold something can be chosen: an empty track has no
/// clip to pick, and the harness must never panic on a project smaller than
/// its own track count.
fn pick_random_clip<'a>(rng: &mut Rng, timeline: &'a Timeline) -> &'a Clip {
    let tracks: Vec<_> = timeline
        .tracks()
        .iter()
        .filter(|track| !track.clips().is_empty())
        .collect();
    let track = tracks[rng.below(tracks.len() as u64) as usize];
    let clips = track.clips();
    &clips[rng.below(clips.len() as u64) as usize]
}

/// A tiny deterministic pseudo-random generator (xorshift64*).
///
/// Why not a crate: the harness must produce the SAME sequence of edits on
/// every machine and every run, so today's numbers can be compared with
/// next week's. That determinism is worth more than a dependency (Law 4).
#[derive(Debug)]
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Rng { state: seed | 1 }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, limit: u64) -> u64 {
        self.next_u64() % limit.max(1)
    }
}

/// The process's resident memory, in bytes, when the platform can tell us.
pub fn resident_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        linux_resident_bytes()
    }
    #[cfg(target_os = "windows")]
    {
        windows_resident_bytes()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// Linux: the resident pages field of /proc/self/statm, times the page size.
#[cfg(target_os = "linux")]
fn linux_resident_bytes() -> Option<u64> {
    const PAGE_SIZE_BYTES: u64 = 4096;
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(resident_pages * PAGE_SIZE_BYTES)
}

/// Windows: ask `tasklist` for this process's memory usage.
///
/// No dependency and no unsafe: the operating system already knows how to
/// report a process's memory, so we ask it instead of calling the Win32 API.
#[cfg(target_os = "windows")]
fn windows_resident_bytes() -> Option<u64> {
    let pid = std::process::id();
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| line.contains(&format!("\"{pid}\"")))?;
    // "name.exe","1234","Console","1","12,345 K"
    let last = line.rsplit("\",\"").next()?.trim_matches('"');
    let kilobytes: u64 = last
        .replace(',', "")
        .trim()
        .trim_end_matches('K')
        .trim()
        .parse()
        .ok()?;
    Some(kilobytes * 1024)
}

/// A percentile of a set of measurements, in milliseconds.
fn percentile(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let index = ((sorted.len() as f64 * q) as usize).min(sorted.len() - 1);
    sorted[index]
}

/// Prints the measured report. Every number here came from the run that
/// produced it — nothing is aspirational (Law 6).
pub fn print_report(report: &Report) {
    println!("spike: timeline edit/undo under load");
    println!(
        "project: clips={} tracks={} media={} timebase={} ticks/s",
        report.clips,
        report.tracks,
        MEDIA_COUNT,
        Timebase::default_timebase().ticks_per_second()
    );
    println!(
        "method: build the project, make {} edits through the command history, \
         measure each one, then undo everything and redo everything.",
        report.edits
    );
    println!(
        "build: {:.1}ms total ({:.1} µs/clip)",
        report.build_ms,
        report.build_ms * 1000.0 / report.clips.max(1) as f64
    );
    println!(
        "edit: p50={:.3}ms p95={:.3}ms max={:.3}ms ({:.0} edits/s, {} refused)",
        report.edit_p50_ms,
        report.edit_p95_ms,
        report.edit_max_ms,
        report.edits_per_second,
        report.edits_refused
    );
    println!(
        "undo: p50={:.3}ms total={:.1}ms | redo: p50={:.3}ms total={:.1}ms",
        report.undo_p50_ms, report.undo_total_ms, report.redo_p50_ms, report.redo_total_ms
    );
    match report.bytes_per_undo_entry {
        Some(bytes) => println!(
            "memory: {bytes} bytes per undoable edit ({UNDO_COST_EDITS} edits) \
             -> {} MB for 100k undo steps",
            bytes * 100_000 / (1024 * 1024)
        ),
        None => println!("memory: per-undo-entry cost not measurable on this platform"),
    }
    match (report.bytes_per_clip, report.project_rss_bytes) {
        (Some(per_clip), Some(total)) => println!(
            "memory: project resident={} MB, ~{} bytes per clip \
             (resident memory moves in pages, so this is approximate)",
            total / (1024 * 1024),
            per_clip
        ),
        _ => println!("memory: project size not measurable on this platform"),
    }
    println!(
        "file: save={:.1}ms load={:.1}ms size={} KB round_trip_exact={}",
        report.save_ms,
        report.load_ms,
        report.file_bytes / 1024,
        report.file_round_trip_exact
    );
    println!(
        "summary: edits={} edit_p50={:.3}ms undo_p50={:.3}ms redo_p50={:.3}ms \
         bytes_per_undo_entry={} bytes_per_clip={} save={:.1}ms load={:.1}ms \
         file_kb={} undo_restored={} redo_restored={} file_exact={}",
        report.edits,
        report.edit_p50_ms,
        report.undo_p50_ms,
        report.redo_p50_ms,
        report
            .bytes_per_undo_entry
            .map(|bytes| bytes.to_string())
            .unwrap_or_else(|| "n/a".to_string()),
        report
            .bytes_per_clip
            .map(|bytes| bytes.to_string())
            .unwrap_or_else(|| "n/a".to_string()),
        report.save_ms,
        report.load_ms,
        report.file_bytes / 1024,
        report.undo_restored_project,
        report.redo_restored_project,
        report.file_round_trip_exact,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_makes_the_same_edits() {
        let config = SpikeConfig {
            clips: 20,
            edits: 50,
            seed: 99,
        };
        let first = run_spike(&config).expect("the spike must run");
        let second = run_spike(&config).expect("the spike must run");
        assert_eq!(first.edits, second.edits);
        assert_eq!(first.edits_refused, second.edits_refused);
    }

    #[test]
    fn undoing_every_edit_returns_the_project_exactly() {
        let report = run_spike(&SpikeConfig {
            clips: 40,
            edits: 200,
            seed: 7,
        })
        .expect("the spike must run");

        assert!(
            report.undo_restored_project,
            "undo must restore the project"
        );
        assert!(
            report.redo_restored_project,
            "redo must restore the project"
        );
        assert_eq!(
            report.edits_refused, 0,
            "the harness only makes legal edits"
        );
    }

    #[test]
    fn a_project_smaller_than_the_track_count_still_works() {
        // Fewer clips than tracks: some tracks would have nothing on them,
        // and the harness must not reach for a clip that is not there.
        let report = run_spike(&SpikeConfig {
            clips: 2,
            edits: 20,
            seed: 3,
        })
        .expect("the spike must run");

        assert!(report.undo_restored_project);
        assert!(report.redo_restored_project);
    }
}
