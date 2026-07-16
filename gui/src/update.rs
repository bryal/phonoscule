//! The messages, and how each of them changes the model.

use crate::model::{
    App, ScanState, View, album_runs, current_album_id, current_glow, flow_target, glow_blend, queue_items, run_of,
};
use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
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
    /// A relative or absolute seek, from the keyboard or the OS media keys (the seek *bar* uses
    /// SeekChanged/SeekReleased instead, since a drag is a stream of absolute fractions).
    Seek(Seek),
    Frame(Instant),
}

/// A seek that isn't a drag of the bar: a jump relative to the current position, or to the start
/// of the track.
#[derive(Debug, Clone, Copy)]
pub enum Seek {
    By(SeekDirection, Duration),
    ToStart,
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
                // A new track invalidates any seek still settling against the old one.
                app.pending_seek = None;
                // A new album (re)starts the glow/cover-flow animation after a possibly long
                // idle stretch; reset the frame clock so the first frame's dt is one frame, not
                // the whole idle gap (which would jump the animation far in a single step).
                app.last_frame = Instant::now();
                push_media_metadata(app);
                push_media_playback(app);
            }
            player::Event::Progress(t) => {
                // While a seek is settling, ignore reports until playback reaches (roughly) the
                // requested position: earlier reports still in flight would otherwise yank the
                // optimistically-placed bar back to where playback was before the seek.
                if matches!(app.pending_seek, Some(target) if t.abs_diff(target) > SEEK_SETTLE) {
                    return Task::none();
                }
                app.pending_seek = None;
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
                app.pending_seek = None;
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
            MediaControlEvent::Seek(direction) => do_seek(app, Seek::By(direction, Duration::from_secs(5))),
            MediaControlEvent::SeekBy(direction, dt) => do_seek(app, Seek::By(direction, dt)),
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
        Msg::Seek(seek) => do_seek(app, seek),
        Msg::Frame(now) => {
            // Clamp to ~one frame: after an idle stretch (frames only run while animating) the
            // gap since the last frame would otherwise lurch every animation forward at once.
            let dt = (now - app.last_frame).as_secs_f32().min(1.0 / 30.0);
            app.last_frame = now;

            // Exponential ease of the cover flow towards the current album run.
            let target = flow_target(app);
            app.anim_pos += (target - app.anim_pos) * (1.0 - (-9.0 * dt).exp());
            if (target - app.anim_pos).abs() < 0.002 {
                app.anim_pos = target;
            }

            // The backdrop glow blends from the album we're leaving into the current one over
            // glow_p (see glow_blend). Start a fresh blend whenever the target changes (album
            // change, or a cover finishing loading), freezing the current on-screen glow as the
            // new starting point so an interruption mid-blend continues smoothly.
            const GLOW_RATE: f32 = 1.5; // blends per second (1/x s per transition)
            let target = current_glow(app);
            if current_album_id(app) != app.glow_album || target != app.glow_to {
                app.glow_from = glow_blend(app.glow_from, app.glow_to, app.glow_p);
                app.glow_to = target;
                app.glow_album = current_album_id(app);
                app.glow_p = 0.0;
            }
            app.glow_p = (app.glow_p + GLOW_RATE * dt).min(1.0);
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

/// How close a reported position must be to a pending seek's target to count as "arrived", after
/// which live reports drive the bar again. Wide enough that the first report once playback catches
/// up clears it, narrow relative to a seek step so it doesn't clear mid-scrub while holding a key.
const SEEK_SETTLE: Duration = Duration::from_secs(1);

/// Performs a [`Seek`]. The target is taken relative to [`pos`](App::pos) -- which is itself moved
/// optimistically below -- so a burst of relative seeks accumulates instead of all reading the
/// same round-trip-lagged position. Relative seeks saturate at zero and clamp to the track length.
fn do_seek(app: &mut App, seek: Seek) {
    let target = match seek {
        Seek::By(SeekDirection::Forward, dt) => app.pos.saturating_add(dt),
        Seek::By(SeekDirection::Backward, dt) => app.pos.saturating_sub(dt),
        Seek::ToStart => Duration::ZERO,
    };
    let target = app.len.map_or(target, |len| target.min(len));
    // Move the bar now and remember the target: the next relative seek reads this position, and
    // reports still in flight are ignored until playback reaches it (see the Progress handler).
    app.pos = target;
    app.pending_seek = Some(target);
    app.send(player::Cmd::Seek(target));
}

/// Translates a key press into a message, or `None` for keys we don't bind. `repeat` marks
/// auto-repeat from a held key: seeking honors it (hold an arrow to scrub), one-shot actions
/// don't (holding Space must not machine-gun play/pause). Any of Ctrl/Alt/Logo suppresses the
/// binding, so window-manager and future chorded shortcuts pass through untouched.
///
/// Bindings: Space toggles play/pause; Left/Right seek by [`SEEK_STEP`]; Shift+Left/Right step to
/// the previous/next track; Home restarts the track; Escape returns to the library.
pub fn key_to_msg(key: Key, modifiers: Modifiers, repeat: bool) -> Option<Msg> {
    /// How far a single Left/Right tap seeks.
    const SEEK_STEP: Duration = Duration::from_secs(5);
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    let one_shot = |msg| if repeat { None } else { Some(msg) };
    match (key, modifiers.shift()) {
        (Key::Named(Named::ArrowLeft), false) => Some(Msg::Seek(Seek::By(SeekDirection::Backward, SEEK_STEP))),
        (Key::Named(Named::ArrowRight), false) => Some(Msg::Seek(Seek::By(SeekDirection::Forward, SEEK_STEP))),
        (Key::Named(Named::ArrowLeft), true) => one_shot(Msg::Prev),
        (Key::Named(Named::ArrowRight), true) => one_shot(Msg::Next),
        (Key::Named(Named::Space), _) => one_shot(Msg::Toggle),
        (Key::Named(Named::Home), _) => one_shot(Msg::Seek(Seek::ToStart)),
        (Key::Named(Named::Escape), _) => one_shot(Msg::Show(View::Library)),
        _ => None,
    }
}
