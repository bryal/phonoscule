//! Everything the core holds: the library, the play queue, and the engine playing it.
//!
//! One [`Core`] lives for the whole process, so it survives Android rebuilding the Activity, and the
//! media service and the screens share it. Kotlin asks it questions and gives it commands through
//! [`proto`](crate::proto), and learns what changed by waiting on [`Core::wait_notices`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use futures::StreamExt;
use phonoscule::fs::Fs;
use phonoscule::library::{self, Album, ScanEvent};
use phonoscule::player::{self, PlayState, Repeat};
use phonoscule::queue::{self, Grouping, Scope};
use phonoscule::sort::{Dir, SortField, SortOrder};
use phonoscule::{search, session};

use crate::saf::{self, Saf};

/// The edge the scan decodes thumbnails to. Nothing draws them: they are only what a cover's accent
/// is read from, so they are small.
const THUMB_EDGE: u32 = 64;

/// How near a reported position must come to a pending seek's target for the seek to have landed.
const SEEK_SETTLE: Duration = Duration::from_secs(1);

/// The order the library starts in, before the listener picks one: by artist, then by year within
/// each artist, as in the terminal player.
const INITIAL_SORT: SortOrder = SortOrder { group_by_artist: Some(Dir::Asc), field: SortField::Year, field_dir: Dir::Asc };

/// Where the core keeps its own files. The listener's music is never among them: that is read
/// through [`Saf`].
pub struct Paths {
    tag_cache: PathBuf,
    album_index: PathBuf,
    thumbs: PathBuf,
    full_covers: PathBuf,
    playlist: PathBuf,
    player: PathBuf,
}

impl Paths {
    /// `files` for what must survive a cache wipe, `cache` for what a rescan rebuilds.
    pub fn new(files: &Path, cache: &Path) -> Self {
        Paths {
            tag_cache: cache.join("tags.json"),
            album_index: cache.join("albums.json"),
            thumbs: library::covers_dir(cache, THUMB_EDGE),
            full_covers: library::covers_dir(cache, library::FULL),
            playlist: files.join("playlist.json"),
            player: files.join("player.json"),
        }
    }
}

/// A track in the play queue. Its album is named by id: the album list holds its tags and cover.
#[derive(Debug, Clone)]
pub struct QueueItem {
    pub path: PathBuf,
    pub album_id: u64,
    pub title: String,
}

/// What changed since Kotlin last asked. See [`Core::wait_notices`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Changes {
    /// The albums, or something shown about them.
    pub library: bool,
    /// The queue's tracks or their order.
    pub queue: bool,
    /// What is playing, whether it is, or how far in.
    pub playback: bool,
    /// Whether a scan has started, got further, or finished.
    pub scanning: bool,
    /// Albums whose cover can now be had, or has changed.
    pub covers: HashSet<u64>,
}

impl Changes {
    fn any(&self) -> bool {
        self.library || self.queue || self.playback || self.scanning || !self.covers.is_empty()
    }
}

struct State {
    /// By artist, then title, which is only the order scans upsert in; a listing sorts its own.
    albums: Vec<Album>,
    /// Where each cover's artwork is, by cover id, as this run's scans have found them.
    artwork: HashMap<u64, Arc<PathBuf>>,
    queue: Vec<QueueItem>,
    current: usize,
    play_state: PlayState,
    position: Duration,
    length: Option<Duration>,
    repeat: Repeat,
    sort: SortOrder,
    scanning: bool,
    /// How far the running scan has come, once it has said.
    scan_progress: Option<library::ScanProgress>,
    rescan_queued: bool,
    /// Where the listener last seeked to, until the engine reports a position near it. Reports
    /// already on their way when the seek was sent would otherwise drag the bar back.
    pending_seek: Option<Duration>,
}

pub struct Core {
    saf: Arc<Saf>,
    paths: Paths,
    engine: player::Engine,
    state: Mutex<State>,
    /// Held through each save of the session, so that saves land one at a time and the last holds the latest state.
    saving: Mutex<()>,
    changes: Mutex<Changes>,
    changed: Condvar,
}

/// The playing track and the state around it, as the player screen and the media session show it.
#[derive(Debug, Clone, PartialEq)]
pub struct Playback {
    /// The queue index of the track, or `None` while the queue is empty.
    pub current: Option<usize>,
    pub playing: bool,
    pub position: Duration,
    pub length: Option<Duration>,
    pub repeat: Repeat,
    pub title: String,
    pub album: Option<AlbumRef>,
    /// The album's cover as a JPEG file, if one has been made.
    pub cover_file: Option<PathBuf>,
}

/// Enough of an album to show it beside something else.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumRef {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub cover: Option<u64>,
    pub accent: Option<library::Rgb>,
}

impl AlbumRef {
    fn of(album: &Album) -> Self {
        AlbumRef {
            id: album.id,
            title: album.title.clone(),
            artist: album.artist.clone(),
            cover: album.cover_id,
            accent: album.accent,
        }
    }
}

/// An album run of the queue, for the cover flow.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub start: usize,
    pub end: usize,
    /// `None` while the scan has not yet found the album the run's tracks belong to.
    pub album: Option<AlbumRef>,
}

impl Core {
    /// Loads the library and the session the last run left, and readies the engine on them,
    /// paused. Returns at once: the scan is Kotlin's to start.
    pub fn new(paths: Paths) -> Self {
        let saf = Arc::new(Saf::new());
        let engine = player::start(
            player::Client { name: "phonoscule-android".into(), description: "Phonoscule".into() },
            Arc::clone(&saf) as Arc<dyn Fs>,
        );

        let albums = smol::block_on(library::load_index(Some(paths.album_index.clone())));
        // Every track, whether the index knows it or not: the index is a cache, which Android may clear, and the queue
        // outlives it. A track gone from the music is skipped when it comes to play.
        let restored = smol::block_on(session::load(Some(paths.playlist.clone()), Some(paths.player.clone()), |_| true));
        let mut queue: Vec<QueueItem> =
            restored.tracks.into_iter().map(|path| QueueItem { path, album_id: 0, title: String::new() }).collect();
        hydrate(&albums, &mut queue);
        if !queue.is_empty() {
            let tracks = entries(&queue);
            let _ =
                engine.cmd.send_blocking(player::Cmd::SetQueue { tracks, start: restored.current, play: PlayState::Paused });
        }
        let _ = engine.cmd.send_blocking(player::Cmd::SetRepeat(restored.repeat));

        let state = State {
            albums,
            artwork: HashMap::new(),
            queue,
            current: restored.current,
            play_state: PlayState::Paused,
            position: Duration::ZERO,
            length: None,
            repeat: restored.repeat,
            sort: restored.sort.unwrap_or(INITIAL_SORT),
            scanning: false,
            scan_progress: None,
            rescan_queued: false,
            pending_seek: None,
        };
        Core {
            saf,
            paths,
            engine,
            state: Mutex::new(state),
            saving: Mutex::new(()),
            changes: Mutex::new(Changes::default()),
            changed: Condvar::new(),
        }
    }

    /// Starts the thread that follows the engine. Call once, on the core that will live for good.
    pub fn follow_engine(&'static self) {
        let events = self.engine.events.clone();
        std::thread::Builder::new()
            .name("phonoscule-events".into())
            .spawn(move || {
                while let Ok(event) = events.recv_blocking() {
                    self.on_engine(event);
                }
            })
            .expect("cannot spawn the engine event thread");
    }

    pub fn saf(&self) -> &Saf {
        &self.saf
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A poisoned lock means a panic halfway through changing the state. Carry on with it: the
        // worst it can be is a queue out of step with the engine, which the next command fixes.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn note(&self, change: impl FnOnce(&mut Changes)) {
        change(&mut self.changes.lock().unwrap_or_else(|e| e.into_inner()));
        self.changed.notify_all();
    }

    /// Waits until something has changed, then returns everything that has, and forgets it.
    pub fn wait_notices(&self) -> Changes {
        let mut changes = self.changes.lock().unwrap_or_else(|e| e.into_inner());
        while !changes.any() {
            changes = self.changed.wait(changes).unwrap_or_else(|e| e.into_inner());
        }
        std::mem::take(&mut *changes)
    }

    fn send(&self, cmd: player::Cmd) {
        if self.engine.cmd.send_blocking(cmd).is_err() {
            log::error!("the engine has stopped");
        }
    }

    // ----- The library -----

    /// Scans the picked folder, or, while a scan runs, queues one more after it.
    pub fn scan(&'static self) {
        {
            let mut state = self.state();
            if state.scanning {
                state.rescan_queued = true;
                return;
            }
            state.scanning = true;
        }
        self.note(|c| c.scanning = true);
        std::thread::Builder::new()
            .name("phonoscule-scan".into())
            .spawn(move || {
                loop {
                    // Caught, so that a scan that fails still ends, and another can start. The panic hook has logged it.
                    let scan = std::panic::AssertUnwindSafe(|| smol::block_on(self.run_scan()));
                    if std::panic::catch_unwind(scan).is_err() {
                        log::error!("the scan failed, and the library is as it was before it");
                    }
                    let mut state = self.state();
                    state.scan_progress = None;
                    if !std::mem::take(&mut state.rescan_queued) {
                        state.scanning = false;
                        break;
                    }
                }
                self.note(|c| c.scanning = true);
            })
            .expect("cannot spawn the scan thread");
    }

    pub fn scanning(&self) -> bool {
        self.state().scanning
    }

    /// How far the running scan has come, once it has said.
    pub fn scan_progress(&self) -> Option<library::ScanProgress> {
        self.state().scan_progress
    }

    async fn run_scan(&self) {
        let options = {
            let state = self.state();
            // A cover whose full-size copy is cached already, with its accent in the index, needs
            // nothing from its artwork. Telling the scan so saves decoding every sleeve again.
            let known_covers = state
                .albums
                .iter()
                .filter(|a| a.accent.is_some())
                .filter_map(|a| a.cover_id)
                .filter(|&id| library::cover_file(&self.paths.full_covers, id, library::FULL_FORMAT).exists())
                .collect();
            library::ScanOptions {
                fs: Arc::clone(&self.saf) as Arc<dyn Fs>,
                root: PathBuf::from(saf::ROOT),
                priority: queue::outward(&state.queue.iter().map(|item| item.album_id).collect::<Vec<_>>(), state.current),
                known_covers,
                cache_file: Some(self.paths.tag_cache.clone()),
                covers_dir: Some(self.paths.thumbs.clone()),
                thumb_edge: THUMB_EDGE,
                full_covers_dir: Some(self.paths.full_covers.clone()),
            }
        };
        let mut events = std::pin::pin!(library::scan(options));
        while let Some(event) = events.next().await {
            if let Some(save) = self.absorb(event) {
                save.await;
            }
        }
    }

    /// Takes in one scan event. Returns the index write the scan's end calls for.
    fn absorb(&self, event: ScanEvent) -> Option<impl Future<Output = ()> + use<>> {
        let mut state = self.state();
        match event {
            ScanEvent::Album(album) => {
                if !library::upsert(&mut state.albums, *album) {
                    return None;
                }
                drop(state);
                self.note(|c| c.library = true);
                None
            }
            ScanEvent::Cover { albums, art, .. } => {
                state.artwork.insert(art.id, Arc::clone(&art.file));
                for album in state.albums.iter_mut().filter(|a| albums.contains(&a.id) && a.cover_id == Some(art.id)) {
                    album.accent = Some(art.accent);
                }
                drop(state);
                self.note(|c| {
                    c.library = true;
                    c.covers.extend(albums);
                });
                None
            }
            ScanEvent::Progress(progress) => {
                state.scan_progress = Some(progress);
                drop(state);
                self.note(|c| c.scanning = true);
                None
            }
            ScanEvent::Done { album_ids, complete } => {
                library::drop_missing(&mut state.albums, &album_ids, complete);
                let State { albums, queue, .. } = &mut *state;
                hydrate(albums, queue);
                let save = library::save_index(Some(self.paths.album_index.clone()), &state.albums);
                drop(state);
                self.note(|c| {
                    c.library = true;
                    c.queue = true;
                    c.playback = true;
                });
                Some(save)
            }
        }
    }

    /// The albums whose title or artist matches `query`, best match first and in the current sort
    /// order otherwise, limited to those whose genre is one of `genres` unless there are none, and
    /// likewise for `artists`.
    pub fn albums(&self, query: &str, genres: &[String], artists: &[String]) -> Vec<Album> {
        let state = self.state();
        let mut ranked: Vec<(&Album, usize)> = state
            .albums
            .iter()
            .filter(|a| genres.is_empty() || genres.contains(&a.genre))
            .filter(|a| artists.is_empty() || artists.contains(&a.artist))
            .filter_map(|a| Some((a, search::rank(&format!("{} {}", a.title, a.artist), query)?)))
            .collect();
        ranked.sort_by(|(a, ra), (b, rb)| rb.cmp(ra).then_with(|| state.sort.cmp(a, b)));
        ranked.into_iter().map(|(a, _)| a.clone()).collect()
    }

    pub fn album(&self, id: u64) -> Option<Album> {
        self.state().albums.iter().find(|a| a.id == id).cloned()
    }

    /// Every genre some album has, sorted.
    pub fn genres(&self) -> Vec<String> {
        sorted_names(self.state().albums.iter().map(|a| &a.genre))
    }

    /// Every artist some album is by, sorted.
    pub fn artists(&self) -> Vec<String> {
        sorted_names(self.state().albums.iter().map(|a| &a.artist))
    }

    pub fn sort(&self) -> SortOrder {
        self.state().sort
    }

    pub fn set_sort(&self, sort: SortOrder) {
        self.state().sort = sort;
        self.save_player();
        self.note(|c| c.library = true);
    }

    /// The full-size copy of a cover, as a JPEG file Kotlin can decode, making it from the artwork
    /// if it is not there yet. Blocks while it does. `None` if the artwork has not been found by a
    /// scan in this run, or will not decode.
    pub fn cover(&self, cover: u64) -> Option<PathBuf> {
        let path = library::cover_file(&self.paths.full_covers, cover, library::FULL_FORMAT);
        if path.exists() {
            return Some(path);
        }
        let artwork = self.state().artwork.get(&cover).cloned()?;
        let _ = std::fs::create_dir_all(&self.paths.full_covers);
        smol::block_on(library::cache_full_cover(&*self.saf, &artwork, path.clone(), library::FULL));
        path.exists().then_some(path)
    }

    // ----- The queue -----

    pub fn queue(&self) -> (Vec<QueueItem>, Vec<Run>) {
        let state = self.state();
        let albums: HashMap<u64, &Album> = state.albums.iter().map(|a| (a.id, a)).collect();
        let runs = queue::runs(state.queue.iter().map(|item| item.album_id))
            .into_iter()
            .map(|run| Run {
                album: albums.get(&state.queue[run.start].album_id).map(|a| AlbumRef::of(a)),
                start: run.start,
                end: run.end,
            })
            .collect();
        (state.queue.clone(), runs)
    }

    /// Replaces the queue with the tracks of `albums`, and plays from track `track` of the first.
    pub fn play(&self, albums: &[u64], track: usize) {
        let mut state = self.state();
        let items = items_of(&state.albums, albums);
        if items.is_empty() {
            return;
        }
        let start = track.min(items.len() - 1);
        self.send(player::Cmd::SetQueue { tracks: entries(&items), start, play: PlayState::Playing });
        state.queue = items;
        state.current = start;
        state.position = Duration::ZERO;
        drop(state);
        self.queue_changed();
    }

    /// Adds the tracks of `albums` to the end of the queue, or only track `track` of the one album.
    pub fn enqueue(&self, albums: &[u64], track: Option<usize>) {
        let mut state = self.state();
        let mut items = items_of(&state.albums, albums);
        if let Some(track) = track {
            items = items.into_iter().nth(track).into_iter().collect();
        }
        self.send(player::Cmd::Append { tracks: entries(&items) });
        state.queue.extend(items);
        drop(state);
        self.queue_changed();
    }

    pub fn clear(&self) {
        let mut state = self.state();
        self.send(player::Cmd::SetQueue { tracks: vec![], start: 0, play: PlayState::Paused });
        state.queue.clear();
        state.current = 0;
        state.position = Duration::ZERO;
        state.length = None;
        drop(state);
        self.queue_changed();
    }

    /// Shuffles the queue in place: the new order *is* the queue, so what the player shows is what
    /// will play. See [`Scope`] for what happens to the playing track.
    pub fn shuffle(&self, grouping: Grouping, scope: Scope) {
        let mut state = self.state();
        if state.queue.is_empty() {
            return;
        }
        let albums: Vec<u64> = state.queue.iter().map(|item| item.album_id).collect();
        let order = queue::shuffle(&albums, state.current, grouping, scope, queue::seed());
        let mut old: Vec<Option<QueueItem>> = std::mem::take(&mut state.queue).into_iter().map(Some).collect();
        state.queue = order.iter().map(|&ix| old[ix].take().expect("a permutation visits each slot once")).collect();
        match scope {
            Scope::Others => {
                state.current = order.iter().position(|&ix| ix == state.current).unwrap_or(0);
                self.send(player::Cmd::Reorder { tracks: entries(&state.queue), current: state.current });
            }
            Scope::All => {
                state.current = 0;
                state.position = Duration::ZERO;
                self.send(player::Cmd::SetQueue { tracks: entries(&state.queue), start: 0, play: PlayState::Paused });
            }
        }
        drop(state);
        self.queue_changed();
    }

    fn queue_changed(&self) {
        self.save_playlist();
        self.save_player();
        self.note(|c| {
            c.queue = true;
            c.playback = true;
        });
    }

    // ----- Playback -----

    pub fn playback(&self) -> Playback {
        let state = self.state();
        let item = state.queue.get(state.current);
        let album = item.and_then(|i| state.albums.iter().find(|a| a.id == i.album_id)).map(AlbumRef::of);
        let cover_file = album
            .as_ref()
            .and_then(|a| a.cover)
            .map(|cover| library::cover_file(&self.paths.full_covers, cover, library::FULL_FORMAT))
            .filter(|path| path.exists());
        Playback {
            current: item.map(|_| state.current),
            playing: state.play_state == PlayState::Playing,
            position: state.position,
            length: state.length,
            repeat: state.repeat,
            title: item.map(|i| i.title.clone()).unwrap_or_default(),
            album,
            cover_file,
        }
    }

    pub fn jump_to(&self, ix: usize) {
        self.send(player::Cmd::JumpTo(ix));
    }

    pub fn set_playing(&self, playing: bool) {
        self.send(player::Cmd::SetPlayState(if playing { PlayState::Playing } else { PlayState::Paused }));
    }

    pub fn toggle_playing(&self) {
        self.send(player::Cmd::TogglePlayPause);
    }

    pub fn next(&self) {
        self.send(player::Cmd::Next);
    }

    pub fn prev(&self) {
        self.send(player::Cmd::Prev);
    }

    /// Seeks the playing track to `to`, showing it there at once rather than when the engine says.
    pub fn seek(&self, to: Duration) {
        let mut state = self.state();
        state.position = state.length.map_or(to, |len| to.min(len));
        state.pending_seek = Some(state.position);
        self.send(player::Cmd::Seek(state.position));
        drop(state);
        self.note(|c| c.playback = true);
    }

    pub fn set_repeat(&self, repeat: Repeat) {
        self.state().repeat = repeat;
        self.send(player::Cmd::SetRepeat(repeat));
        self.save_player();
        self.note(|c| c.playback = true);
    }

    fn on_engine(&self, event: player::Event) {
        let mut state = self.state();
        let mut track_changed = false;
        match event {
            player::Event::TrackStarted { ix, len } => {
                state.current = ix;
                state.length = len;
                state.position = Duration::ZERO;
                state.pending_seek = None;
                track_changed = true;
            }
            player::Event::Progress(pos) => {
                if state.pending_seek.is_some_and(|target| pos.abs_diff(target) > SEEK_SETTLE) {
                    return;
                }
                state.pending_seek = None;
                state.position = pos;
            }
            player::Event::PlayState(play_state) => state.play_state = play_state,
            player::Event::QueueEnded => {
                state.play_state = PlayState::Paused;
                state.position = state.length.unwrap_or_default();
            }
        }
        drop(state);
        if track_changed {
            self.save_player();
        }
        self.note(|c| c.playback = true);
    }

    // ----- The session -----

    fn save_playlist(&self) {
        let _saving = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        let tracks = self.state().queue.iter().map(|item| item.path.clone()).collect();
        smol::block_on(session::save_playlist(Some(self.paths.playlist.clone()), session::SavedPlaylist::new(tracks)));
    }

    fn save_player(&self) {
        let _saving = self.saving.lock().unwrap_or_else(|e| e.into_inner());
        let saved = {
            let state = self.state();
            session::SavedPlayer::new(state.current, state.repeat, state.sort)
        };
        smol::block_on(session::save_player(Some(self.paths.player.clone()), saved));
    }
}

/// `names` without the empty one, each once, in alphabetical order whatever their case.
fn sorted_names<'a>(names: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut names: Vec<String> = names.filter(|n| !n.is_empty()).cloned().collect();
    // Case only breaks ties, so that equal names end up next to each other for `dedup`.
    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b)));
    names.dedup();
    names
}

/// The tracks of `albums` as queue items, album by album.
fn items_of(library: &[Album], albums: &[u64]) -> Vec<QueueItem> {
    albums
        .iter()
        .filter_map(|id| library.iter().find(|a| a.id == *id))
        .flat_map(|album| {
            album.tracks.iter().map(|track| QueueItem {
                path: track.path.clone(),
                album_id: album.id,
                title: track.title.clone(),
            })
        })
        .collect()
}

/// The engine-facing form of queue items.
fn entries(items: &[QueueItem]) -> Vec<player::Entry> {
    items.iter().map(|item| player::Entry { path: item.path.clone(), album: item.album_id }).collect()
}

/// Fills in the queue's titles and album ids from the library, matched by path: how a queue
/// restored from paths alone gets its tags back, and keeps them as the library is rescanned.
fn hydrate(albums: &[Album], queue: &mut [QueueItem]) {
    if queue.is_empty() {
        return;
    }
    let mut by_path: HashMap<&Path, (u64, &str)> = HashMap::new();
    for album in albums {
        for track in &album.tracks {
            by_path.insert(track.path.as_path(), (album.id, track.title.as_str()));
        }
    }
    for item in queue.iter_mut() {
        if let Some(&(album_id, title)) = by_path.get(item.path.as_path()) {
            item.album_id = album_id;
            item.title = title.to_string();
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// A WAV of a tenth of a second of silence, tagged like a track of `album`.
    fn wav(title: &str, artist: &str, album: &str) -> Vec<u8> {
        fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut out = [&id[..], &(body.len() as u32).to_le_bytes(), body].concat();
            if body.len() % 2 == 1 {
                out.push(0);
            }
            out
        }
        let info = |id: &[u8; 4], value: &str| chunk(id, &[value.as_bytes(), b"\0"].concat());
        let fmt = [
            &1u16.to_le_bytes()[..],
            &2u16.to_le_bytes(),
            &48000u32.to_le_bytes(),
            &192000u32.to_le_bytes(),
            &4u16.to_le_bytes(),
            &16u16.to_le_bytes(),
        ]
        .concat();
        let list = [&b"INFO"[..], &info(b"INAM", title), &info(b"IART", artist), &info(b"IPRD", album)].concat();
        let riff = [&b"WAVE"[..], &chunk(b"fmt ", &fmt), &chunk(b"LIST", &list), &chunk(b"data", &[0; 4800 * 4])].concat();
        [&b"RIFF"[..], &(riff.len() as u32).to_le_bytes(), &riff].concat()
    }

    /// The queue outlives the album index, which sits among the caches Android may clear: a core that finds no index
    /// still restores every track it was left with.
    #[test]
    fn restores_the_queue_without_the_index() {
        let root = std::env::temp_dir().join(format!("phonoscule-android-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = Paths::new(&root.join("files"), &root.join("cache"));
        std::fs::create_dir_all(root.join("files")).unwrap();
        let tracks = vec![PathBuf::from("/A/First/1.wav"), PathBuf::from("/A/First/2.wav")];
        smol::block_on(session::save_playlist(Some(paths.playlist.clone()), session::SavedPlaylist::new(tracks.clone())));

        let core = Core::new(paths);
        let (queue, _) = core.queue();
        assert_eq!(queue.iter().map(|item| item.path.clone()).collect::<Vec<_>>(), tracks);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Waits out the scan that was just started.
    fn await_scan(core: &Core) {
        while core.scanning() {
            core.wait_notices();
        }
    }

    /// A library read through the SAF bridge comes out as albums, with a cover Kotlin can decode,
    /// and plays.
    #[test]
    fn scans_through_the_bridge_and_plays() {
        let root = std::env::temp_dir().join(format!("phonoscule-android-core-{}", std::process::id()));
        let music = root.join("music");
        std::fs::create_dir_all(music.join("A/First")).unwrap();
        std::fs::create_dir_all(music.join("B/Second")).unwrap();
        std::fs::write(music.join("A/First/1.wav"), wav("One", "A", "First")).unwrap();
        std::fs::write(music.join("A/First/2.wav"), wav("Two", "A", "First")).unwrap();
        std::fs::write(music.join("B/Second/1.wav"), wav("Uno", "B", "Second")).unwrap();
        image::RgbImage::from_pixel(32, 32, image::Rgb([200, 30, 30])).save(music.join("A/First/cover.png")).unwrap();

        let core: &'static Core = Box::leak(Box::new(Core::new(Paths::new(&root.join("files"), &root.join("cache")))));
        core.follow_engine();
        saf::test::serve(Arc::clone(&core.saf), music.clone());
        core.scan();
        await_scan(core);

        let albums = core.albums("", &[], &[]);
        let titles: Vec<&str> = albums.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, ["First", "Second"]);
        assert_eq!(core.albums("seco", &[], &[]).len(), 1, "search narrows by title");
        assert_eq!(core.albums("b", &[], &[])[0].title, "Second", "and by artist");
        // Neither has a genre tag, so both have the empty genre.
        assert!(core.albums("", &["Jazz".into()], &[]).is_empty(), "genres narrow");
        assert_eq!(core.albums("", &["Jazz".into(), String::new()], &[]).len(), 2, "to the albums of any of them");
        assert_eq!(core.artists(), ["A", "B"]);
        let by_b = core.albums("", &[], &["B".into()]);
        assert_eq!(by_b.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), ["Second"], "artists narrow");
        assert_eq!(core.albums("", &[], &["A".into(), "B".into()]).len(), 2, "to the albums of any of them");

        let first = &albums[0];
        assert!(first.accent.is_some(), "the cover's accent arrived with it");
        let cover = core.cover(first.cover_id.expect("a cover was found")).expect("a cover file");
        assert!(image::open(&cover).is_ok(), "the cover is a file anything can decode");

        // A folder that cannot be listed keeps its albums, where a folder that is gone loses them.
        use std::os::unix::fs::PermissionsExt;
        let second = music.join("B");
        std::fs::set_permissions(&second, std::fs::Permissions::from_mode(0o000)).unwrap();
        core.scan();
        await_scan(core);
        std::fs::set_permissions(&second, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(core.albums("", &[], &[]).len(), 2, "the unreadable folder's album is kept");
        std::fs::rename(&second, root.join("B-gone")).unwrap();
        core.scan();
        await_scan(core);
        assert_eq!(core.albums("", &[], &[]).len(), 1, "the missing folder's album is dropped");
        std::fs::rename(root.join("B-gone"), &second).unwrap();
        core.scan();
        await_scan(core);

        core.play(&[first.id, albums[1].id], 1);
        let (queue, runs) = core.queue();
        assert_eq!(queue.len(), 3);
        assert_eq!(core.playback().current, Some(1));
        assert_eq!(runs.iter().map(|r| r.start..r.end).collect::<Vec<_>>(), [0..2, 2..3]);
        assert_eq!(core.playback().title, "Two");

        std::fs::remove_dir_all(&root).unwrap();
    }
}
