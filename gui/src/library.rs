//! Music library scanning: find audio files, read their tags with phonoscule, group them into
//! albums, and load folder cover art (`cover.jpg` & friends).

use embedded_io_adapters::futures_03::FromFutures;
use futures::StreamExt;
use phonoscule::{
    io::Skippable,
    metadata::{Metadata, StaticMetadata},
    opus::OggOpus,
    wav::Wav,
};
use smol::{
    fs::File,
    io::{AsyncReadExt, BufReader},
};
use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone)]
pub struct Album {
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

/// Cover art is downscaled to fit this square (center-cropped, like the iPod did).
const COVER_SIZE: u32 = 512;

pub async fn scan(root: PathBuf) -> Vec<Album> {
    log::info!("scanning {root:?}");

    // Collect all audio files under the root.
    let mut audio_files = Vec::new();
    let mut dirs = vec![root];
    while let Some(dir) = dirs.pop() {
        let Ok(mut entries) = smol::fs::read_dir(&dir).await else {
            log::warn!("could not read directory {dir:?}");
            continue;
        };
        while let Some(Ok(entry)) = entries.next().await {
            let path = entry.path();
            let Ok(file_type) = entry.file_type().await else { continue };
            if file_type.is_dir() {
                dirs.push(path);
            } else if matches!(extension(&path).as_deref(), Some("wav" | "opus")) {
                audio_files.push(path);
            }
        }
    }
    audio_files.sort();

    // Read tags and group into albums. Tracks with no album tag are grouped by directory.
    let mut albums: Vec<Album> = Vec::new();
    let mut album_ix: HashMap<(PathBuf, String), usize> = HashMap::new();
    for path in audio_files {
        let Some(tags) = read_tags(&path).await else {
            log::warn!("could not parse {path:?}");
            continue;
        };
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let title = match tags.title() {
            "" => path.file_stem().unwrap_or_default().to_string_lossy().to_string(),
            t => t.to_string(),
        };
        let album_title = match tags.album() {
            "" => dir.file_name().unwrap_or_default().to_string_lossy().to_string(),
            a => a.to_string(),
        };
        let ix = *album_ix.entry((dir, album_title.clone())).or_insert_with(|| {
            albums.push(Album {
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
        albums[ix].tracks.push(TrackInfo { path, title });
    }

    // Load cover art, one lookup per directory.
    let mut covers: HashMap<PathBuf, Option<CoverArt>> = HashMap::new();
    let mut next_id = 0u64;
    for album in &mut albums {
        let Some(dir) = album.tracks.first().and_then(|t| t.path.parent()) else { continue };
        let cover = match covers.get(dir) {
            Some(cached) => cached.clone(),
            None => {
                let loaded = match find_cover_file(dir).await {
                    Some(file) => load_cover(&file, next_id).await,
                    None => None,
                };
                if loaded.is_some() {
                    next_id += 1;
                }
                covers.insert(dir.to_path_buf(), loaded.clone());
                loaded
            }
        };
        album.cover = cover;
    }

    log::info!("found {} albums", albums.len());
    albums
}

fn extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_string_lossy().to_lowercase())
}

async fn read_tags(path: &Path) -> Option<StaticMetadata> {
    let mut magic = [0u8; 4];
    File::open(path).await.ok()?.read_exact(&mut magic).await.ok()?;
    let f = Skippable(FromFutures::new(BufReader::new(File::open(path).await.ok()?)));
    match &magic {
        b"RIFF" => Some(Wav::<StaticMetadata, _>::parse(f).await?.metadata),
        b"OggS" => Some(OggOpus::<StaticMetadata, _>::parse(f).await?.metadata),
        _ => None,
    }
}

async fn find_cover_file(dir: &Path) -> Option<PathBuf> {
    const STEMS: [&str; 4] = ["cover", "folder", "front", "albumart"];
    const EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];
    let mut entries = smol::fs::read_dir(dir).await.ok()?;
    while let Some(Ok(entry)) = entries.next().await {
        let path = entry.path();
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
        let ext = extension(&path).unwrap_or_default();
        if STEMS.contains(&stem.as_str()) && EXTS.contains(&ext.as_str()) {
            return Some(path);
        }
    }
    None
}

async fn load_cover(path: &Path, id: u64) -> Option<CoverArt> {
    let path = path.to_path_buf();
    let rgba = smol::unblock(move || {
        let img = image::open(&path)
            .inspect_err(|e| log::warn!("could not decode cover {path:?}: {e}"))
            .ok()?;
        Some(img.resize_to_fill(COVER_SIZE, COVER_SIZE, image::imageops::FilterType::Triangle).into_rgba8())
    })
    .await?;
    let (width, height) = rgba.dimensions();
    let rgba = rgba.into_raw();
    let handle = iced::widget::image::Handle::from_rgba(width, height, rgba.clone());
    Some(CoverArt { id, size: (width, height), rgba: Arc::new(rgba), handle })
}
