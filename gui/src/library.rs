//! Music library scanning: find audio files, read their tags with phonoscule, group them into
//! albums, and load folder cover art (`cover.jpg` & friends).
//!
//! [`scan`] streams results incrementally: albums are reported as soon as their directory has
//! been read, and cover art (the expensive part) trickles in afterwards, decoded concurrently.

use embedded_io_adapters::futures_03::FromFutures;
use embedded_io_async::{Read as _, Seek as _, SeekFrom};
use futures::StreamExt;
use phonoscule::{
    io::Skippable,
    metadata::{Metadata, StaticMetadata},
    opus,
    wav::Wav,
};
use smol::{
    channel,
    fs::File,
    io::BufReader,
    stream::Stream,
};
use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone)]
pub struct Album {
    /// Scan-unique id, used to associate later [`ScanEvent::Cover`] events.
    pub id: u64,
    pub title: String,
    pub artist: String,
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
    pub id: u64,
    pub size: (u32, u32),
    pub rgba: Arc<Vec<u8>>,
    pub handle: iced::widget::image::Handle,
}

impl fmt::Debug for CoverArt {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("CoverArt").field("id", &self.id).field("size", &self.size).finish()
    }
}

#[derive(Debug, Clone)]
pub enum ScanEvent {
    /// A fully discovered album. Its cover art may still be loading.
    Album(Album),
    /// Cover art finished loading for the albums with the given ids.
    Cover { albums: Vec<u64>, art: CoverArt },
    /// The scan is complete: every album and cover has been reported.
    Done,
}

/// Cover art is downscaled to fit this square (center-cropped, like the iPod did).
const COVER_SIZE: u32 = 512;

/// Scans `root`, streaming results as they are found. The stream ends after [`ScanEvent::Done`]
/// (or early, if the scan task fails); dropping it cancels the scan.
pub fn scan(root: PathBuf) -> impl Stream<Item = ScanEvent> + Send {
    let (tx, rx) = channel::bounded(64);
    smol::spawn(drive(root, tx)).detach();
    rx
}

/// How many directories have their tags read, and how many covers are decoded, at once.
fn concurrency() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

async fn drive(root: PathBuf, tx: channel::Sender<ScanEvent>) {
    log::info!("scanning {root:?}");

    // Phase 1: walk the tree, collecting the audio files of each directory. Cheap (no file
    // contents are read), so not worth parallelizing.
    let mut dirs = vec![root];
    let mut jobs = Vec::new();
    while let Some(dir) = dirs.pop() {
        let Ok(mut entries) = smol::fs::read_dir(&dir).await else {
            log::warn!("could not read directory {dir:?}");
            continue;
        };
        let mut audio_files = Vec::new();
        while let Some(Ok(entry)) = entries.next().await {
            let path = entry.path();
            let Ok(file_type) = entry.file_type().await else { continue };
            if file_type.is_dir() {
                dirs.push(path);
            } else if matches!(extension(&path).as_deref(), Some("wav" | "opus")) {
                audio_files.push(path);
            }
        }
        if !audio_files.is_empty() {
            audio_files.sort();
            jobs.push((dir, audio_files));
        }
    }

    // Phase 2 & 3 run concurrently: tag reading emits albums per directory and queues that
    // directory's cover; cover decoding streams in whenever ready. Sends only fail when the
    // receiver is gone (scan cancelled), which also cancels these phases via `return`.
    let (cover_tx, cover_rx) = channel::bounded::<(Vec<u64>, PathBuf)>(64);
    let mut n_albums = 0u64;

    let read_tags_phase = async {
        let mut per_dir = futures::stream::iter(jobs)
            .map(|(dir, files)| async move {
                let albums = albums_in_dir(&files).await;
                (dir, albums)
            })
            .buffer_unordered(concurrency());
        while let Some((dir, albums)) = per_dir.next().await {
            let mut ids = Vec::with_capacity(albums.len());
            for mut album in albums {
                album.id = n_albums;
                n_albums += 1;
                ids.push(album.id);
                if tx.send(ScanEvent::Album(album)).await.is_err() {
                    return;
                }
            }
            if cover_tx.send((ids, dir)).await.is_err() {
                return;
            }
        }
        drop(cover_tx); // lets the cover phase finish
    };

    let covers_phase = async {
        // Pinned on the stack: the channel receiver (hence the whole chain) is not `Unpin`.
        let mut covers = std::pin::pin!(
            cover_rx
                .map(|(ids, dir)| async move { (ids, load_cover_in_dir(&dir).await) })
                .buffer_unordered(concurrency())
        );
        let mut n_covers = 0u64;
        while let Some((ids, cover)) = covers.next().await {
            let Some((size, rgba)) = cover else { continue };
            let handle = iced::widget::image::Handle::from_rgba(size.0, size.1, rgba.clone());
            let art = CoverArt { id: n_covers, size, rgba: Arc::new(rgba), handle };
            n_covers += 1;
            if tx.send(ScanEvent::Cover { albums: ids, art }).await.is_err() {
                return;
            }
        }
    };

    // The clone lets read_tags_phase drop its cover_tx while covers_phase still runs.
    futures::join!(read_tags_phase, covers_phase);
    log::info!("scan done: found {n_albums} albums");
    let _ = tx.send(ScanEvent::Done).await;
}

/// Reads the tags of each file and groups them into albums (id 0; the caller assigns real ids).
/// The files all live in one directory, so tracks only group into the same album when both their
/// directory and their album tag agree.
async fn albums_in_dir(files: &[PathBuf]) -> Vec<Album> {
    let mut albums: Vec<Album> = Vec::new();
    let mut by_title: HashMap<String, usize> = HashMap::new();
    for path in files {
        let Some(tags) = read_tags(path).await else {
            log::warn!("could not parse {path:?}");
            continue;
        };
        let title = match tags.title() {
            "" => path.file_stem().unwrap_or_default().to_string_lossy().to_string(),
            t => t.to_string(),
        };
        let album_title = match tags.album() {
            "" => parent_name(path),
            a => a.to_string(),
        };
        let ix = *by_title.entry(album_title.clone()).or_insert_with(|| {
            albums.push(Album {
                id: 0,
                title: album_title,
                artist: match tags.artist() {
                    "" => "Unknown Artist".to_string(),
                    a => a.to_string(),
                },
                cover: None,
                tracks: vec![],
            });
            albums.len() - 1
        });
        albums[ix].tracks.push(TrackInfo { path: path.clone(), title });
    }
    albums
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

/// Finds and decodes the cover image of a directory, downscaled to [`COVER_SIZE`]. The decode
/// runs on the blocking thread pool, so calls can proceed in parallel regardless of executor
/// threads.
async fn load_cover_in_dir(dir: &Path) -> Option<((u32, u32), Vec<u8>)> {
    const STEMS: [&str; 4] = ["cover", "folder", "front", "albumart"];
    const EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];
    let mut file = None;
    let mut entries = smol::fs::read_dir(dir).await.ok()?;
    while let Some(Ok(entry)) = entries.next().await {
        let path = entry.path();
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
        let ext = extension(&path).unwrap_or_default();
        if STEMS.contains(&stem.as_str()) && EXTS.contains(&ext.as_str()) {
            file = Some(path);
            break;
        }
    }
    let file = file?;
    smol::unblock(move || {
        let img = image::open(&file)
            .inspect_err(|e| log::warn!("could not decode cover {file:?}: {e}"))
            .ok()?;
        let rgba = img.resize_to_fill(COVER_SIZE, COVER_SIZE, image::imageops::FilterType::Triangle).into_rgba8();
        let size = rgba.dimensions();
        Some((size, rgba.into_raw()))
    })
    .await
}
