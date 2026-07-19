//! Music library scanning: find audio files, read their tags with phonoscule, group them into
//! albums, and load folder cover art (`cover.jpg` & friends).
//!
//! Albums are what the tags say, not what the directory layout says: tracks group by (album
//! artist, album title) -- the ALBUMARTIST tag, or the track's own artist without one -- wherever
//! their files live in the pool. So a multi-disc album split over `CD1`/`CD2` directories is one
//! album, and two same-named albums by different artists are two even side by side in one
//! directory. The flip side is deliberate: a compilation without ALBUMARTIST tags fragments into
//! per-artist albums -- maintaining that tag is the library's job, not ours to guess from paths.
//!
//! [`scan`] streams results incrementally: an album is (re-)reported as directories contribute
//! tracks to it, growing until the scan completes, and cover art (the expensive part) trickles in
//! afterwards, decoded concurrently. On a warm rescan the tag cache predicts every directory an
//! album draws from, so each album is reported exactly once, fully assembled (see `Assembler`).
//!
//! Tags are cached persistently (validated by file mtime + size), so only new or changed files
//! are actually opened; re-scans of an unchanged library cost directory reads and stats. Album
//! and cover ids are stable content-derived hashes, so consumers can reconcile a re-scan against
//! previous state: upsert albums by id as they arrive, keep already-loaded cover art when the
//! cover id is unchanged (pass it in [`ScanOptions::known_covers`] to skip its decoding
//! entirely), and finally retain only [`ScanEvent::Done::album_ids`].

use embedded_io_adapters::futures_03::FromFutures;
use embedded_io_async::{Read as _, Seek as _, SeekFrom};
use futures::{StreamExt, stream};
use phonoscule::{io::Skippable, metadata::Tag, opus, wav::Wav};
use serde::{Deserialize, Serialize};
use smol::{channel, fs::File, io::BufReader, stream::Stream};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    future::Future,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

#[derive(Debug, Clone)]
pub struct Album {
    /// Stable content-derived id (album artist + album title), the same across re-scans -- and
    /// across the user reorganizing where the files live.
    pub id: u64,
    pub title: String,
    pub artist: String,
    /// Empty when no track of the album carries a genre tag.
    pub genre: String,
    /// The id the cover art for this album has (or would have, when not loaded yet).
    pub cover_id: Option<u64>,
    pub cover: Option<CoverArt>,
    /// The cover's accent color, known before (and independently of) the cover pixels: persisted
    /// in the album index, so freshly launched fallback tiles can already carry it.
    pub accent: Option<iced::Color>,
    pub tracks: Vec<TrackInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackInfo {
    pub path: PathBuf,
    pub title: String,
}

/// Decoded cover art, shared between the browser view (via the iced image handle) and the Cover
/// Flow (via the raw pixels, uploaded to a GPU texture cached by `id`).
#[derive(Clone)]
pub struct CoverArt {
    /// Stable content-derived id (image file path + mtime).
    pub id: u64,
    /// The (absolute) image file this was decoded from, e.g. for pointing other programs at it
    /// and (later) decoding a higher-resolution version on demand.
    pub file: Arc<PathBuf>,
    /// The thumbnail: [`THUMB`]²  RGBA, as an `Rgba` handle. The pixels live here (in the handle's
    /// shared, ref-counted buffer), so this is the only in-memory copy -- the grid renders the
    /// handle directly, and the cover flow reads the pixels back out of its `Rgba` variant.
    pub handle: iced::widget::image::Handle,
    /// The cover's most distinct color, e.g. for theming the surroundings after it.
    pub accent: iced::Color,
}

impl fmt::Debug for CoverArt {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("CoverArt").field("id", &self.id).field("file", &self.file).finish()
    }
}

#[derive(Debug, Clone)]
pub enum ScanEvent {
    /// A discovered album, in its assembled-so-far state: consumers upsert by id. Usually
    /// reported once and complete, but an album drawing tracks from directories the tag cache
    /// didn't predict grows across reports until [`ScanEvent::Done`]. Its cover art may still be
    /// loading (or, when its cover id was in [`ScanOptions::known_covers`], arrive not at all:
    /// keep what you have). Boxed: an album is by far the largest event, and it travels through
    /// every message channel.
    Album(Box<Album>),
    /// Cover art finished loading for the albums with the given ids. Apply it only where the
    /// album's current `cover_id` still matches [`CoverArt::id`]: an album can outgrow a queued
    /// cover mid-scan (a later directory contributed more of its tracks), and the stale decode
    /// must not overwrite the winner.
    Cover { albums: Vec<u64>, art: CoverArt },
    /// The scan is complete: every album has been reported. Albums absent from `album_ids` no
    /// longer exist and should be dropped.
    Done { album_ids: Vec<u64> },
}

pub struct ScanOptions {
    pub root: PathBuf,
    /// Album ids whose directories are scanned (and thus whose covers load) before everything
    /// else, in this order -- e.g. the restored queue's albums, circularly outward from the
    /// playing one, so the cover flow dresses up first.
    pub priority: Vec<u64>,
    /// Ids of cover art the consumer already has: decoding (and [`ScanEvent::Cover`]) is skipped
    /// for these.
    pub known_covers: HashSet<u64>,
    /// Where tags are cached between scans. `None` disables persistence.
    pub cache_file: Option<PathBuf>,
    /// Directory holding the raw decoded thumbnails, keyed by cover id. Reading one back is a
    /// plain file read -- no image decoding -- so warm launches are fast even in debug builds.
    /// `None` disables the cache (always decode from source).
    pub covers_dir: Option<PathBuf>,
}

/// The default location of the tag cache: `<cache>/phonoscule/library.json`.
pub fn default_cache_file() -> Option<PathBuf> {
    Some(cache_dir()?.join("library.json"))
}

/// The default thumbnail cache directory: `<cache>/phonoscule/covers.<THUMB>`. The edge size is
/// in the name, so bumping [`THUMB`] starts a fresh directory rather than reading mismatched files.
pub fn default_covers_dir() -> Option<PathBuf> {
    Some(cache_dir()?.join(format!("covers.{THUMB}")))
}

/// The default location of the album index: `<cache>/phonoscule/albums.json`.
pub fn default_index_file() -> Option<PathBuf> {
    Some(cache_dir()?.join("albums.json"))
}

/// Bumped when [`SavedAlbum`] changes shape or meaning (like the id derivation); an old or
/// unreadable index just means the grid stays empty until the scan streams the albums in, like
/// before the index existed.
const INDEX_VERSION: u32 = 3;

/// The persisted album index: the assembled album list minus the cover pixels, so a launch can
/// show the whole library instantly instead of waiting for the directory walk. The boot scan then
/// reconciles it exactly like a rescan (upsert by id, retain what `Done` reports), so staleness
/// self-heals; covers stream in from the thumbnail cache as always.
#[derive(Serialize, Deserialize)]
struct AlbumIndex {
    version: u32,
    albums: Vec<SavedAlbum>,
}

/// [`Album`] minus the runtime-only cover art.
#[derive(Serialize, Deserialize)]
struct SavedAlbum {
    id: u64,
    title: String,
    artist: String,
    genre: String,
    cover_id: Option<u64>,
    /// The cover's accent color as linear RGB components.
    accent: Option<[f32; 3]>,
    tracks: Vec<TrackInfo>,
}

/// Loads the album index; a missing, outdated, or unreadable one is an empty library (the scan
/// rebuilds and re-saves it).
pub async fn load_index(path: Option<PathBuf>) -> Vec<Album> {
    let Some(path) = path else { return vec![] };
    let src = match smol::fs::read_to_string(&path).await {
        Ok(src) => src,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return vec![],
        Err(e) => {
            log::warn!("could not read the album index at {path:?}: {e}");
            return vec![];
        }
    };
    let index = match serde_json::from_str::<AlbumIndex>(&src) {
        Ok(index) if index.version == INDEX_VERSION => index,
        Ok(_) => return vec![],
        Err(e) => {
            log::warn!("discarding an unreadable album index at {path:?}: {e}");
            return vec![];
        }
    };
    index
        .albums
        .into_iter()
        .map(|a| Album {
            id: a.id,
            title: a.title,
            artist: a.artist,
            genre: a.genre,
            cover_id: a.cover_id,
            cover: None,
            accent: a.accent.map(|[r, g, b]| iced::Color { r, g, b, a: 1.0 }),
            tracks: a.tracks,
        })
        .collect()
}

/// Saves the album index (snapshotting `albums` eagerly, so the write can run detached).
/// Best-effort atomic, like the tag cache; failure only costs the next launch its instant grid.
pub fn save_index(path: Option<PathBuf>, albums: &[Album]) -> impl Future<Output = ()> + Send + 'static {
    let index = AlbumIndex {
        version: INDEX_VERSION,
        albums: albums
            .iter()
            .map(|a| SavedAlbum {
                id: a.id,
                title: a.title.clone(),
                artist: a.artist.clone(),
                genre: a.genre.clone(),
                cover_id: a.cover_id,
                accent: a.accent.map(|c| [c.r, c.g, c.b]),
                tracks: a.tracks.clone(),
            })
            .collect(),
    };
    async move {
        let Some(path) = path else { return };
        let write = async {
            if let Some(dir) = path.parent() {
                smol::fs::create_dir_all(dir).await?;
            }
            let json = serde_json::to_string(&index).map_err(std::io::Error::other)?;
            let tmp = path.with_extension("json.partial");
            smol::fs::write(&tmp, json).await?;
            smol::fs::rename(&tmp, &path).await
        };
        if let Err(e) = write.await {
            log::warn!("could not write the album index to {path:?}: {e}");
        }
    }
}

/// Our cache directory, gathering every persistent cache under one roof:
/// `$XDG_CACHE_HOME/phonoscule`, falling back to `~/.cache/phonoscule`.
fn cache_dir() -> Option<PathBuf> {
    let home = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(std::env::home_dir()?.join(".cache")))?;
    Some(home.join("phonoscule"))
}

/// Cover thumbnails are downscaled to fit this square (center-cropped, like the iPod did). Sized
/// for the library grid; the now-playing view decodes a higher-resolution version on demand. Also
/// the LOD placeholder the cover flow shows until full-res arrives. Deliberately trades a bit of
/// full-screen sharpening subtlety for faster cover loading -- at launch, every thumbnail is read
/// from disk, and this squares into that bill.
pub const THUMB: u32 = 320;

/// The higher-resolution edge the now-playing cover flow decodes on demand (see [`full_res`]), for
/// the focused covers when the window is run full-screen. Short of a true 4K-panel edge on
/// purpose: it halves the per-cover memory and decode time versus 1024² while staying crisp enough
/// that the difference isn't visible at the sizes the flow actually draws.
pub const FULL: u32 = 900;

/// Decodes a cover to [`FULL`]²  RGBA (~3 MiB), for the now-playing view. Decoded on demand around
/// the current track and handed to the global high-res cache, which retains a bounded, LRU-managed
/// set of them -- so it needn't scale with the library. Its own bitmap is an `Arc<[u8]>` (never
/// shared with iced, unlike the thumbnails), so the cache and the cover flow's GPU upload reference
/// the same allocation rather than copying.
pub async fn full_res(file: PathBuf) -> Option<Arc<[u8]>> {
    smol::unblock(move || match image::open(&file) {
        Ok(img) => {
            let rgba = img.resize_to_fill(FULL, FULL, image::imageops::FilterType::Triangle).into_rgba8().into_raw();
            Some(Arc::<[u8]>::from(rgba))
        }
        Err(e) => {
            log::warn!("could not decode cover {file:?}: {e}");
            None
        }
    })
    .await
}

/// Scans `root`, streaming results as they are found. The stream ends after [`ScanEvent::Done`]
/// (or early, if the scan task fails); dropping it cancels the scan.
pub fn scan(options: ScanOptions) -> impl Stream<Item = ScanEvent> + Send {
    let (tx, rx) = channel::bounded(64);
    // `drive` fans two concurrent phases into `tx`. Rather than spawn it, fold it into the
    // returned stream: a non-yielding "driver" (running `drive` to completion) selected with the
    // receiver. iced then drives the whole scan on its own executor whenever it polls this stream
    // (via `Task::run`) -- no task or thread of ours -- and dropping the stream cancels the scan.
    let driver = stream::once(drive(options, tx)).filter_map(|()| async { None::<ScanEvent> }).boxed();
    stream::select(driver, rx)
}

/// How many directories have their tags read, and how many covers are decoded, at once.
fn concurrency() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// One tag cache entry; valid for a file as long as its mtime and size still match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CacheEntry {
    mtime: SystemTime,
    size: u64,
    title: String,
    artist: String,
    album: String,
    /// Empty when the file carries no ALBUMARTIST tag.
    album_artist: String,
    /// Empty when the file carries no genre tag.
    genre: String,
    /// The track's position within its album, when tagged (parsed leniently -- see [`number`]).
    track: Option<u32>,
    /// The disc the track belongs to on a multi-disc album, when tagged.
    disc: Option<u32>,
}

/// The identity a track's album groups by -- and the album's displayed byline: `(artist, album
/// title)`, where the artist is the ALBUMARTIST tag when present, else the track's own artist.
/// The directory plays no part: same key means same album wherever the files live.
fn album_key(entry: &CacheEntry) -> (&str, &str) {
    let artist = match entry.album_artist.as_str() {
        "" => entry.artist.as_str(),
        a => a,
    };
    (artist, entry.album.as_str())
}

/// The stable album id: [`album_key`], hashed.
fn album_id(entry: &CacheEntry) -> u64 {
    stable_id(album_key(entry))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    version: u32,
    files: HashMap<PathBuf, CacheEntry>,
}

const CACHE_VERSION: u32 = 5;

/// A stable hash-based identity for albums and covers.
fn stable_id(parts: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    parts.hash(&mut hasher);
    hasher.finish()
}

/// An audio file found during the walk, with the stat data that decides cache validity.
struct AudioFile {
    path: PathBuf,
    mtime: SystemTime,
    size: u64,
}

/// A directory's worth of scanning work. The cover candidate is harvested into the
/// [`Assembler`]'s per-directory cover map before the tag-reading phase.
struct DirJob {
    dir: PathBuf,
    files: Vec<AudioFile>,
    cover: Option<(PathBuf, SystemTime)>,
}

/// A track awaiting its album's emission.
struct PendingTrack {
    disc: Option<u32>,
    track: Option<u32>,
    /// The track's own genre tag (empty when untagged); the album shows its first one.
    genre: String,
    info: TrackInfo,
}

impl PendingTrack {
    /// Where the track sorts within its album: by disc, then track number, then title -- the
    /// title (not the path) breaks ties so an untagged album orders the same wherever its files
    /// live. Untagged discs are disc 1; untagged tracks sort after their disc's tagged ones.
    fn order(&self) -> (u32, u32, &str, &Path) {
        (self.disc.unwrap_or(1), self.track.unwrap_or(u32::MAX), &self.info.title, &self.info.path)
    }
}

/// An album being assembled from the tracks the walked directories contribute.
struct PendingAlbum {
    title: String,
    artist: String,
    tracks: Vec<PendingTrack>,
    /// How many of the album's tracks each directory holds, for choosing its cover.
    contributions: HashMap<PathBuf, usize>,
}

/// Cross-directory album assembly: albums grow as directories contribute tracks and are emitted,
/// possibly repeatedly in their assembled-so-far state, as they become ready.
///
/// Readiness is about rescan hygiene: the tag cache predicts, per directory, which albums its
/// files belong to, so an album is held back until every directory the cache expects of it has
/// been read. On a warm rescan the prediction is complete and each album emits exactly once,
/// identical to the consumer's state -- no churn, no index rewrite every five minutes. Files the
/// cache doesn't know (new or changed) predict nothing, so a cold scan emits albums growing per
/// contribution and the first launch stays progressive.
struct Assembler {
    assembled: HashMap<u64, PendingAlbum>,
    /// Per album, how many directories the tag cache expects to contribute are still unread.
    waiting: HashMap<u64, usize>,
    /// Per unread directory, the albums the tag cache expects it to contribute to (`waiting`'s
    /// feeder: drained -- decrementing the counts -- as directories complete).
    expected: HashMap<PathBuf, Vec<u64>>,
    /// Albums touched since they were last emitted.
    dirty: HashSet<u64>,
    /// `(cover id, album id)` pairs already queued for decoding, so a re-emission with an
    /// unchanged choice doesn't decode the cover again.
    queued: HashSet<(u64, u64)>,
    /// Every walked directory's cover image candidate -- including directories without audio
    /// files, whose covers may dress the albums in their subdirectories (see [`choose_cover`]).
    covers: HashMap<PathBuf, (PathBuf, SystemTime)>,
    /// Covers the consumer already holds decoded: never queued.
    known_covers: HashSet<u64>,
}

impl Assembler {
    fn new(
        jobs: &[DirJob],
        cache: &Cache,
        covers: HashMap<PathBuf, (PathBuf, SystemTime)>,
        known_covers: HashSet<u64>,
    ) -> Self {
        let mut waiting: HashMap<u64, usize> = HashMap::new();
        let mut expected: HashMap<PathBuf, Vec<u64>> = HashMap::new();
        for job in jobs {
            let mut ids: Vec<u64> = job
                .files
                .iter()
                .filter_map(|f| cache.files.get(&f.path).filter(|e| e.mtime == f.mtime && e.size == f.size))
                .map(album_id)
                .collect();
            ids.sort_unstable();
            ids.dedup();
            for &id in &ids {
                *waiting.entry(id).or_default() += 1;
            }
            if !ids.is_empty() {
                expected.insert(job.dir.clone(), ids);
            }
        }
        Assembler {
            assembled: HashMap::new(),
            waiting,
            expected,
            dirty: HashSet::new(),
            queued: HashSet::new(),
            covers,
            known_covers,
        }
    }

    /// Takes in one read directory's worth of resolved tags: grows the albums its files belong
    /// to and retires the directory from every album's expectations.
    fn absorb(&mut self, dir: &Path, entries: &[(PathBuf, CacheEntry)]) {
        for (path, entry) in entries {
            let id = album_id(entry);
            let (artist, album) = album_key(entry);
            let pending = self.assembled.entry(id).or_insert_with(|| PendingAlbum {
                title: album.to_string(),
                artist: artist.to_string(),
                tracks: vec![],
                contributions: HashMap::new(),
            });
            pending.tracks.push(PendingTrack {
                disc: entry.disc,
                track: entry.track,
                genre: entry.genre.clone(),
                info: TrackInfo { path: path.clone(), title: entry.title.clone() },
            });
            *pending.contributions.entry(dir.to_path_buf()).or_default() += 1;
            self.dirty.insert(id);
        }
        for id in self.expected.remove(dir).unwrap_or_default() {
            if let Some(n) = self.waiting.get_mut(&id) {
                *n = n.saturating_sub(1);
            }
        }
    }

    /// Drains and returns the touched albums that are ready to emit: all their expected
    /// directories have been read. Sorted for a deterministic emission order.
    fn ready(&mut self) -> Vec<u64> {
        let waiting = &self.waiting;
        let mut ready: Vec<u64> = self.dirty.iter().copied().filter(|id| waiting.get(id).is_none_or(|&n| n == 0)).collect();
        ready.sort_unstable();
        for id in &ready {
            self.dirty.remove(id);
        }
        ready
    }

    /// Drains and returns every still-unemitted album, for the end of the tag phase (an album
    /// expecting a directory that failed to read, or whose count a stale prediction overshot).
    fn flush(&mut self) -> Vec<u64> {
        let mut rest: Vec<u64> = self.dirty.drain().collect();
        rest.sort_unstable();
        rest
    }

    /// The album's current assembled state, plus -- when its chosen cover still needs decoding --
    /// the cover to queue, as `(cover id, image path, mtime)`.
    fn snapshot(&mut self, id: u64) -> (Album, Option<(u64, PathBuf, SystemTime)>) {
        let pending = &self.assembled[&id];
        let mut tracks: Vec<&PendingTrack> = pending.tracks.iter().collect();
        tracks.sort_unstable_by(|a, b| a.order().cmp(&b.order()));
        // Genre is per-album: the first track (in album order) carrying one names the album's.
        let genre = tracks.iter().map(|t| &t.genre).find(|g| !g.is_empty()).cloned().unwrap_or_default();
        let cover = choose_cover(&pending.contributions, &self.covers);
        let cover_id = cover.map(|(path, mtime)| stable_id((path, mtime)));
        let queue = match (cover, cover_id) {
            (Some((path, mtime)), Some(cid)) if !self.known_covers.contains(&cid) && self.queued.insert((cid, id)) => {
                Some((cid, path.clone(), *mtime))
            }
            _ => None,
        };
        let album = Album {
            id,
            title: pending.title.clone(),
            artist: pending.artist.clone(),
            genre,
            cover_id,
            cover: None,
            accent: None,
            tracks: tracks.into_iter().map(|t| t.info.clone()).collect(),
        };
        (album, queue)
    }
}

/// Picks an album's cover from the directories its tracks live in: the cover of the contributing
/// directory holding most of its tracks (the lexicographically first directory breaks ties, for
/// determinism across scan orders). When no contributing directory has a cover, their parents are
/// tried the same way -- the disc-per-directory layout keeps the cover beside the disc
/// directories, in the album's own.
fn choose_cover<'c>(
    contributions: &HashMap<PathBuf, usize>,
    covers: &'c HashMap<PathBuf, (PathBuf, SystemTime)>,
) -> Option<&'c (PathBuf, SystemTime)> {
    let best = |cover_of: &dyn Fn(&Path) -> Option<&'c (PathBuf, SystemTime)>| {
        contributions
            .iter()
            .filter_map(|(dir, &n)| cover_of(dir).map(|cover| (n, dir, cover)))
            .max_by(|(n1, d1, _), (n2, d2, _)| n1.cmp(n2).then_with(|| d2.cmp(d1)))
            .map(|(_, _, cover)| cover)
    };
    best(&|dir| covers.get(dir)).or_else(|| best(&|dir| covers.get(dir.parent()?)))
}

/// Emits the given ready albums, queueing their covers (batched by cover, so a pooled directory's
/// shared cover decodes once for all its albums). Albums go out before their covers, so a Cover
/// event can never reach the consumer ahead of the album it belongs to. Returns `false` when the
/// scan was cancelled (the receiver is gone).
async fn emit_albums(
    asm: &mut Assembler,
    ids: Vec<u64>,
    tx: &channel::Sender<ScanEvent>,
    cover_tx: &channel::Sender<(Vec<u64>, PathBuf, SystemTime)>,
) -> bool {
    let mut batch: HashMap<u64, (Vec<u64>, PathBuf, SystemTime)> = HashMap::new();
    for id in ids {
        let (album, cover) = asm.snapshot(id);
        if tx.send(ScanEvent::Album(Box::new(album))).await.is_err() {
            return false;
        }
        if let Some((cid, path, mtime)) = cover {
            batch.entry(cid).or_insert_with(|| (Vec::new(), path, mtime)).0.push(id);
        }
    }
    for (ids, path, mtime) in batch.into_values() {
        if cover_tx.send((ids, path, mtime)).await.is_err() {
            return false;
        }
    }
    true
}

async fn drive(options: ScanOptions, tx: channel::Sender<ScanEvent>) {
    log::info!("scanning {:?}", options.root);
    let cache = match &options.cache_file {
        Some(path) => load_cache(path).await,
        None => Cache::default(),
    };

    // Phase 1: walk the tree, collecting each directory's audio files (with stat data) and its
    // cover image candidate in a single directory listing. Covers are kept for every directory,
    // even file-less ones -- a parent directory's cover can dress the albums below it.
    let mut dirs = vec![options.root];
    let mut jobs = Vec::new();
    let mut covers = HashMap::new();
    while let Some(dir) = dirs.pop() {
        let Some(mut job) = dir_job(&dir, &mut dirs).await else { continue };
        if let Some(cover) = job.cover.take() {
            covers.insert(job.dir.clone(), cover);
        }
        if !job.files.is_empty() {
            jobs.push(job);
        }
    }

    // Order the jobs so covers stream in usefully: prioritized albums first (in their given
    // order), then the way the grid sorts (artist, then album title) so the visible top of an
    // unscrolled library fills next. The keys -- including each directory's album id, for the
    // priority lookup -- come from the tag cache, already loaded, no file reads; new directories
    // (a cache miss) fall back to their name, which usually approximates the artist anyway.
    let rank: HashMap<u64, usize> = options.priority.iter().enumerate().map(|(rank, &id)| (id, rank)).collect();
    jobs.sort_by_cached_key(|job| {
        job.files.first().and_then(|file| cache.files.get(&file.path)).map_or_else(
            || (usize::MAX, job.dir.file_name().unwrap_or_default().to_string_lossy().to_lowercase(), String::new()),
            |entry| {
                let (artist, album) = album_key(entry);
                (rank.get(&album_id(entry)).copied().unwrap_or(usize::MAX), artist.to_lowercase(), album.to_lowercase())
            },
        )
    });

    let mut asm = Assembler::new(&jobs, &cache, covers, options.known_covers);

    // Phases 2 & 3 run concurrently: tag reading grows the albums per directory, emitting the
    // ready ones and queueing their covers; cover decoding streams in whenever ready. Sends only
    // fail when the receiver is gone (scan cancelled), which also cancels these phases via
    // `return`.
    let (cover_tx, cover_rx) = channel::bounded::<(Vec<u64>, PathBuf, SystemTime)>(64);
    let mut fresh = HashMap::new();
    let mut n_parsed = 0usize;

    let cache = &cache;
    let read_tags_phase = async {
        let mut per_dir = futures::stream::iter(jobs)
            .map(|job| async move {
                let read = read_dir_tags(&job, cache).await;
                (job, read)
            })
            .buffer_unordered(concurrency());
        while let Some((job, (entries, parsed))) = per_dir.next().await {
            n_parsed += parsed;
            asm.absorb(&job.dir, &entries);
            fresh.extend(entries);
            let ready = asm.ready();
            if !emit_albums(&mut asm, ready, &tx, &cover_tx).await {
                return;
            }
        }
        let rest = asm.flush();
        if !emit_albums(&mut asm, rest, &tx, &cover_tx).await {
            return;
        }
        drop(cover_tx); // lets the cover phase finish
    };

    // Best-effort: make the thumbnail cache directory once, up front.
    if let Some(dir) = &options.covers_dir {
        let _ = smol::fs::create_dir_all(dir).await;
    }
    let covers_dir = options.covers_dir.as_deref();
    let covers_phase = async {
        // Pinned on the stack: the channel receiver (hence the whole chain) is not `Unpin`.
        let mut covers = std::pin::pin!(
            cover_rx
                .map(|(ids, path, mtime)| {
                    let id = stable_id((&path, mtime));
                    async move { (ids, id, load_cover(path, covers_dir, id).await) }
                })
                .buffer_unordered(concurrency())
        );
        while let Some((ids, id, cover)) = covers.next().await {
            let Some((file, rgba, accent)) = cover else { continue };
            let handle = iced::widget::image::Handle::from_rgba(THUMB, THUMB, rgba);
            let art = CoverArt { id, file: Arc::new(file), handle, accent };
            if tx.send(ScanEvent::Cover { albums: ids, art }).await.is_err() {
                return;
            }
        }
    };

    futures::join!(read_tags_phase, covers_phase);
    let album_ids: Vec<u64> = asm.assembled.keys().copied().collect();
    log::info!("scan done: found {} albums ({n_parsed} files (re)parsed)", album_ids.len());

    if let Some(path) = &options.cache_file
        && (n_parsed > 0 || fresh.len() != cache.files.len())
    {
        save_cache(path, &Cache { version: CACHE_VERSION, files: fresh }).await;
    }
    let _ = tx.send(ScanEvent::Done { album_ids }).await;
}

/// Lists one directory: collects audio files with their stat data, spots a cover image, and
/// pushes subdirectories onto `dirs`.
async fn dir_job(dir: &Path, dirs: &mut Vec<PathBuf>) -> Option<DirJob> {
    const COVER_STEMS: [&str; 4] = ["cover", "folder", "front", "albumart"];
    const COVER_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

    let mut entries = match smol::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) => {
            log::warn!("could not read directory {dir:?}: {e}");
            return None;
        }
    };
    let mut job = DirJob { dir: dir.to_path_buf(), files: Vec::new(), cover: None };
    while let Some(Ok(entry)) = entries.next().await {
        let path = entry.path();
        let Ok(file_type) = entry.file_type().await else { continue };
        if file_type.is_dir() {
            dirs.push(path);
            continue;
        }
        let ext = extension(&path).unwrap_or_default();
        if matches!(ext.as_str(), "wav" | "opus") {
            let Ok(meta) = entry.metadata().await else { continue };
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            job.files.push(AudioFile { path, mtime, size: meta.len() });
        } else if job.cover.is_none()
            && COVER_EXTS.contains(&ext.as_str())
            && COVER_STEMS.contains(&path.file_stem().unwrap_or_default().to_string_lossy().to_lowercase().as_str())
        {
            let Ok(meta) = entry.metadata().await else { continue };
            job.cover = Some((path, meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
        }
    }
    job.files.sort_by(|a, b| a.path.cmp(&b.path));
    Some(job)
}

/// Reads the tags of each of the directory's files, from the cache when it is still valid.
/// Returns the resolved entries (fallbacks applied) and how many files were actually parsed;
/// grouping into albums is the [`Assembler`]'s job.
async fn read_dir_tags(job: &DirJob, cache: &Cache) -> (Vec<(PathBuf, CacheEntry)>, usize) {
    let mut entries = Vec::with_capacity(job.files.len());
    let mut n_parsed = 0usize;

    for file in &job.files {
        let cached = cache.files.get(&file.path).filter(|e| e.mtime == file.mtime && e.size == file.size);
        let entry = match cached {
            Some(entry) => entry.clone(),
            None => {
                n_parsed += 1;
                let Some(tags) = read_tags(&file.path).await else {
                    log::warn!("could not parse {:?}", file.path);
                    continue;
                };
                // Cache the resolved values, fallbacks applied. An untagged album is "Singles":
                // an artist's loose tracks pool into one album of theirs, wherever the files sit.
                CacheEntry {
                    mtime: file.mtime,
                    size: file.size,
                    title: match tags.title.as_str() {
                        "" => file.path.file_stem().unwrap_or_default().to_string_lossy().to_string(),
                        _ => tags.title,
                    },
                    artist: match tags.artist.as_str() {
                        "" => "Unknown Artist".to_string(),
                        _ => tags.artist,
                    },
                    album: match tags.album.as_str() {
                        "" => "Singles".to_string(),
                        _ => tags.album,
                    },
                    album_artist: tags.album_artist,
                    genre: tags.genre,
                    track: tags.track,
                    disc: tags.disc,
                }
            }
        };
        entries.push((file.path.clone(), entry));
    }
    (entries, n_parsed)
}

fn extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_string_lossy().to_lowercase())
}

/// A file's tags, collected from the parser's [`Tag`] pushes. A repeated tag overwrites: the last
/// occurrence wins, matching most players' reading of multi-value vorbis comments.
#[derive(Default)]
struct FileTags {
    title: String,
    artist: String,
    album: String,
    album_artist: String,
    genre: String,
    track: Option<u32>,
    disc: Option<u32>,
}

impl FileTags {
    fn set(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Title(s) => s.clone_into(&mut self.title),
            Tag::Artist(s) => s.clone_into(&mut self.artist),
            Tag::Album(s) => s.clone_into(&mut self.album),
            Tag::AlbumArtist(s) => s.clone_into(&mut self.album_artist),
            Tag::Genre(s) => s.clone_into(&mut self.genre),
            Tag::TrackNumber(s) => self.track = number(s),
            Tag::DiscNumber(s) => self.disc = number(s),
        }
    }
}

/// Parses a track or disc number leniently: its leading digits, so the "3/12" (position of total)
/// values some taggers write read as 3. No digits (or an overflowing count) is no number.
fn number(s: &str) -> Option<u32> {
    let digits = &s[..s.bytes().take_while(u8::is_ascii_digit).count()];
    digits.parse().ok()
}

async fn read_tags(path: &Path) -> Option<FileTags> {
    let mut f = Skippable(FromFutures::new(BufReader::new(File::open(path).await.ok()?)));
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).await.ok()?;
    f.seek(SeekFrom::Start(0)).await.ok()?;
    let mut tags = FileTags::default();
    match &magic {
        b"RIFF" => {
            Wav::parse(f, |tag| tags.set(tag)).await?;
        }
        b"OggS" => {
            opus::Headers::parse(&mut f, |tag| tags.set(tag)).await?;
        }
        _ => return None,
    }
    Some(tags)
}

/// Number of bytes in a cached thumbnail: [`THUMB`]²  RGB.
const THUMB_RGB_LEN: usize = (THUMB * THUMB * 3) as usize;

/// Loads a cover thumbnail as [`THUMB`]²  RGBA, plus its accent color and absolute path. Reads the
/// raw cached thumbnail when present -- a plain file read, no image decoding, so this is fast even
/// in debug builds. Otherwise decodes and downscales the source on the blocking pool (parallel
/// regardless of executor threads) and caches the result for next time.
async fn load_cover(path: PathBuf, covers_dir: Option<&Path>, id: u64) -> Option<(PathBuf, Vec<u8>, iced::Color)> {
    // Absolute, so consumers (e.g. the MPRIS art URL) don't depend on our working directory.
    let file = smol::fs::canonicalize(path).await.ok()?;
    let cache_path = covers_dir.map(|dir| dir.join(format!("{id:016x}")));

    if let Some(cache_path) = &cache_path
        && let Ok(rgb) = smol::fs::read(cache_path).await
        && rgb.len() == THUMB_RGB_LEN
    {
        let accent = accent_color(&rgb);
        return Some((file, rgb_to_rgba(&rgb), accent));
    }

    let decode_file = file.clone();
    let rgb = smol::unblock(move || decode_thumbnail(&decode_file)).await?;
    if let Some(cache_path) = &cache_path
        && let Err(e) = smol::fs::write(cache_path, &rgb).await
    {
        // Best-effort: a failed write just means we decode again next launch.
        log::warn!("could not cache thumbnail {cache_path:?}: {e}");
    }
    let accent = accent_color(&rgb);
    Some((file, rgb_to_rgba(&rgb), accent))
}

/// Decodes an image file and downscales it to [`THUMB`]²  RGB, center-cropped to a square.
fn decode_thumbnail(file: &Path) -> Option<Vec<u8>> {
    match image::open(file) {
        Ok(img) => Some(img.resize_to_fill(THUMB, THUMB, image::imageops::FilterType::Triangle).into_rgb8().into_raw()),
        Err(e) => {
            log::warn!("could not decode cover {file:?}: {e}");
            None
        }
    }
}

/// Expands packed RGB triplets to the RGBA quartets iced and wgpu want (fully opaque).
fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
    for px in rgb.chunks_exact(3) {
        rgba.extend_from_slice(px);
        rgba.push(u8::MAX);
    }
    rgba
}

/// Picks the image's most distinct color: dominant among saturated, reasonably bright pixels,
/// falling back towards the most common color for near-grayscale images.
pub fn accent_color(rgb: &[u8]) -> iced::Color {
    // Histogram over a coarsely quantized (4 bits per channel) color space, accumulating exact
    // sums per bucket so the winner keeps its true shade.
    let mut buckets = vec![[0u64; 4]; 16 * 16 * 16];
    for px in rgb.chunks_exact(3).step_by(7) {
        let (r, g, b) = (px[0] as u64, px[1] as u64, px[2] as u64);
        let bucket = &mut buckets[((r >> 4 << 8) | (g >> 4 << 4) | (b >> 4)) as usize];
        *bucket = [bucket[0] + 1, bucket[1] + r, bucket[2] + g, bucket[3] + b];
    }
    let samples = (rgb.len() / 3).div_ceil(7) as u64;
    let score = |&[n, r, g, b]: &[u64; 4]| {
        if n == 0 {
            return 0.0;
        }
        let (r, g, b) = ((r / n) as f32 / 255.0, (g / n) as f32 / 255.0, (b / n) as f32 / 255.0);
        let max = r.max(g).max(b);
        let chroma = max - r.min(g).min(b);
        // Vividness must dominate size -- a small vivid region (a logo, a lit screen) IS the
        // accent of a mostly-dark cover, so the population floor is tiny: a tiebreaker letting
        // grayscale art degrade to a representative shade, never a rival to real color. Absolute
        // chroma, not a max-relative ratio, so near-black masses can't ride their color cast. A
        // vivid bucket must still cover ~0.1% of the art: JPEG noise on dark covers yields lone
        // max-chroma pixels. The tiebreaker itself is brightness-weighted: on achromatic art a
        // bright shade beats a dark mass unless the dark is overwhelmingly dominant (a black
        // cover with a fair patch of white should accent white, not black).
        let vivid = if n * 1000 >= samples { chroma.powi(3) } else { 0.0 };
        n as f32 * (1e-4 * (0.1 + max) + vivid)
    };
    let best = buckets.iter().max_by(|a, b| score(a).total_cmp(&score(b)));
    match best {
        Some(&[n, r, g, b]) if n > 0 => {
            iced::Color::from_rgb((r / n) as f32 / 255.0, (g / n) as f32 / 255.0, (b / n) as f32 / 255.0)
        }
        _ => iced::Color::BLACK,
    }
}

async fn load_cache(path: &Path) -> Cache {
    let src = match smol::fs::read_to_string(path).await {
        Ok(src) => src,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Cache::default(),
        Err(e) => {
            log::warn!("could not read the tag cache at {path:?}: {e}");
            return Cache::default();
        }
    };
    match serde_json::from_str::<Cache>(&src) {
        Ok(cache) if cache.version == CACHE_VERSION => cache,
        Ok(_) => Cache::default(),
        Err(e) => {
            log::warn!("discarding unreadable tag cache at {path:?}: {e}");
            Cache::default()
        }
    }
}

/// Best-effort atomic cache write; failure only costs a rescan later.
async fn save_cache(path: &Path, cache: &Cache) {
    let write = async {
        if let Some(dir) = path.parent() {
            smol::fs::create_dir_all(dir).await?;
        }
        let json = serde_json::to_string(cache).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.partial");
        smol::fs::write(&tmp, json).await?;
        smol::fs::rename(&tmp, path).await
    };
    if let Err(e) = write.await {
        log::warn!("could not write the tag cache to {path:?}: {e}");
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The album index round-trips everything but the runtime-only cover art.
    #[test]
    fn album_index_roundtrip() {
        let root = std::env::temp_dir().join(format!("phonoscule-index-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = Some(root.join("albums.json"));

        let album = Album {
            id: 7,
            title: "One".into(),
            artist: "Artist".into(),
            genre: "Genre".into(),
            cover_id: Some(9),
            cover: None,
            accent: Some(iced::Color { r: 0.25, g: 0.5, b: 0.75, a: 1.0 }),
            tracks: vec![TrackInfo { path: "/x/one/1.opus".into(), title: "First".into() }],
        };
        smol::block_on(async {
            save_index(path.clone(), std::slice::from_ref(&album)).await;
            let loaded = load_index(path.clone()).await;
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].id, album.id);
            assert_eq!(loaded[0].title, album.title);
            assert_eq!(loaded[0].genre, album.genre);
            assert_eq!(loaded[0].cover_id, album.cover_id);
            assert_eq!(loaded[0].tracks, album.tracks);
            assert_eq!(loaded[0].accent, album.accent, "the accent color round-trips");
            assert!(loaded[0].cover.is_none(), "covers are runtime-only");
        });

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A minimal valid WAV: 48 kHz stereo 16-bit PCM silence with LIST-INFO tags.
    fn wav_bytes(title: &str, artist: &str, album: &str, track: Option<u32>) -> Vec<u8> {
        fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut out = Vec::with_capacity(8 + body.len() + 1);
            out.extend(id);
            out.extend((body.len() as u32).to_le_bytes());
            out.extend(body);
            if body.len() % 2 == 1 {
                out.push(0); // chunks are padded to even sizes
            }
            out
        }
        fn info(id: &[u8; 4], value: &str) -> Vec<u8> {
            chunk(id, &[value.as_bytes(), b"\0"].concat())
        }
        let mut fmt = Vec::new();
        fmt.extend(1u16.to_le_bytes()); // WAVE_FORMAT_PCM
        fmt.extend(2u16.to_le_bytes()); // channels
        fmt.extend(48000u32.to_le_bytes()); // blocks per second
        fmt.extend((48000u32 * 4).to_le_bytes()); // avg bytes per second
        fmt.extend(4u16.to_le_bytes()); // block size
        fmt.extend(16u16.to_le_bytes()); // bits per sample
        let mut list =
            [&b"INFO"[..], &info(b"INAM", title), &info(b"IART", artist), &info(b"IPRD", album), &info(b"IGNR", "Test Genre")]
                .concat();
        if let Some(track) = track {
            list.extend(info(b"ITRK", &track.to_string()));
        }
        let riff = [&b"WAVE"[..], &chunk(b"fmt ", &fmt), &chunk(b"LIST", &list), &chunk(b"data", &[0u8; 4800])].concat();
        let mut out = Vec::new();
        out.extend(b"RIFF");
        out.extend((riff.len() as u32).to_le_bytes());
        out.extend(riff);
        out
    }

    /// A minimal Ogg Opus stream: the two header packets -- with the given vorbis comments --
    /// and one dummy audio packet. Tags WAV's LIST-INFO can't express (ALBUMARTIST, DISCNUMBER)
    /// need this.
    fn opus_bytes(comments: &[(&str, &str)]) -> Vec<u8> {
        let mut head = Vec::new();
        head.extend(b"OpusHead");
        head.push(1); // version
        head.push(2); // channels
        head.extend(312u16.to_le_bytes()); // pre-skip
        head.extend(48000u32.to_le_bytes()); // input sample rate
        head.extend(0u16.to_le_bytes()); // output gain
        head.push(0); // channel mapping family 0

        let mut tags = Vec::new();
        tags.extend(b"OpusTags");
        let vendor = b"phonoscule-test";
        tags.extend((vendor.len() as u32).to_le_bytes());
        tags.extend(vendor);
        tags.extend((comments.len() as u32).to_le_bytes());
        for (key, value) in comments {
            let comment = format!("{key}={value}");
            tags.extend((comment.len() as u32).to_le_bytes());
            tags.extend(comment.as_bytes());
        }

        let serial = 0x5eed;
        let mut out = Vec::new();
        let mut writer = ogg::PacketWriter::new(std::io::Cursor::new(&mut out));
        writer.write_packet(head, serial, ogg::PacketWriteEndInfo::EndPage, 0).unwrap();
        writer.write_packet(tags, serial, ogg::PacketWriteEndInfo::EndPage, 0).unwrap();
        writer.write_packet(vec![0xfc], serial, ogg::PacketWriteEndInfo::EndStream, 960).unwrap();
        drop(writer);
        out
    }

    /// Scans and applies the event stream the way the GUI does: upsert by album id, then retain
    /// what `Done` reports.
    fn scan_and_apply(albums: &mut Vec<Album>, options: ScanOptions) {
        smol::block_on(async {
            let mut stream = std::pin::pin!(scan(options));
            while let Some(event) = stream.next().await {
                match event {
                    ScanEvent::Album(mut album) => {
                        if let Some(ix) = albums.iter().position(|a| a.id == album.id) {
                            let old = albums.remove(ix);
                            if old.cover_id == album.cover_id {
                                album.cover = old.cover;
                            }
                        }
                        albums.push(*album);
                    }
                    ScanEvent::Cover { albums: ids, art } => {
                        for album in albums.iter_mut().filter(|a| ids.contains(&a.id) && a.cover_id == Some(art.id)) {
                            album.cover = Some(art.clone());
                        }
                    }
                    ScanEvent::Done { album_ids } => {
                        albums.retain(|a| album_ids.contains(&a.id));
                        break;
                    }
                }
            }
            albums.sort_by(|a, b| a.title.cmp(&b.title));
        })
    }

    #[test]
    fn accent_prefers_the_saturated_color() {
        // Mostly dull gray, with a strong red minority: the red should win.
        let mut rgb = Vec::new();
        for i in 0..10_000 {
            if i % 4 == 0 {
                rgb.extend([200u8, 16, 16]);
            } else {
                rgb.extend([90u8, 90, 90]);
            }
        }
        let accent = accent_color(&rgb);
        assert!(accent.r > 0.5 && accent.g < 0.2 && accent.b < 0.2, "{accent:?}");
    }

    #[test]
    fn accent_ignores_a_dominant_bright_near_neutral() {
        // Mostly bright, slightly warm near-white (like skin filling a cover), with a modest
        // amount of vivid red spread over a few different shades: the red must still win.
        let mut rgb = Vec::new();
        for i in 0..10_000 {
            match i % 20 {
                0 => rgb.extend([204u8, 24, 24]),
                1 => rgb.extend([232u8, 40, 32]),
                2 => rgb.extend([176u8, 16, 40]),
                _ => rgb.extend([235u8, 225, 218]),
            }
        }
        let accent = accent_color(&rgb);
        assert!(accent.r > 0.5 && accent.g < 0.3 && accent.b < 0.3, "{accent:?}");
    }

    #[test]
    fn accent_ignores_a_dark_color_cast_mass() {
        // Mostly near-black with a faint teal cast (hair and shadow on a dim photo) -- a huge
        // channel ratio but barely any color -- against a modest vivid blue: the blue must win.
        let mut rgb = Vec::new();
        for i in 0..10_000 {
            match i % 8 {
                0 => rgb.extend([40u8, 80, 220]),
                1 => rgb.extend([60u8, 100, 235]),
                _ => rgb.extend([4u8, 45, 34]),
            }
        }
        let accent = accent_color(&rgb);
        assert!(accent.b > 0.5 && accent.b > accent.g, "{accent:?}");
    }

    #[test]
    fn accent_prefers_a_bright_neutral_over_a_dark_mass() {
        // Achromatic art, mostly black with a substantial bright-gray region (a parchment
        // background): the bright shade makes the better accent.
        let mut rgb = Vec::new();
        for i in 0..10_000 {
            if i % 3 == 0 {
                rgb.extend([215u8, 213, 209]);
            } else {
                rgb.extend([7u8, 7, 7]);
            }
        }
        let accent = accent_color(&rgb);
        assert!(accent.r > 0.7, "{accent:?}");
    }

    #[test]
    fn accent_prefers_a_vivid_sliver_over_a_dark_mass() {
        // Almost entirely near-black, with a small vivid red region (~1%, a lit phone screen on
        // a dark cover): the red is the only real color and must win.
        let mut rgb = Vec::new();
        for i in 0..10_000 {
            if i % 100 == 0 {
                rgb.extend([180u8, 30, 40]);
            } else {
                rgb.extend([7u8, 8, 12]);
            }
        }
        let accent = accent_color(&rgb);
        assert!(accent.r > 0.4 && accent.r > accent.b, "{accent:?}");
    }

    /// Cache-less scan options for a test library at `root`.
    fn plain_options(root: &Path) -> ScanOptions {
        ScanOptions {
            root: root.to_path_buf(),
            priority: vec![],
            known_covers: Default::default(),
            cache_file: None,
            covers_dir: None,
        }
    }

    /// Albums are what the tags say, not what the directories say: two same-named albums by
    /// different artists stay separate even side by side in one directory, and one album spread
    /// over several directories assembles into one, its tracks ordered by their numbers.
    #[test]
    fn albums_group_by_artist_and_title_across_directories() {
        let root = std::env::temp_dir().join(format!("phonoscule-group-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("pool")).unwrap();
        std::fs::write(root.join("pool/a.wav"), wav_bytes("Neon", "FM-84", "Atlas", Some(1))).unwrap();
        std::fs::write(root.join("pool/b.wav"), wav_bytes("Wishing Wells", "Parkway Drive", "Atlas", Some(1))).unwrap();
        for (dir, title, track) in [("Spread/CD1", "One", 1), ("Spread/CD2", "Two", 2)] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join("t.wav"), wav_bytes(title, "Artist", "Spread", Some(track))).unwrap();
        }

        let mut albums = Vec::new();
        scan_and_apply(&mut albums, plain_options(&root));
        let by_artist: Vec<(&str, &str, usize)> =
            albums.iter().map(|a| (a.artist.as_str(), a.title.as_str(), a.tracks.len())).collect();
        assert!(by_artist.contains(&("FM-84", "Atlas", 1)), "{by_artist:?}");
        assert!(by_artist.contains(&("Parkway Drive", "Atlas", 1)), "{by_artist:?}");
        assert!(by_artist.contains(&("Artist", "Spread", 2)), "{by_artist:?}");
        assert_eq!(albums.len(), 3);
        let spread = albums.iter().find(|a| a.title == "Spread").unwrap();
        let titles: Vec<&str> = spread.tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["One", "Two"], "track numbers order the merged album");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A warm rescan (everything cached) reports a multi-directory album exactly once, fully
    /// assembled: the tag cache predicts the album's directories, so consumers see none of the
    /// partial growth a cold scan streams -- and rescans stay churn-free.
    #[test]
    fn warm_rescan_reports_each_album_once() {
        let root = std::env::temp_dir().join(format!("phonoscule-warm-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, title, track) in [("CD1", "One", 1), ("CD2", "Two", 2)] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join("t.wav"), wav_bytes(title, "Artist", "Spread", Some(track))).unwrap();
        }
        let options = || ScanOptions { cache_file: Some(root.join("cache.json")), ..plain_options(&root) };

        // The cold scan primes the cache (its events may show the album growing).
        let mut albums = Vec::new();
        scan_and_apply(&mut albums, options());
        assert_eq!(albums.len(), 1);

        let mut reports = Vec::new();
        smol::block_on(async {
            let mut stream = std::pin::pin!(scan(options()));
            while let Some(event) = stream.next().await {
                match event {
                    ScanEvent::Album(album) => reports.push(album.tracks.len()),
                    ScanEvent::Cover { .. } => (),
                    ScanEvent::Done { .. } => break,
                }
            }
        });
        assert_eq!(reports, [2], "one report, already holding both discs' tracks");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// ALBUMARTIST is the grouping identity (and the byline) when present: a compilation whose
    /// tracks credit different artists stays one album. Without it, the same tracks would
    /// fragment per artist -- the documented cost of not guessing from paths.
    #[test]
    fn album_artist_binds_a_compilation() {
        let root = std::env::temp_dir().join(format!("phonoscule-albumartist-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (file, title, artist, track) in [("1.opus", "Opener", "First Act", "1"), ("2.opus", "Closer", "Second Act", "2")] {
            let comments = [
                ("TITLE", title),
                ("ARTIST", artist),
                ("ALBUMARTIST", "Various Artists"),
                ("ALBUM", "Sampler"),
                ("TRACKNUMBER", track),
            ];
            std::fs::write(root.join(file), opus_bytes(&comments)).unwrap();
        }

        let mut albums = Vec::new();
        scan_and_apply(&mut albums, plain_options(&root));
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].artist, "Various Artists");
        let titles: Vec<&str> = albums[0].tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Opener", "Closer"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// An artist's untagged loose tracks pool into one "Singles" album, wherever the files sit.
    #[test]
    fn untagged_albums_pool_into_singles() {
        let root = std::env::temp_dir().join(format!("phonoscule-singles-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, title) in [("here", "Loosie"), ("there/deeper", "Another")] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join("t.wav"), wav_bytes(title, "Artist", "", None)).unwrap();
        }

        let mut albums = Vec::new();
        scan_and_apply(&mut albums, plain_options(&root));
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].title, "Singles");
        assert_eq!(albums[0].artist, "Artist");
        let titles: Vec<&str> = albums[0].tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Another", "Loosie"], "no numbers: titles order the pool");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Tracks order by their tags: track number first, title for the untagged rest -- never by
    /// the file names.
    #[test]
    fn tracks_order_by_their_tags() {
        let root = std::env::temp_dir().join(format!("phonoscule-order-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (file, title, track) in
            [("w.wav", "Beta", Some(2)), ("x.wav", "Alpha", Some(1)), ("y.wav", "B Side", None), ("z.wav", "A Side", None)]
        {
            std::fs::write(root.join(file), wav_bytes(title, "Artist", "Album", track)).unwrap();
        }

        let mut albums = Vec::new();
        scan_and_apply(&mut albums, plain_options(&root));
        assert_eq!(albums.len(), 1);
        let titles: Vec<&str> = albums[0].tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Alpha", "Beta", "A Side", "B Side"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cached_incremental_rescans() {
        let root = std::env::temp_dir().join(format!("phonoscule-library-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, title) in [("One", "First Song"), ("Two", "Second Song")] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join("track.wav"), wav_bytes(title, "Artist", dir, None)).unwrap();
        }
        let cache_file = root.join("cache.json");
        let options = || ScanOptions {
            root: root.clone(),
            priority: vec![],
            known_covers: Default::default(),
            cache_file: Some(cache_file.clone()),
            covers_dir: None,
        };

        // Initial scan populates the cache.
        let mut albums = Vec::new();
        scan_and_apply(&mut albums, options());
        assert_eq!(albums.len(), 2);
        assert_eq!(albums[0].tracks[0].title, "First Song");
        assert_eq!(albums[0].genre, "Test Genre", "the genre tag reaches the album");
        assert!(cache_file.exists());
        let ids = (albums[0].id, albums[1].id);

        // An unchanged rescan yields the same albums, with stable ids.
        scan_and_apply(&mut albums, options());
        assert_eq!(albums.len(), 2);
        assert_eq!(ids, (albums[0].id, albums[1].id));

        // A modified file (different size => cache miss) is re-parsed.
        std::fs::write(root.join("One/track.wav"), wav_bytes("First Song, Remastered", "Artist", "One", None)).unwrap();
        scan_and_apply(&mut albums, options());
        assert_eq!(albums[0].tracks[0].title, "First Song, Remastered");

        // Proof the cache is trusted: an in-place edit with the same size and a restored mtime
        // is, by design, not noticed -- the cached tags remain.
        let path = root.join("One/track.wav");
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, wav_bytes("Sneaky Edit Same Size!", "Artist", "One", None)).unwrap();
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(mtime).unwrap();
        scan_and_apply(&mut albums, options());
        assert_eq!(albums[0].tracks[0].title, "First Song, Remastered");

        // A removed album disappears.
        std::fs::remove_dir_all(root.join("Two")).unwrap();
        scan_and_apply(&mut albums, options());
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].title, "One");

        let _ = std::fs::remove_dir_all(&root);
    }
}
