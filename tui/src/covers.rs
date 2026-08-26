//! Cover art in the terminal, as half blocks.
//!
//! Covers are kept in bounded caches (see the cache module), so what this costs does not grow with
//! the size of the library -- the point of the player, which is meant for machines that have not got
//! the memory to hold a library's worth of artwork.
//!
//! Half blocks and nothing else, deliberately. One cell carries two colours, so a cover is at most
//! `width` by `height * 2` pixels however grand the terminal's artwork protocol might have been --
//! a preview pane is a few dozen cells across, which is a few dozen pixels. Everything here is sized
//! to that, which the thumbnail cache already exceeds several times over.
//!
//! What is cached is the grid of blocks, not the pixels it came from: those are dropped once it is
//! encoded. Reading and shrinking one costs a few hundred microseconds, enough not to want it on the
//! thread drawing frames, so covers arrive as messages ([`Load`]). Until one does, a view draws the
//! album's accent colour, which is known long before any pixels are, so a keypress waits for nothing.

use crate::cache::Lru;
use phonoscule::library;
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// How many encoded thumbnails are held. The library's own size does not enter into it.
const THUMB_CAPACITY: usize = 32;

/// How far either side of the browser's cursor thumbnails are loaded before they are asked for, and
/// kept from being evicted. Small, because a cover that is not there yet costs nothing but a coloured
/// block: this is for the neighbours a single keypress reaches, not for guessing where the user is
/// headed.
pub const PIN_RADIUS: usize = 2;

/// A terminal cell is about twice as tall as it is wide, and a half block splits it in two -- so a
/// cover of `w` by `h` cells is `w` by `h * 2` pixels, and a square one wants a cell area twice as
/// wide as it is tall.
const CELL_ASPECT: u16 = 2;

/// The square edge covers are cached at.
///
/// A block grid is at most `width` by `height * 2` pixels, and the largest pane this player draws --
/// the player view's cover, on a tall terminal -- comes to something under a hundred a side. This is
/// the next power of two above that, so a cover is never upscaled in practice, and it is a sixteenth
/// of the pixels the graphical player wants for its album grid.
pub const COVER_EDGE: u32 = 128;

/// An encoded cover, and the area it was encoded for. Kept together because an encoding is good for
/// one size only: a lookup at any other misses rather than stretching what it found, which is what
/// catches a load that was already in flight when the terminal changed shape.
struct Encoded {
    protocol: Protocol,
    size: Size,
}

/// Which layout the covers belong to, bumped whenever every one of them is invalidated at once -- a
/// resize. Shared with the loaders, which read it to abandon work for a layout that has since gone:
/// decoding artwork and encoding it costs tens of milliseconds, and there is no point spending them
/// on a cover that will be rejected on arrival.
///
/// A generation rather than the area itself, because the browser's preview pane and the player's are
/// different sizes and one counter invalidates both without either having to know the other's.
#[derive(Clone)]
pub struct Layout(Arc<AtomicU64>);

impl Layout {
    fn new() -> Self {
        Layout(Arc::new(AtomicU64::new(0)))
    }

    fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    fn bump(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// Whether `generation` is still the current layout.
    pub fn current(&self, generation: u64) -> bool {
        self.get() == generation
    }
}

/// The covers held for display.
pub struct Covers {
    /// Where thumbnails are read from, or `None` if there is no cache directory to read -- in which
    /// case covers never appear and the accent colours stand in for good.
    covers_dir: Option<PathBuf>,
    /// The artwork file each cover came from. Not for drawing -- half blocks never want more pixels
    /// than the thumbnail has -- but for pointing other programs at it, and for telling a rescan
    /// which covers it need not read back.
    files: HashMap<u64, Arc<PathBuf>>,
    thumbs: Lru<Encoded>,
    /// Covers to load, taken by the event loop once the frame that asked for them is out.
    wanted: Vec<Request>,
    /// Ids that must not be evicted.
    pinned: HashSet<u64>,
    layout: Layout,
}

/// One cover to load and encode, off the UI thread.
pub struct Request {
    pub cover_id: u64,
    /// The cell area to encode for.
    pub size: Size,
    /// The layout this was asked for in (see [`Layout`]); the load gives up if it is no longer
    /// current.
    pub generation: u64,
}

/// A loaded, encoded cover on its way back to a cache.
pub struct Load {
    pub cover_id: u64,
    pub size: Size,
    pub generation: u64,
    /// `None` if it could not be read or decoded, or if the layout changed while it was being
    /// loaded. Either way it leaves the cover retryable.
    pub protocol: Option<Protocol>,
}

impl Covers {
    pub fn new(covers_dir: Option<PathBuf>) -> Self {
        Covers {
            covers_dir,
            files: HashMap::new(),
            thumbs: Lru::new(THUMB_CAPACITY),
            wanted: Vec::new(),
            pinned: HashSet::new(),
            layout: Layout::new(),
        }
    }

    /// The current layout, for a loader to check against.
    pub fn layout(&self) -> Layout {
        self.layout.clone()
    }

    /// Remembers where a cover's artwork lives. Learnt from the scan, which reports it alongside the
    /// thumbnail.
    pub fn learn_file(&mut self, cover_id: u64, file: Arc<PathBuf>) {
        self.files.insert(cover_id, file);
    }

    /// The covers whose artwork file is already known, so a rescan need not read and re-digest their
    /// thumbnails to tell us a path we are holding (see `library::ScanOptions::known_covers`).
    pub fn known(&self) -> std::collections::HashSet<u64> {
        self.files.keys().copied().collect()
    }

    /// The encoded cover held for `id` at `size`, if there is one. An encoding is good for the area
    /// it was made for and no other, so a lookup at a different size misses rather than stretching.
    pub fn best(&mut self, id: u64, size: Size) -> Option<&Protocol> {
        self.thumbs.get(id).filter(|held| held.size == size).map(|held| &held.protocol)
    }

    /// Asks for a cover, unless it is held at this size already or is on its way. Cheap and
    /// idempotent, so a caller can ask on every frame.
    pub fn want(&mut self, cover_id: u64, size: Size) {
        if size.width == 0 || size.height == 0 || self.covers_dir.is_none() {
            return;
        }
        // A cover encoded for a different size is stale, not held: ask again at the new one.
        if self.thumbs.get(cover_id).is_some_and(|held| held.size != size) {
            self.thumbs.forget(cover_id);
        }
        if self.thumbs.start_loading(cover_id) {
            self.wanted.push(Request { cover_id, size, generation: self.layout.get() });
        }
    }

    /// Names the covers that must stay held.
    pub fn pin(&mut self, ids: impl IntoIterator<Item = u64>) {
        self.pinned = ids.into_iter().collect();
    }

    /// The loads to start, handed to whoever runs them.
    pub fn take_wanted(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.wanted)
    }

    /// Takes in a finished load.
    /// Reports whether anything actually landed in a cache. A load that was abandoned -- stale
    /// layout, or a thumbnail that could not be read -- changes nothing on screen, and drawing a
    /// frame for it is what turns a cover that never loads into a spin: the frame asks for it again,
    /// the load fails again, and the redraw it triggers closes the loop.
    #[must_use]
    pub fn absorb(&mut self, load: Load) -> bool {
        let Load { cover_id, size, generation, protocol } = load;
        match protocol {
            // Encoded for a layout that has since gone. Dropped without touching the in-flight
            // marks, which `clear` already cleared and a fresh request may since have taken -- this
            // load is nobody's outstanding request any more.
            Some(_) if !self.layout.current(generation) => false,
            Some(protocol) => {
                self.thumbs.insert(cover_id, Encoded { protocol, size }, &self.pinned);
                true
            }
            // Leave it uncached and retryable: a thumbnail may appear once the scan writes it.
            None => {
                self.thumbs.give_up(cover_id);
                false
            }
        }
    }

    /// Drops every cached cover, for when they were all encoded for an area that no longer exists --
    /// the terminal having been resized. Without this they would linger, counting against the bound,
    /// until each was asked for again and found stale one at a time.
    pub fn clear(&mut self) {
        // Including what is in flight. Those loads are for the layout that just went, so they will
        // be dropped on arrival -- and leaving them marked would suppress the request at the size
        // now wanted, leaving the pane blank until something unrelated redrew it.
        self.thumbs.abandon();
        self.layout.bump();
    }

    /// The artwork file a cover came from, for anything that wants to point elsewhere at it -- the
    /// desktop's now-playing art, for one.
    pub fn file_of(&self, cover_id: u64) -> Option<PathBuf> {
        self.files.get(&cover_id).map(|file| (**file).clone())
    }

    /// Where thumbnails are read from, for the loader.
    pub fn dir(&self) -> Option<PathBuf> {
        self.covers_dir.clone()
    }
}

/// Loads a cover and encodes it for the area it will be drawn in. The shrinking runs off the UI
/// thread -- a few hundred microseconds, which is not a frame's business.
pub async fn load(dir: Option<PathBuf>, layout: Layout, request: Request) -> Load {
    let Request { cover_id, size, generation } = request;
    let give_up = Load { cover_id, size, generation, protocol: None };
    // Checked before the read, and again before the encode: a resize while this was queued means
    // nothing it produces will be wanted.
    if !layout.current(generation) {
        return give_up;
    }
    let Some(dir) = dir else { return give_up };
    let Some(pixels) = library::read_thumbnail(&dir, cover_id).await else { return give_up };
    let (pixels, edge) = pixels;
    let Some(image) = image::RgbaImage::from_raw(edge, edge, pixels.to_vec()) else { return give_up };
    if !layout.current(generation) {
        return give_up;
    }
    let protocol = smol::unblock(move || {
        // The blocks only: the pixels are dropped here rather than held for as long as the cover is
        // cached.
        picker().new_protocol(image::DynamicImage::ImageRgba8(image), size, Resize::Fit(None)).ok()
    })
    .await;
    Load { cover_id, size, generation, protocol }
}

/// What encodes a cover, and the only thing this module wants from `ratatui-image`.
///
/// The font size is a lie, and deliberately: `new_protocol` fits an image to `cells x font`, and the
/// half-block encoder then fits *that* to `width x height*2`. Declaring a cell 1x2 makes the two
/// agree, so the image is shrunk once. At the usual 10x20 a 256-pixel thumbnail is first blown up to
/// several hundred pixels a side and then crushed back down to a few dozen -- an upscale of half a
/// megabyte, to draw something that was always going to be a grid of blocks.
fn picker() -> Picker {
    // `halfblocks()` is the un-deprecated spelling but hardcodes that 10x20 cell, which is the whole
    // thing being avoided here.
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(1, CELL_ASPECT));
    picker.set_protocol_type(ProtocolType::Halfblocks);
    picker
}

/// Encodes a plain image for `size`, for tests in other modules that need a cover to hand back.
#[cfg(test)]
pub fn encode_for_test(image: image::DynamicImage, size: Size) -> Protocol {
    picker().new_protocol(image, size, Resize::Fit(None)).expect("a plain image encodes")
}

/// The largest cell area within `space` that a square cover fills exactly. Cells are taller than they
/// are wide (see [`CELL_ASPECT`]), so that is twice as many columns as rows.
pub fn square(space: Size) -> Size {
    let width = space.width.min(space.height.saturating_mul(CELL_ASPECT));
    Size::new(width, width / CELL_ASPECT)
}

#[cfg(test)]
mod test {
    use super::*;

    /// An encoded cover of a plain colour, at `size`.
    fn encoded(size: Size) -> Protocol {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(64, 64, image::Rgba([1, 2, 3, 255])));
        picker().new_protocol(image, size, Resize::Fit(None)).expect("a plain image encodes")
    }

    fn covers() -> Covers {
        Covers::new(Some(PathBuf::from("/covers")))
    }

    /// A cover encoded for one area is not drawn in another: the terminal having been resized must
    /// not stretch what was encoded for the old size.
    #[test]
    fn a_cover_is_only_used_at_the_size_it_was_encoded_for() {
        let (small, large) = (Size::new(20, 10), Size::new(40, 20));
        let mut covers = covers();
        let protocol = encoded(small);
        let _ = covers.absorb(Load { cover_id: 7, size: small, generation: 0, protocol: Some(protocol) });

        assert!(covers.best(7, small).is_some(), "held at the size it was encoded for");
        assert!(covers.best(7, large).is_none(), "not at any other");
    }

    /// Asking at a new size discards the entry and asks again, rather than leaving it to be found
    /// stale over and over.
    #[test]
    fn asking_at_a_new_size_reloads() {
        let (small, large) = (Size::new(20, 10), Size::new(40, 20));
        let mut covers = covers();
        let protocol = encoded(small);
        let _ = covers.absorb(Load { cover_id: 7, size: small, generation: 0, protocol: Some(protocol) });
        assert!(covers.take_wanted().is_empty());

        covers.want(7, small);
        assert!(covers.take_wanted().is_empty(), "already held at this size");

        covers.want(7, large);
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
            let protocol = encoded(size);
            let _ = covers.absorb(Load { cover_id: id, size, generation: 0, protocol: Some(protocol) });
        }
        assert!(covers.best(1, size).is_some());

        covers.clear();
        for id in 0..3 {
            assert!(covers.best(id, size).is_none(), "cover {id} should be gone");
        }
        covers.want(1, size);
        assert_eq!(covers.take_wanted().len(), 1, "and is loaded afresh when asked for");
    }

    /// A failed load leaves nothing cached and can be tried again.
    #[test]
    fn a_failed_load_is_retried() {
        let size = Size::new(20, 10);
        let mut covers = covers();
        covers.want(7, size);
        assert_eq!(covers.take_wanted().len(), 1);
        covers.want(7, size);
        assert!(covers.take_wanted().is_empty(), "not asked twice while in flight");

        let _ = covers.absorb(Load { cover_id: 7, size, generation: 0, protocol: None });
        covers.want(7, size);
        assert_eq!(covers.take_wanted().len(), 1, "asked again once the load failed");
    }
}
