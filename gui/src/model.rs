//! The application model: all state, and the queue/album-run bookkeeping around it.

use crate::update::Msg;
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
    pub media: media::Media,
    pub watcher: watcher::Watcher,
    /// The playback position last pushed to [`media`]: pushes are throttled to ~1/s, since each
    /// becomes a D-Bus signal.
    pub media_pos: Duration,
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
    /// Animated Cover Flow position, chasing `current`.
    pub anim_pos: f32,
    /// Animated backdrop glow color, chasing the playing album's accent.
    pub glow: iced::Color,
    pub last_frame: Instant,
}

pub fn boot(conf: Conf) -> impl Fn() -> (App, Task<Msg>) {
    move || {
        let app = App {
            engine: player::start(),
            media: media::start(),
            watcher: watcher::start(&conf.music_dir),
            media_pos: Duration::ZERO,
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
            anim_pos: 0.0,
            glow: iced::Color::BLACK,
            last_frame: Instant::now(),
        };
        let options = library::ScanOptions {
            root: conf.music_dir.clone(),
            known_covers: Default::default(),
            cache_file: library::default_cache_file(),
        };
        let scan = Task::run(library::scan(options), Msg::Library);
        (app, scan)
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

/// The backdrop glow color the application is (fading towards) showing: the playing album's
/// accent at full brightness -- normalized so its strongest channel saturates; the backdrop
/// shader decides how much of it to actually show.
pub fn glow_target(app: &App) -> iced::Color {
    match app.queue.get(app.current).and_then(|item| item.cover.as_ref()) {
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
    }
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
