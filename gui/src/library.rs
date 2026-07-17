//! Music library scanning: find audio files, read their tags with phonoscule, group them into
//! albums, and load folder cover art (`cover.jpg` & friends).
//!
//! [`scan`] streams results incrementally: albums are reported as soon as their directory has
//! been read, and cover art (the expensive part) trickles in afterwards, decoded concurrently.
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
use phonoscule::{
    io::Skippable,
    metadata::{Metadata, StaticMetadata},
    opus,
    wav::Wav,
};
use serde::{Deserialize, Serialize};
use smol::{channel, fs::File, io::BufReader, stream::Stream};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

#[derive(Debug, Clone)]
pub struct Album {
    /// Stable content-derived id (directory + album title), the same across re-scans.
    pub id: u64,
    pub title: String,
    pub artist: String,
    /// The id the cover art for this album has (or would have, when not loaded yet).
    pub cover_id: Option<u64>,
    pub cover: Option<CoverArt>,
    pub tracks: Vec<TrackInfo>,
}

#[derive(Debug, Clone)]
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
    /// A fully discovered album. Its cover art may still be loading (or, when its cover id was
    /// in [`ScanOptions::known_covers`], arrive not at all: keep what you have).
    Album(Album),
    /// Cover art finished loading for the albums with the given ids.
    Cover { albums: Vec<u64>, art: CoverArt },
    /// The scan is complete: every album has been reported. Albums absent from `album_ids` no
    /// longer exist and should be dropped.
    Done { album_ids: Vec<u64> },
}

pub struct ScanOptions {
    pub root: PathBuf,
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
/// for the library grid; the now-playing view decodes a higher-resolution version on demand.
pub const THUMB: u32 = 320;

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
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    version: u32,
    files: HashMap<PathBuf, CacheEntry>,
}

const CACHE_VERSION: u32 = 1;

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

/// A directory's worth of scanning work.
struct DirJob {
    dir: PathBuf,
    files: Vec<AudioFile>,
    cover: Option<(PathBuf, SystemTime)>,
}

async fn drive(options: ScanOptions, tx: channel::Sender<ScanEvent>) {
    log::info!("scanning {:?}", options.root);
    let cache = match &options.cache_file {
        Some(path) => load_cache(path).await,
        None => Cache::default(),
    };

    // Phase 1: walk the tree, collecting each directory's audio files (with stat data) and its
    // cover image candidate in a single directory listing.
    let mut dirs = vec![options.root];
    let mut jobs = Vec::new();
    while let Some(dir) = dirs.pop() {
        match dir_job(&dir, &mut dirs).await {
            Some(job) if !job.files.is_empty() => jobs.push(job),
            _ => (),
        }
    }

    // Phases 2 & 3 run concurrently: tag reading emits albums per directory and queues that
    // directory's cover; cover decoding streams in whenever ready. Sends only fail when the
    // receiver is gone (scan cancelled), which also cancels these phases via `return`.
    let (cover_tx, cover_rx) = channel::bounded::<(Vec<u64>, PathBuf, SystemTime)>(64);
    let mut album_ids = Vec::new();
    let mut fresh = HashMap::new();
    let mut n_parsed = 0usize;

    let cache = &cache;
    let read_tags_phase = async {
        let mut per_dir = futures::stream::iter(jobs)
            .map(|job| async move {
                let albums = albums_in_dir(&job, cache).await;
                (job, albums)
            })
            .buffer_unordered(concurrency());
        while let Some((job, (albums, entries, parsed))) = per_dir.next().await {
            n_parsed += parsed;
            fresh.extend(entries);
            let mut ids = Vec::with_capacity(albums.len());
            for album in albums {
                ids.push(album.id);
                album_ids.push(album.id);
                if tx.send(ScanEvent::Album(album)).await.is_err() {
                    return;
                }
            }
            if let Some((path, mtime)) = job.cover
                && !options.known_covers.contains(&stable_id((&path, mtime)))
                && cover_tx.send((ids, path, mtime)).await.is_err()
            {
                return;
            }
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

/// Reads the tags of each file (from the cache when it is still valid) and groups them into
/// albums. Tracks only group into the same album when both their directory and their album tag
/// agree. Returns the albums, the fresh cache entries, and how many files were actually parsed.
async fn albums_in_dir(job: &DirJob, cache: &Cache) -> (Vec<Album>, Vec<(PathBuf, CacheEntry)>, usize) {
    let mut albums: Vec<Album> = Vec::new();
    let mut by_title: HashMap<String, usize> = HashMap::new();
    let mut entries = Vec::with_capacity(job.files.len());
    let mut n_parsed = 0usize;
    let cover_id = job.cover.as_ref().map(|(path, mtime)| stable_id((path, mtime)));

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
                // Cache the resolved values, fallbacks applied.
                CacheEntry {
                    mtime: file.mtime,
                    size: file.size,
                    title: match tags.title() {
                        "" => file.path.file_stem().unwrap_or_default().to_string_lossy().to_string(),
                        t => t.to_string(),
                    },
                    artist: match tags.artist() {
                        "" => "Unknown Artist".to_string(),
                        a => a.to_string(),
                    },
                    album: match tags.album() {
                        "" => parent_name(&file.path),
                        a => a.to_string(),
                    },
                }
            }
        };
        let ix = *by_title.entry(entry.album.clone()).or_insert_with(|| {
            albums.push(Album {
                id: stable_id((&job.dir, &entry.album)),
                title: entry.album.clone(),
                artist: entry.artist.clone(),
                cover_id,
                cover: None,
                tracks: vec![],
            });
            albums.len() - 1
        });
        albums[ix].tracks.push(TrackInfo { path: file.path.clone(), title: entry.title.clone() });
        entries.push((file.path.clone(), entry));
    }
    (albums, entries, n_parsed)
}

fn parent_name(path: &Path) -> String {
    path.parent().and_then(Path::file_name).unwrap_or_default().to_string_lossy().to_string()
}

fn extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_string_lossy().to_lowercase())
}

async fn read_tags(path: &Path) -> Option<StaticMetadata> {
    let mut f = Skippable(FromFutures::new(BufReader::new(File::open(path).await.ok()?)));
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic).await.ok()?;
    f.seek(SeekFrom::Start(0)).await.ok()?;
    match &magic {
        b"RIFF" => Some(Wav::<StaticMetadata, _>::parse(f).await?.metadata),
        b"OggS" => Some(opus::Headers::<StaticMetadata>::parse(&mut f).await?.metadata),
        _ => None,
    }
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
    let score = |&[n, r, g, b]: &[u64; 4]| {
        if n == 0 {
            return 0.0;
        }
        let (r, g, b) = ((r / n) as f32 / 255.0, (g / n) as f32 / 255.0, (b / n) as f32 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let saturation = if max > 0.0 { (max - min) / max } else { 0.0 };
        // Distinctness must dominate size: saturation counts cubed, and the population floor is
        // only a tiebreaker so that grayscale art degrades to its most common shade -- any
        // bigger and a large mass of near-white/gray outweighs smaller vivid regions.
        n as f32 * (0.01 + saturation.powi(3) * max)
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

    /// A minimal valid WAV: 48 kHz stereo 16-bit PCM silence with LIST-INFO tags.
    fn wav_bytes(title: &str, artist: &str, album: &str) -> Vec<u8> {
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
        let list = [&b"INFO"[..], &info(b"INAM", title), &info(b"IART", artist), &info(b"IPRD", album)].concat();
        let riff = [&b"WAVE"[..], &chunk(b"fmt ", &fmt), &chunk(b"LIST", &list), &chunk(b"data", &[0u8; 4800])].concat();
        let mut out = Vec::new();
        out.extend(b"RIFF");
        out.extend((riff.len() as u32).to_le_bytes());
        out.extend(riff);
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
                        albums.push(album);
                    }
                    ScanEvent::Cover { albums: ids, art } => {
                        for album in albums.iter_mut().filter(|a| ids.contains(&a.id)) {
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
    fn cached_incremental_rescans() {
        let root = std::env::temp_dir().join(format!("phonoscule-library-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, title) in [("One", "First Song"), ("Two", "Second Song")] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join("track.wav"), wav_bytes(title, "Artist", dir)).unwrap();
        }
        let cache_file = root.join("cache.json");
        let options = || ScanOptions {
            root: root.clone(),
            known_covers: Default::default(),
            cache_file: Some(cache_file.clone()),
            covers_dir: None,
        };

        // Initial scan populates the cache.
        let mut albums = Vec::new();
        scan_and_apply(&mut albums, options());
        assert_eq!(albums.len(), 2);
        assert_eq!(albums[0].tracks[0].title, "First Song");
        assert!(cache_file.exists());
        let ids = (albums[0].id, albums[1].id);

        // An unchanged rescan yields the same albums, with stable ids.
        scan_and_apply(&mut albums, options());
        assert_eq!(albums.len(), 2);
        assert_eq!(ids, (albums[0].id, albums[1].id));

        // A modified file (different size => cache miss) is re-parsed.
        std::fs::write(root.join("One/track.wav"), wav_bytes("First Song, Remastered", "Artist", "One")).unwrap();
        scan_and_apply(&mut albums, options());
        assert_eq!(albums[0].tracks[0].title, "First Song, Remastered");

        // Proof the cache is trusted: an in-place edit with the same size and a restored mtime
        // is, by design, not noticed -- the cached tags remain.
        let path = root.join("One/track.wav");
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, wav_bytes("Sneaky Edit Same Size!", "Artist", "One")).unwrap();
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
