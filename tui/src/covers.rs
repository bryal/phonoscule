//! Cover art in the terminal, through whatever image protocol it speaks.
//!
//! Covers are kept in bounded caches (see the cache module), so what this costs does not grow with
//! the size of the library -- the point of the player, which is meant for machines that have not got
//! the memory to hold a library's worth of artwork.
//!
//! What is cached is the *encoded* cover and nothing else: the source pixels are dropped once it is
//! encoded, because an entry that kept them would weigh its 400 KiB (or a high-resolution cover's
//! 3 MiB) for as long as it was held. Encoding is also the expensive part -- reading a thumbnail off
//! disk costs tens of microseconds against a few milliseconds to resize and encode it -- so it runs
//! off the UI thread and covers arrive as messages ([`Load`]). Until one does, a view draws the
//! album's accent colour, which is known long before any pixels are, so a keypress waits for nothing.

use crate::cache::Lru;
use image::imageops::FilterType;
use phonoscule::library;
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// How many encoded thumbnails are held. The library's own size does not enter into it.
const THUMB_CAPACITY: usize = 32;

/// How many encoded high-resolution covers are held. Fewer, because only the player shows them and
/// it shows one at a time; the rest of the room is for skipping back and forth through the queue.
const FULL_CAPACITY: usize = 16;

/// How far either side of the browser's cursor thumbnails are loaded before they are asked for, and
/// kept from being evicted. Small, because a cover that is not there yet costs nothing but a coloured
/// block: this is for the neighbours a single keypress reaches, not for guessing where the user is
/// headed.
pub const PIN_RADIUS: usize = 2;

/// How many albums either side of the playing one in the queue keep a high-resolution cover ready.
/// Asymmetric because skipping forward is the commoner move.
pub const FULL_BEHIND: usize = 1;
pub const FULL_AHEAD: usize = 2;

/// Which of an album's two covers is meant. They are cached apart: the thumbnail is what a browser
/// row wants, the high-resolution decode what a player filling half the screen wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// The scan's cached thumbnail: a file read, no image decoding.
    Thumb,
    /// Decoded from the original artwork, for a cover drawn large.
    Full,
}

/// An encoded cover, and the area it was encoded for. Kept together because an encoding is good for
/// one size only: the terminal being resized makes it a miss rather than something to stretch.
struct Encoded {
    protocol: Protocol,
    size: Size,
}

/// The covers held for display, and the terminal's way of drawing them.
pub struct Covers {
    pub picker: Picker,
    /// Where thumbnails are read from, or `None` if there is no cache directory to read -- in which
    /// case covers never appear and the accent colours stand in for good.
    covers_dir: Option<PathBuf>,
    /// The artwork file each cover came from, for decoding it at a higher resolution than the
    /// thumbnail. Paths only: a few kilobytes for a whole library.
    files: HashMap<u64, Arc<PathBuf>>,
    thumbs: Lru<Encoded>,
    full: Lru<Encoded>,
    /// Covers to load, taken by the event loop once the frame that asked for them is out.
    wanted: Vec<Request>,
    /// Ids that must not be evicted, per quality.
    pinned_thumbs: HashSet<u64>,
    pinned_full: HashSet<u64>,
}

/// One cover to load and encode, off the UI thread.
pub struct Request {
    pub cover_id: u64,
    pub quality: Quality,
    /// The area to encode for, and the artwork file when a high-resolution decode is wanted.
    pub size: Size,
    pub file: Option<Arc<PathBuf>>,
}

/// A loaded, encoded cover on its way back to a cache.
pub struct Load {
    pub cover_id: u64,
    pub quality: Quality,
    pub size: Size,
    /// `None` if it could not be read or decoded, which leaves it retryable.
    pub protocol: Option<Protocol>,
}

impl Covers {
    pub fn new(picker: Picker, covers_dir: Option<PathBuf>) -> Self {
        Covers {
            picker,
            covers_dir,
            files: HashMap::new(),
            thumbs: Lru::new(THUMB_CAPACITY),
            full: Lru::new(FULL_CAPACITY),
            wanted: Vec::new(),
            pinned_thumbs: HashSet::new(),
            pinned_full: HashSet::new(),
        }
    }

    /// Remembers where a cover's artwork lives, so it can be decoded large later. Learnt from the
    /// scan, which reports it alongside the thumbnail.
    pub fn learn_file(&mut self, cover_id: u64, file: Arc<PathBuf>) {
        self.files.insert(cover_id, file);
    }

    /// The best encoded cover held for `id` at `size`, preferring the high-resolution one. `None`
    /// when neither is there yet, or neither was encoded for this size.
    pub fn best(&mut self, id: u64, size: Size) -> Option<&Protocol> {
        // Checked before borrowing, since only one of the two lookups may keep its borrow.
        let full = self.full.get(id).is_some_and(|held| held.size == size);
        let cache = if full { &mut self.full } else { &mut self.thumbs };
        cache.get(id).filter(|held| held.size == size).map(|held| &held.protocol)
    }

    /// Asks for a cover, unless it is held at this size already or is on its way. Cheap and
    /// idempotent, so a caller can ask on every frame.
    pub fn want(&mut self, cover_id: u64, quality: Quality, size: Size) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        let file = match quality {
            Quality::Thumb if self.covers_dir.is_none() => return,
            Quality::Thumb => None,
            // Nothing to decode from: the scan has not reported this cover yet.
            Quality::Full => match self.files.get(&cover_id) {
                Some(file) => Some(file.clone()),
                None => return,
            },
        };
        let cache = match quality {
            Quality::Thumb => &mut self.thumbs,
            Quality::Full => &mut self.full,
        };
        // A cover encoded for a different size is stale, not held: ask again at the new one.
        let stale = cache.get(cover_id).is_some_and(|held| held.size != size);
        if stale {
            cache.forget(cover_id);
        }
        if cache.start_loading(cover_id) {
            self.wanted.push(Request { cover_id, quality, size, file });
        }
    }

    /// Names the covers of each quality that must stay held.
    pub fn pin(&mut self, thumbs: impl IntoIterator<Item = u64>, full: impl IntoIterator<Item = u64>) {
        self.pinned_thumbs = thumbs.into_iter().collect();
        self.pinned_full = full.into_iter().collect();
    }

    /// The loads to start, handed to whoever runs them.
    pub fn take_wanted(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.wanted)
    }

    /// Takes in a finished load.
    pub fn absorb(&mut self, load: Load) {
        let Load { cover_id, quality, size, protocol } = load;
        let (cache, pinned) = match quality {
            Quality::Thumb => (&mut self.thumbs, &self.pinned_thumbs),
            Quality::Full => (&mut self.full, &self.pinned_full),
        };
        match protocol {
            Some(protocol) => cache.insert(cover_id, Encoded { protocol, size }, pinned),
            // Leave it uncached and retryable: a thumbnail may appear once the scan writes it.
            None => cache.give_up(cover_id),
        }
    }

    /// Drops every cached cover, for when they were all encoded for an area that no longer exists --
    /// the terminal having been resized. Without this they would linger, counting against the bound,
    /// until each was asked for again and found stale one at a time.
    pub fn clear(&mut self) {
        self.thumbs.clear();
        self.full.clear();
    }

    /// Where thumbnails are read from, for the loader.
    pub fn dir(&self) -> Option<PathBuf> {
        self.covers_dir.clone()
    }
}

/// Loads a cover and encodes it for the area it will be drawn in. The expensive half runs on the
/// blocking pool: a few milliseconds of decoding, resizing and encoding has no business on the
/// thread drawing frames.
pub async fn load(picker: Picker, dir: Option<PathBuf>, request: Request) -> Load {
    let Request { cover_id, quality, size, file } = request;
    let give_up = Load { cover_id, quality, size, protocol: None };
    let image = match quality {
        Quality::Thumb => {
            let Some(dir) = dir else { return give_up };
            let Some(pixels) = library::read_thumbnail(&dir, cover_id).await else { return give_up };
            let edge = library::THUMB;
            match image::RgbaImage::from_raw(edge, edge, pixels.to_vec()) {
                Some(image) => image,
                None => return give_up,
            }
        }
        Quality::Full => {
            let Some(file) = file else { return give_up };
            // Decoded straight to the size it will be drawn at, so the artwork is resized once.
            let edge = drawn_edge(&picker, size);
            let Some(pixels) = library::decode_cover((*file).clone(), edge).await else { return give_up };
            match image::RgbaImage::from_raw(edge, edge, pixels.to_vec()) {
                Some(image) => image,
                None => return give_up,
            }
        }
    };
    let protocol = smol::unblock(move || {
        // The encoded form only: the pixels above are dropped here, rather than held for as long as
        // the cover is cached.
        picker.new_protocol(image::DynamicImage::ImageRgba8(image), size, resize()).ok()
    })
    .await;
    Load { cover_id, quality, size, protocol }
}

/// Asks the terminal what it can draw images with. Must run after the alternate screen is up but
/// before terminal events are read, since it writes a query to stdout and reads the reply from stdin.
///
/// The query costs more than it looks: it reads stdin on a thread of its own, which outlives this
/// call and keeps reading until the terminal sends a device status report. Keys pressed before that
/// arrives are eaten by it rather than delivered to us, so a terminal that answers slowly (or not at
/// all) leaves the player unable to type for as long as two seconds after it starts.
///
/// `forced` names a protocol to use instead, skipping the query and its cost entirely.
pub fn picker(forced: Option<&str>) -> Picker {
    if let Some(name) = forced {
        let mut picker = Picker::halfblocks();
        match protocol_named(name) {
            Some(protocol) => {
                picker.set_protocol_type(protocol);
                log::info!("image protocol {protocol:?}, from the config");
            }
            None => log::warn!("unknown image protocol {name:?}, using half blocks"),
        }
        return picker;
    }
    match Picker::from_query_stdio() {
        Ok(picker) => {
            log::info!("terminal image protocol: {:?}", picker.protocol_type());
            picker
        }
        Err(e) => {
            log::warn!("could not query the terminal for an image protocol, using half blocks: {e}");
            Picker::halfblocks()
        }
    }
}

/// The protocols nameable in the config.
pub const PROTOCOL_NAMES: &str = "kitty, sixel, iterm2, halfblocks";

fn protocol_named(name: &str) -> Option<ProtocolType> {
    match name {
        "kitty" => Some(ProtocolType::Kitty),
        "sixel" => Some(ProtocolType::Sixel),
        "iterm2" => Some(ProtocolType::Iterm2),
        "halfblocks" => Some(ProtocolType::Halfblocks),
        _ => None,
    }
}

/// The pixel edge an area of `size` cells covers: what a cover drawn there should be decoded to.
/// The longer side, since the cover is square and gets center-cropped to fit.
fn drawn_edge(picker: &Picker, size: Size) -> u32 {
    let font = picker.font_size();
    let width = u32::from(size.width) * u32::from(font.width);
    let height = u32::from(size.height) * u32::from(font.height);
    width.max(height).max(1)
}

/// How a cover is fitted to the space it is given. `Scale` rather than `Fit`, which clamps to the
/// source resolution and so would leave a thumbnail sitting at its own 320 pixels in the middle of a
/// larger pane instead of filling it. Bilinear, since that upscaling is otherwise blocky.
pub fn resize() -> Resize {
    Resize::Scale(Some(FilterType::Triangle))
}

/// The largest square area, in cells, fitting within `space`. Square *in pixels*: cells are about
/// twice as tall as they are wide, so a square block of cells comes out stretched.
///
/// What the accent-coloured placeholder fills, and what a cover is asked to encode itself for, so
/// the artwork does not shift when it replaces the placeholder.
pub fn square(picker: &Picker, space: Size) -> Size {
    let font = picker.font_size();
    let (fw, fh) = (u32::from(font.width.max(1)), u32::from(font.height.max(1)));
    let width = u32::from(space.width).min(u32::from(space.height) * fh / fw);
    let height = width * fw / fh;
    Size::new(width.try_into().unwrap_or(u16::MAX), height.try_into().unwrap_or(u16::MAX))
}

#[cfg(test)]
mod test {
    use super::*;

    /// An encoded cover of a plain colour, at `size`.
    fn encoded(picker: &Picker, size: Size) -> Protocol {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(64, 64, image::Rgba([1, 2, 3, 255])));
        picker.new_protocol(image, size, resize()).expect("a plain image encodes")
    }

    fn covers() -> Covers {
        Covers::new(Picker::halfblocks(), Some(PathBuf::from("/covers")))
    }

    /// A cover encoded for one area is not drawn in another: the terminal having been resized must
    /// not stretch what was encoded for the old size.
    #[test]
    fn a_cover_is_only_used_at_the_size_it_was_encoded_for() {
        let (small, large) = (Size::new(20, 10), Size::new(40, 20));
        let mut covers = covers();
        let protocol = encoded(&covers.picker, small);
        covers.absorb(Load { cover_id: 7, quality: Quality::Thumb, size: small, protocol: Some(protocol) });

        assert!(covers.best(7, small).is_some(), "held at the size it was encoded for");
        assert!(covers.best(7, large).is_none(), "not at any other");
    }

    /// Asking at a new size discards the entry and asks again, rather than leaving it to be found
    /// stale over and over.
    #[test]
    fn asking_at_a_new_size_reloads() {
        let (small, large) = (Size::new(20, 10), Size::new(40, 20));
        let mut covers = covers();
        let protocol = encoded(&covers.picker, small);
        covers.absorb(Load { cover_id: 7, quality: Quality::Thumb, size: small, protocol: Some(protocol) });
        assert!(covers.take_wanted().is_empty());

        covers.want(7, Quality::Thumb, small);
        assert!(covers.take_wanted().is_empty(), "already held at this size");

        covers.want(7, Quality::Thumb, large);
        let wanted = covers.take_wanted();
        assert_eq!(wanted.len(), 1, "a new size is a new load");
        assert_eq!(wanted[0].size, large);
        assert!(covers.best(7, small).is_none(), "the entry for the old size is gone");
    }

    /// A resize drops everything, so entries encoded for an area that no longer exists stop counting
    /// against the bound.
    #[test]
    fn clearing_drops_every_cached_cover() {
        let size = Size::new(20, 10);
        let mut covers = covers();
        for id in 0..3 {
            let protocol = encoded(&covers.picker, size);
            covers.absorb(Load { cover_id: id, quality: Quality::Thumb, size, protocol: Some(protocol) });
        }
        assert!(covers.best(1, size).is_some());

        covers.clear();
        for id in 0..3 {
            assert!(covers.best(id, size).is_none(), "cover {id} should be gone");
        }
        covers.want(1, Quality::Thumb, size);
        assert_eq!(covers.take_wanted().len(), 1, "and is loaded afresh when asked for");
    }

    /// A failed load leaves nothing cached and can be tried again.
    #[test]
    fn a_failed_load_is_retried() {
        let size = Size::new(20, 10);
        let mut covers = covers();
        covers.want(7, Quality::Thumb, size);
        assert_eq!(covers.take_wanted().len(), 1);
        covers.want(7, Quality::Thumb, size);
        assert!(covers.take_wanted().is_empty(), "not asked twice while in flight");

        covers.absorb(Load { cover_id: 7, quality: Quality::Thumb, size, protocol: None });
        covers.want(7, Quality::Thumb, size);
        assert_eq!(covers.take_wanted().len(), 1, "asked again once the load failed");
    }
}
