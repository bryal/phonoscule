//! The messages, and how each of them changes the model.

use crate::model::{App, ScanState, View, album_runs, current_album_id, flow_target, glow_target, queue_items, run_of};
use iced::Task;
use phonoscule_gui::library::{self, Album};
use phonoscule_gui::player;
use souvlaki::{MediaControlEvent, MediaMetadata, MediaPlayback, MediaPosition, SeekDirection};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub enum Msg {
    Library(library::ScanEvent),
    /// Time to poll the music directory for changes.
    Rescan,
    Show(View),
    PlayAlbum(usize),
    QueueAlbum(usize),
    Player(player::Event),
    Media(MediaControlEvent),
    Toggle,
    Next,
    Prev,
    CoverClicked(usize),
    TrackClicked(usize),
    SeekChanged(f32),
    SeekReleased,
    Frame(Instant),
}

pub fn update(app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Library(library::ScanEvent::Album(mut album)) => {
            // Re-scans re-report albums we already have: upsert by the stable id, keeping the
            // already-loaded cover art when the cover is unchanged (the scanner skips
            // re-decoding and re-sending it).
            if let Some(ix) = app.albums.iter().position(|a| a.id == album.id) {
                let old = app.albums.remove(ix);
                if old.cover_id == album.cover_id {
                    album.cover = old.cover;
                }
            }
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
            // The playing track's cover art may just have arrived.
            if app.queue.get(app.current).is_some_and(|item| albums.contains(&item.album_id)) {
                push_media_metadata(app);
            }
        }
        Msg::Library(library::ScanEvent::Done { album_ids }) => {
            let ids: std::collections::HashSet<u64> = album_ids.into_iter().collect();
            app.albums.retain(|album| ids.contains(&album.id));
            app.scan = ScanState::Complete;
        }
        Msg::Rescan => match app.scan {
            // The running scan will pick changes up anyway.
            ScanState::Scanning => (),
            ScanState::Complete => {
                app.scan = ScanState::Scanning;
                return Task::run(library::scan(rescan_options(app)), Msg::Library);
            }
        },
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
                // A new album (re)starts the glow/cover-flow animation after a possibly long
                // idle stretch; reset the frame clock so the first frame's dt is one frame, not
                // the whole idle gap (which would jump the animation far in a single step).
                app.last_frame = Instant::now();
                push_media_metadata(app);
                push_media_playback(app);
            }
            player::Event::Progress(t) => {
                app.pos = t;
                if app.pos.abs_diff(app.media_pos) >= Duration::from_secs(1) {
                    push_media_playback(app);
                }
            }
            player::Event::PlayState(state) => {
                app.play_state = state;
                push_media_playback(app);
            }
            player::Event::QueueEnded => {
                app.play_state = player::PlayState::Paused;
                // The queue may have ended through a skip: rest the bar at the end rather than
                // wherever the last track happened to be.
                app.pos = app.len.unwrap_or(Duration::ZERO);
                app.media.set_playback(MediaPlayback::Stopped);
            }
        },
        Msg::Media(event) => match event {
            MediaControlEvent::Play => match app.play_state {
                player::PlayState::Paused => app.send(player::Cmd::TogglePlayPause),
                player::PlayState::Playing => (),
            },
            // We have no stopped-with-a-track-open state; pausing is the closest thing.
            MediaControlEvent::Pause | MediaControlEvent::Stop => match app.play_state {
                player::PlayState::Playing => app.send(player::Cmd::TogglePlayPause),
                player::PlayState::Paused => (),
            },
            MediaControlEvent::Toggle => app.send(player::Cmd::TogglePlayPause),
            MediaControlEvent::Next => app.send(player::Cmd::Next),
            MediaControlEvent::Previous => app.send(player::Cmd::Prev),
            MediaControlEvent::Seek(direction) => media_seek(app, direction, Duration::from_secs(5)),
            MediaControlEvent::SeekBy(direction, dt) => media_seek(app, direction, dt),
            MediaControlEvent::SetPosition(MediaPosition(t)) => app.send(player::Cmd::Seek(t)),
            // No volume control (yet): playback follows the system volume.
            MediaControlEvent::SetVolume(_) => (),
            MediaControlEvent::OpenUri(_) => (),
            // TODO: raise the window (needs a runtime window task in iced).
            MediaControlEvent::Raise => (),
            MediaControlEvent::Quit => (),
        },
        Msg::Toggle => app.send(player::Cmd::TogglePlayPause),
        Msg::Next => app.send(player::Cmd::Next),
        Msg::Prev => app.send(player::Cmd::Prev),
        Msg::CoverClicked(ix) => {
            let runs = album_runs(&app.queue);
            if ix == run_of(&runs, app.current) {
                app.send(player::Cmd::TogglePlayPause);
            } else if let Some(run) = runs.get(ix) {
                app.send(player::Cmd::JumpTo(run.start));
            }
        }
        Msg::TrackClicked(ix) => app.send(player::Cmd::JumpTo(ix)),
        Msg::SeekChanged(frac) => app.seek_drag = Some(frac),
        Msg::SeekReleased => {
            if let (Some(frac), Some(len)) = (app.seek_drag.take(), app.len) {
                let t = len.mul_f32(frac.clamp(0.0, 1.0));
                // No optimistic position update: the player reports its position right after
                // seeking (stale in-flight reports can't yank the bar around this way, at the
                // price of the bar resting at the old position for the seek's few ms).
                app.send(player::Cmd::Seek(t));
            }
        }
        Msg::Frame(now) => {
            // Clamp to ~one frame: after an idle stretch (frames only run while animating) the
            // gap since the last frame would otherwise lurch every animation forward at once.
            let dt = (now - app.last_frame).as_secs_f32().min(1.0 / 30.0);
            app.last_frame = now;

            // Exponential ease of the cover flow towards the current album run.
            let target = flow_target(app);
            app.anim_pos += (target - app.anim_pos) * (1.0 - (-6.0 * dt).exp());
            if (target - app.anim_pos).abs() < 0.002 {
                app.anim_pos = target;
            }

            // The backdrop glow. On the same album it just fades its color towards the accent.
            // On an album change the glow position is fixed per album, so a direct color fade
            // would slide the glow across the screen mid-fade; instead fade the color down to
            // black, swap the position seed while it is dark, then fade the new color up -- a
            // cross-dissolve through black. Linear (constant speed), so the fade reads evenly:
            // an exponential ease front-loads the drop then crawls a long near-black tail.
            const RATE: f32 = 3.0; // color-units per second
            let album = current_album_id(app);
            let goal = if app.glow_seed == album { glow_target(app) } else { iced::Color::BLACK };
            let max_step = RATE * dt;
            let approach = |from: f32, to: f32| from + (to - from).clamp(-max_step, max_step);
            app.glow = iced::Color {
                r: approach(app.glow.r, goal.r),
                g: approach(app.glow.g, goal.g),
                b: approach(app.glow.b, goal.b),
                a: 1.0,
            };
            // Reached black at the end of a fade-out: adopt the new album's position (invisible
            // while dark), so the next frames fade its color up in the new spot.
            if app.glow_seed != album && app.glow.r.max(app.glow.g).max(app.glow.b) <= 0.0 {
                app.glow_seed = album;
            }
        }
    }
    Task::none()
}

/// Options for a periodic re-scan: skip re-decoding all the cover art we already hold.
fn rescan_options(app: &App) -> library::ScanOptions {
    library::ScanOptions {
        root: app.conf.music_dir.clone(),
        known_covers: app.albums.iter().filter_map(|a| a.cover.as_ref().map(|c| c.id)).collect(),
        cache_file: library::default_cache_file(),
    }
}

/// Pushes the playing track's metadata to the OS media integration.
fn push_media_metadata(app: &mut App) {
    let Some(item) = app.queue.get(app.current) else { return };
    let cover_url = item.cover.as_ref().and_then(|c| url::Url::from_file_path(&*c.file).ok()).map(String::from);
    app.media.set_metadata(MediaMetadata {
        title: Some(&item.title),
        album: Some(&item.album),
        artist: Some(&item.artist),
        cover_url: cover_url.as_deref(),
        duration: app.len,
    });
}

/// Pushes the playback state & position to the OS media integration.
fn push_media_playback(app: &mut App) {
    let progress = Some(MediaPosition(app.pos));
    let playback = match app.play_state {
        player::PlayState::Playing => MediaPlayback::Playing { progress },
        player::PlayState::Paused => MediaPlayback::Paused { progress },
    };
    app.media_pos = app.pos;
    app.media.set_playback(playback);
}

/// Seeks relative to the current position, on behalf of the OS media integration.
fn media_seek(app: &mut App, direction: SeekDirection, dt: Duration) {
    let target = match direction {
        SeekDirection::Forward => app.pos.saturating_add(dt),
        SeekDirection::Backward => app.pos.saturating_sub(dt),
    };
    app.send(player::Cmd::Seek(target));
}
