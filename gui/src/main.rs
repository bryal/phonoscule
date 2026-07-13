//! Phonoscule GUI: an album-focused music player.
//!
//! Two views: a library browser (play or queue whole albums) and an iPod-style Cover Flow of the
//! play queue with a seekable playback bar.

use phonoscule_gui::{conf, coverflow, library, player};

use conf::Conf;
use coverflow::cover_flow;
use futures::StreamExt;
use iced::widget::{button, column, container, image, row, scrollable, slider, text};
use iced::{Center, Element, Fill, Subscription, Task, Theme};
use library::Album;
use smol::channel;
use std::cmp::min;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    simple_logger::SimpleLogger::new().with_level(log::LevelFilter::Info).env().init().unwrap();

    let mut args = std::env::args().skip(1);
    let arg_conf_path = args.next().map(PathBuf::from);
    anyhow::ensure!(args.next().is_none(), "expected at most one argument: a path to a config file");
    let conf = smol::block_on(Conf::load(conf::locate(arg_conf_path)))?;

    iced::application(boot(conf), update, view)
        .title("Phonoscule")
        .subscription(subscription)
        .theme(theme)
        .style(style)
        .run()?;
    Ok(())
}

fn style(_app: &App, theme: &Theme) -> iced::theme::Style {
    iced::theme::Style { background_color: iced::Color::BLACK, text_color: theme.palette().text }
}

fn theme(_app: &App) -> Theme {
    Theme::Dark
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Library,
    NowPlaying,
}

#[derive(Debug, Clone)]
struct QueueItem {
    path: PathBuf,
    album_id: u64,
    title: String,
    artist: String,
    album: String,
    cover: Option<library::CoverArt>,
}

struct App {
    engine: player::Engine,
    conf: Conf,
    scanning: bool,
    albums: Vec<Album>,
    view: View,
    queue: Vec<QueueItem>,
    current: usize,
    playing: bool,
    pos: Duration,
    len: Option<Duration>,
    /// Seek-bar fraction while the user is dragging it.
    seek_drag: Option<f32>,
    /// Animated Cover Flow position, chasing `current`.
    anim_pos: f32,
    last_frame: Instant,
}

#[derive(Debug, Clone)]
enum Msg {
    Library(library::ScanEvent),
    Show(View),
    PlayAlbum(usize),
    QueueAlbum(usize),
    Player(player::Event),
    Toggle,
    Next,
    Prev,
    CoverClicked(usize),
    SeekChanged(f32),
    SeekReleased,
    Frame(Instant),
}

fn boot(conf: Conf) -> impl Fn() -> (App, Task<Msg>) {
    move || {
        let app = App {
            engine: player::start(),
            conf: conf.clone(),
            scanning: true,
            albums: vec![],
            view: View::Library,
            queue: vec![],
            current: 0,
            playing: false,
            pos: Duration::ZERO,
            len: None,
            seek_drag: None,
            anim_pos: 0.0,
            last_frame: Instant::now(),
        };
        let scan = Task::run(library::scan(conf.music_dir.clone()), Msg::Library);
        (app, scan)
    }
}

impl App {
    fn send(&self, cmd: player::Cmd) {
        // The command channel is unbounded, so this only fails when the engine is gone.
        if self.engine.cmd.try_send(cmd).is_err() {
            log::error!("player engine is gone");
        }
    }
}

fn queue_items(album: &Album) -> Vec<QueueItem> {
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

fn update(app: &mut App, msg: Msg) {
    match msg {
        Msg::Library(library::ScanEvent::Album(album)) => {
            // Keep the browser sorted; scan order is nondeterministic (directories complete
            // in parallel).
            let key = |a: &Album| (a.artist.to_lowercase(), a.title.to_lowercase());
            let ix = app.albums.partition_point(|a| key(a) <= key(&album));
            app.albums.insert(ix, album);
        }
        Msg::Library(library::ScanEvent::Cover { albums, art }) => {
            for album in app.albums.iter_mut().filter(|a| albums.contains(&a.id)) {
                album.cover = Some(art.clone());
            }
            for item in app.queue.iter_mut().filter(|i| albums.contains(&i.album_id)) {
                item.cover = Some(art.clone());
            }
        }
        Msg::Library(library::ScanEvent::Done) => app.scanning = false,
        Msg::Show(v) => app.view = v,
        Msg::PlayAlbum(ix) => {
            let items = queue_items(&app.albums[ix]);
            app.send(player::Cmd::SetQueue { tracks: items.iter().map(|i| i.path.clone()).collect(), start: 0 });
            app.queue = items;
            app.current = 0;
            app.anim_pos = 0.0;
            app.view = View::NowPlaying;
        }
        Msg::QueueAlbum(ix) => {
            let items = queue_items(&app.albums[ix]);
            app.send(player::Cmd::Append { tracks: items.iter().map(|i| i.path.clone()).collect() });
            app.queue.extend(items);
        }
        Msg::Player(event) => match event {
            player::Event::TrackStarted { ix, len } => {
                app.current = ix;
                app.len = len;
                app.pos = Duration::ZERO;
            }
            player::Event::Progress(t) => {
                if app.seek_drag.is_none() {
                    app.pos = t;
                }
            }
            player::Event::Playing(playing) => app.playing = playing,
            player::Event::QueueEnded => app.playing = false,
        },
        Msg::Toggle => app.send(player::Cmd::TogglePlayPause),
        Msg::Next => app.send(player::Cmd::Next),
        Msg::Prev => app.send(player::Cmd::Prev),
        Msg::CoverClicked(ix) => {
            if ix == app.current {
                app.send(player::Cmd::TogglePlayPause);
            } else {
                app.send(player::Cmd::JumpTo(ix));
            }
        }
        Msg::SeekChanged(frac) => app.seek_drag = Some(frac),
        Msg::SeekReleased => {
            if let (Some(frac), Some(len)) = (app.seek_drag.take(), app.len) {
                let t = len.mul_f32(frac.clamp(0.0, 1.0));
                app.pos = t;
                app.send(player::Cmd::Seek(t));
            }
        }
        Msg::Frame(now) => {
            let dt = (now - app.last_frame).as_secs_f32().min(0.1);
            app.last_frame = now;
            let target = app.current as f32;
            // Exponential ease towards the current track.
            app.anim_pos += (target - app.anim_pos) * (1.0 - (-10.0 * dt).exp());
            if (target - app.anim_pos).abs() < 0.002 {
                app.anim_pos = target;
            }
        }
    }
}

fn subscription(app: &App) -> Subscription<Msg> {
    struct EventsRx(channel::Receiver<player::Event>);
    impl std::hash::Hash for EventsRx {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            "player-events".hash(state)
        }
    }
    let events =
        Subscription::run_with(EventsRx(app.engine.events.clone()), |rx| rx.0.clone().map(Msg::Player));

    let animating = app.view == View::NowPlaying && app.anim_pos != app.current as f32;
    let frames = if animating {
        iced::time::every(Duration::from_millis(16)).map(Msg::Frame)
    } else {
        Subscription::none()
    };

    Subscription::batch([events, frames])
}

fn view(app: &App) -> Element<'_, Msg> {
    let tab = |label, target| {
        let b = button(text(label).size(14)).on_press(Msg::Show(target));
        if app.view == target { b } else { b.style(button::secondary) }
    };
    let top = row![tab("Library", View::Library), tab("Now Playing", View::NowPlaying)].spacing(8).padding(8);
    let body = match app.view {
        View::Library => library_view(app),
        View::NowPlaying => now_playing_view(app),
    };
    column![top, body].into()
}

fn library_view(app: &App) -> Element<'_, Msg> {
    if app.albums.is_empty() {
        let status = if app.scanning { "Scanning" } else { "No albums found under" };
        return container(text(format!("{status} {:?}…", app.conf.music_dir))).center(Fill).into();
    }
    const COLS: usize = 4;
    let mut grid = column![].spacing(24).padding(16);
    if app.scanning {
        grid = grid.push(text(format!("Scanning {:?}…", app.conf.music_dir)).size(12).style(text::secondary));
    }
    for (row_ix, albums) in app.albums.chunks(COLS).enumerate() {
        let mut r = row![].spacing(16);
        for (col_ix, album) in albums.iter().enumerate() {
            r = r.push(album_card(row_ix * COLS + col_ix, album));
        }
        grid = grid.push(r);
    }
    scrollable(grid).height(Fill).into()
}

fn album_card(ix: usize, album: &Album) -> Element<'_, Msg> {
    const SIDE: f32 = 168.0;
    let cover: Element<'_, Msg> = match &album.cover {
        Some(c) => image(c.handle.clone())
            .width(SIDE)
            .height(SIDE)
            .content_fit(iced::ContentFit::Cover)
            .into(),
        None => container(text(&album.title).size(16).center())
            .width(SIDE)
            .height(SIDE)
            .center(SIDE)
            .style(container::rounded_box)
            .into(),
    };
    column![
        button(cover).padding(0).style(button::text).on_press(Msg::PlayAlbum(ix)),
        text(&album.title).size(14),
        text(&album.artist).size(12).style(text::secondary),
        row![
            button(text("Play").size(12)).on_press(Msg::PlayAlbum(ix)),
            button(text("Queue").size(12)).style(button::secondary).on_press(Msg::QueueAlbum(ix)),
        ]
        .spacing(8),
    ]
    .spacing(4)
    .width(SIDE)
    .into()
}

fn now_playing_view(app: &App) -> Element<'_, Msg> {
    if app.queue.is_empty() {
        return container(text("Play or queue an album from the library")).center(Fill).into();
    }
    let current = &app.queue[min(app.current, app.queue.len() - 1)];

    let covers = app.queue.iter().map(|item| item.cover.clone()).collect();
    let flow = cover_flow(covers, app.anim_pos, Msg::CoverClicked);

    let shown_pos = match (app.seek_drag, app.len) {
        (Some(frac), Some(len)) => len.mul_f32(frac.clamp(0.0, 1.0)),
        _ => app.pos,
    };
    let frac = match (app.seek_drag, app.len) {
        (Some(frac), _) => frac,
        (None, Some(len)) if !len.is_zero() => (app.pos.as_secs_f32() / len.as_secs_f32()).clamp(0.0, 1.0),
        _ => 0.0,
    };
    let seek_bar = row![
        text(fmt_time(shown_pos)).size(12),
        slider(0.0..=1.0f32, frac, Msg::SeekChanged)
            .step(0.001_f32)
            .on_release(Msg::SeekReleased)
            .width(Fill),
        text(app.len.map(fmt_time).unwrap_or_else(|| "--:--".into())).size(12),
    ]
    .spacing(12)
    .align_y(Center);

    let controls = row![
        button(text("⏮").size(18)).style(button::text).on_press(Msg::Prev),
        button(text(if app.playing { "⏸" } else { "▶" }).size(24)).style(button::text).on_press(Msg::Toggle),
        button(text("⏭").size(18)).style(button::text).on_press(Msg::Next),
    ]
    .spacing(24)
    .align_y(Center);

    column![
        flow,
        text(&current.title).size(20),
        text(format!("{} — {}", current.artist, current.album)).size(14).style(text::secondary),
        seek_bar,
        controls,
    ]
    .spacing(10)
    .padding(16)
    .align_x(Center)
    .into()
}

fn fmt_time(t: Duration) -> String {
    format!("{:02}:{:02}", t.as_secs() / 60, t.as_secs() % 60)
}
