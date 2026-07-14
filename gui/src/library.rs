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
use futures::StreamExt;
use phonoscule::{
    io::Skippable,
    metadata::{Metadata, StaticMetadata},
    opus,
    wav::Wav,
};
use serde::{Deserialize, Serialize};
use smol::{
    channel,
    fs::File,
    io::BufReader,
    stream::Stream,
};
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
    /// The (absolute) image file this was decoded from, e.g. for pointing other programs at it.
    pub file: Arc<PathBuf>,
    pub size: (u32, u32),
    pub rgba: Arc<Vec<u8>>,
    pub handle: iced::widget::image::Handle,
}

impl fmt::Debug for CoverArt {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("CoverArt").field("id", &self.id).field("file", &self.file).field("size", &self.size).finish()
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
}

/// The default location of the tag cache: `$XDG_CACHE_HOME/phonoscule.library.json`, falling
/// back to `~/.cache/phonoscule.library.json`.
pub fn default_cache_file() -> Option<PathBuf> {
    let cache_home = std::env::var("XDG_CACHE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(std::env::home_dir()?.join(".cache")))?;
    Some(cache_home.join("phonoscule.library.json"))
}

/// Cover art is downscaled to fit this square (center-cropped, like the iPod did).
const COVER_SIZE: u32 = 512;

/// Scans `root`, streaming results as they are found. The stream ends after [`ScanEvent::Done`]
/// (or early, if the scan task fails); dropping it cancels the scan.
pub fn scan(options: ScanOptions) -> impl Stream<Item = ScanEvent> + Send {
    let (tx, rx) = channel::bounded(64);
    smol::spawn(drive(options, tx)).detach();
    rx
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

    let covers_phase = async {
        // Pinned on the stack: the channel receiver (hence the whole chain) is not `Unpin`.
        let mut covers = std::pin::pin!(
            cover_rx
                .map(|(ids, path, mtime)| async move { (ids, stable_id((&path, mtime)), load_cover(path).await) })
                .buffer_unordered(concurrency())
        );
        while let Some((ids, cover_id, cover)) = covers.next().await {
            let Some((file, size, rgba)) = cover else { continue };
            let handle = iced::widget::image::Handle::from_rgba(size.0, size.1, rgba.clone());
            let art = CoverArt { id: cover_id, file: Arc::new(file), size, rgba: Arc::new(rgba), handle };
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

/// Decodes a cover image, downscaled to [`COVER_SIZE`], on the blocking thread pool (so calls
/// can proceed in parallel regardless of executor threads). Also returns the absolute path.
async fn load_cover(path: PathBuf) -> Option<(PathBuf, (u32, u32), Vec<u8>)> {
    // Absolute, so consumers (e.g. the MPRIS art URL) don't depend on our working directory.
    let file = smol::fs::canonicalize(path).await.ok()?;
    smol::unblock(move || {
        let img = match image::open(&file) {
            Ok(img) => img,
            Err(e) => {
                log::warn!("could not decode cover {file:?}: {e}");
                return None;
            }
        };
        let rgba = img.resize_to_fill(COVER_SIZE, COVER_SIZE, image::imageops::FilterType::Triangle).into_rgba8();
        let size = rgba.dimensions();
        Some((file, size, rgba.into_raw()))
    })
    .await
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
