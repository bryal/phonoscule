//! The application model: all state, and the queue/album-run bookkeeping around it.

use crate::update::Msg;
use futures::StreamExt;
use iced::Task;
use phonoscule_gui::conf::Conf;
use phonoscule_gui::library::{self, Album};
use phonoscule_gui::{media, player, watcher};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Library,
    NowPlaying,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    Scanning,
    Complete,
}

#[derive(Debug, Clone)]
pub struct QueueItem {
    pub path: PathBuf,
    pub album_id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover: Option<library::CoverArt>,
}

pub struct App {
    pub engine: player::Engine,
    /// Handle to the OS media integration. State changes are published to it (see
    /// [`publish_media`](crate::update)); a worker coalesces and pushes them, so the update loop
    /// holds no rate-limiting state of its own.
    pub media: media::Media,
    pub watcher: watcher::Watcher,
    pub conf: Conf,
    pub scan: ScanState,
    pub albums: Vec<Album>,
    pub view: View,
    pub queue: Vec<QueueItem>,
    pub current: usize,
    pub play_state: player::PlayState,
    pub pos: Duration,
    pub len: Option<Duration>,
    /// Seek-bar fraction while the user is dragging it.
    pub seek_drag: Option<f32>,
    /// The position last requested of the player, held until playback actually reaches it. Lets a
    /// burst of relative seeks (a held arrow key) accumulate from the requested position rather
    /// than the round-trip-lagged reported one, and stops stale in-flight reports from yanking the
    /// bar back to the pre-seek spot.
    pub pending_seek: Option<Duration>,
    /// When the last track skip from a held Home/End key fired, used to rate-limit its auto-repeat
    /// (see `skip_ready`). `None` until the first such skip.
    pub last_skip: Option<Instant>,
    /// When the current held Home/End press began, so its auto-repeat can accelerate the longer
    /// it's held (see `skip_interval`). Reset on each fresh press.
    pub hold_start: Option<Instant>,
    /// Animated Cover Flow position, chasing `current`.
    pub anim_pos: f32,
    /// The backdrop glow transitions between two album states: `glow_from` -> `glow_to` as
    /// `glow_p` runs 0 -> 1 (1 = settled on `glow_to`). `glow_album` is the album `glow_to`
    /// represents, so a change is detected when the current album differs.
    pub glow_from: GlowState,
    pub glow_to: GlowState,
    pub glow_album: u64,
    pub glow_p: f32,
    pub last_frame: Instant,
}

/// A backdrop glow: its color and its center as a fraction of the viewport. Album changes
/// animate one of these into another (see [`glow_blend`]).
#[derive(Clone, Copy, PartialEq)]
pub struct GlowState {
    pub color: iced::Color,
    pub center: (f32, f32),
}

pub fn boot(conf: Conf) -> impl Fn() -> (App, Task<Msg>) {
    move || {
        let (media, media_worker) = media::start();
        let app = App {
            engine: player::start(),
            media,
            watcher: watcher::start(&conf.music_dir),
            conf: conf.clone(),
            scan: ScanState::Scanning,
            albums: vec![],
            view: View::Library,
            queue: vec![],
            current: 0,
            play_state: player::PlayState::Paused,
            pos: Duration::ZERO,
            len: None,
            seek_drag: None,
            pending_seek: None,
            last_skip: None,
            hold_start: None,
            anim_pos: 0.0,
            glow_from: GlowState { color: iced::Color::BLACK, center: glow_center(0) },
            glow_to: GlowState { color: iced::Color::BLACK, center: glow_center(0) },
            glow_album: 0,
            glow_p: 1.0,
            last_frame: Instant::now(),
        };
        let options = library::ScanOptions {
            root: conf.music_dir.clone(),
            known_covers: Default::default(),
            cache_file: library::default_cache_file(),
            covers_dir: library::default_covers_dir(),
        };
        let scan = Task::run(library::scan(options), Msg::Library);
        // Run the media worker for the whole session, on iced's executor; it pushes to the OS and
        // yields no messages, so wrap its never-ending future as a stream that produces nothing.
        let media = Task::stream(futures::stream::once(media_worker.run()).filter_map(|()| async { None }));
        (app, Task::batch([scan, media]))
    }
}

impl App {
    pub fn send(&self, cmd: player::Cmd) {
        // The command channel is unbounded, so this only fails when the engine is gone.
        if self.engine.cmd.try_send(cmd).is_err() {
            log::error!("player engine is gone");
        }
    }
}

/// The queue's contiguous runs of tracks from the same album, as ranges into the queue. The
/// Cover Flow shows one cover per run rather than one per track.
pub fn album_runs(queue: &[QueueItem]) -> Vec<std::ops::Range<usize>> {
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    for (ix, item) in queue.iter().enumerate() {
        match runs.last_mut() {
            Some(run) if queue[run.start].album_id == item.album_id => run.end = ix + 1,
            _ => runs.push(ix..ix + 1),
        }
    }
    runs
}

/// The index of the run containing the given track index.
pub fn run_of(runs: &[std::ops::Range<usize>], track: usize) -> usize {
    runs.iter().position(|run| run.contains(&track)).unwrap_or(0)
}

/// The Cover Flow target position for the currently playing track.
pub fn flow_target(app: &App) -> f32 {
    run_of(&album_runs(&app.queue), app.current) as f32
}

/// The currently playing album's id (a stable, near-unique per-album value); 0 when nothing is
/// playing.
pub fn current_album_id(app: &App) -> u64 {
    app.queue.get(app.current).map_or(0, |item| item.album_id)
}

/// Where the currently playing album's glow sits, as a fraction of the viewport size. Scattered
/// per album by indexing fixed position tables with the album id (already a well-mixed hash);
/// the low digit picks the column, a higher one the row, so the two axes don't correlate.
pub fn glow_center(album_id: u64) -> (f32, f32) {
    const CENTERS_X: [f32; 7] = [0.20, 0.30, 0.40, 0.50, 0.60, 0.70, 0.80];
    const CENTERS_Y: [f32; 4] = [0.10, 0.20, 0.30, 0.40];
    let (nx, ny) = (CENTERS_X.len() as u64, CENTERS_Y.len() as u64);
    (CENTERS_X[(album_id % nx) as usize], CENTERS_Y[(album_id / nx % ny) as usize])
}

/// The glow the currently playing album should ultimately show: its accent at full brightness
/// (normalized so the strongest channel saturates) placed at its scattered position; black when
/// nothing is playing. The backdrop shader decides how much of the color to actually show.
pub fn current_glow(app: &App) -> GlowState {
    let color = match app.queue.get(app.current).and_then(|item| item.cover.as_ref()) {
        Some(cover) => {
            let accent = cover.accent;
            let max = accent.r.max(accent.g).max(accent.b);
            if max <= f32::EPSILON {
                iced::Color::BLACK
            } else {
                iced::Color { r: accent.r / max, g: accent.g / max, b: accent.b / max, a: 1.0 }
            }
        }
        None => iced::Color::BLACK,
    };
    GlowState { color, center: glow_center(current_album_id(app)) }
}

/// Blend between two glow states by progress `p` (0 = `from`, 1 = `to`): the center glides from one position to the
/// other while the color cross-fades, both smoothstep-eased. The color is interpolated in linear light so the midpoint
/// stays correctly bright, rather than dipping dark the way a gamma-space lerp of the sRGB values would.
pub fn glow_blend(from: GlowState, to: GlowState, p: f32) -> GlowState {
    let ease = p * p * (3.0 - 2.0 * p); // smoothstep
    let center = (from.center.0 + (to.center.0 - from.center.0) * ease, from.center.1 + (to.center.1 - from.center.1) * ease);
    let from_c = from.color.into_linear();
    let to_c = to.color.into_linear();
    let [r, g, b] = std::array::from_fn(|i| (1.0 - ease) * from_c[i] + ease * to_c[i]);
    let color = iced::Color::from_linear_rgba(r, g, b, 1.0);
    GlowState { color, center }
}

/// Whether the backdrop glow is mid-transition (or needs to start one), i.e. not settled on the
/// current album's target glow.
pub fn glow_animating(app: &App) -> bool {
    app.glow_p < 1.0 || app.glow_album != current_album_id(app) || app.glow_to != current_glow(app)
}

/// The glow to render this frame: the current point of the `glow_from` -> `glow_to` blend.
pub fn glow_now(app: &App) -> GlowState {
    glow_blend(app.glow_from, app.glow_to, app.glow_p)
}

pub fn queue_items(album: &Album) -> Vec<QueueItem> {
    album
        .tracks
        .iter()
        .map(|t| QueueItem {
            path: t.path.clone(),
            album_id: album.id,
            title: t.title.clone(),
            artist: album.artist.clone(),
            album: album.title.clone(),
            cover: album.cover.clone(),
        })
        .collect()
}
