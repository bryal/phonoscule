//! Cover art in the terminal, as half blocks.
//!
//! Half blocks and nothing else, deliberately. One cell carries two colours, so a cover is at most
//! `width` by `height * 2` pixels however grand the terminal's artwork protocol might have been --
//! a preview pane is a few dozen cells across, which is a few dozen pixels.
//!
//! That smallness shapes everything here. The whole library's covers, as the encoded bytes the cache
//! stores, come to a few tens of megabytes -- less than a hundred *decoded* covers would -- so they
//! are simply held, and drawing one never waits on a disk that may well be an SD card. Turning one
//! into blocks is around a tenth of a millisecond: too cheap to be worth a thread, a message and a
//! cache of its own, and cheap enough to do while building the frame that wants it.
//!
//! What is worth avoiding is doing it *again* for a frame that wants the same cover at the same size,
//! which a ticking clock asks for about once a second. Hence [`CoverMemo`]: one per pane, holding
//! what that pane last drew.

use phonoscule::library;
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

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

/// Every cover the library has, and where each came from.
pub struct Covers {
    /// Where the encoded covers are read from, or `None` if there is no cache directory -- in which
    /// case covers never appear and the accent colours stand in for good.
    covers_dir: Option<PathBuf>,
    /// Each cover's artwork file. Not for drawing -- half blocks never want more pixels than the
    /// cache holds -- but for pointing other programs at it (the desktop's now-playing art), and for
    /// telling a rescan which covers it need not read back.
    files: HashMap<u64, Arc<PathBuf>>,
    /// Each cover as it sits on disk, encoded. See the module docs for why all of them.
    encoded: HashMap<u64, Arc<[u8]>>,
}

impl Covers {
    pub fn new(covers_dir: Option<PathBuf>) -> Self {
        Covers { covers_dir, files: HashMap::new(), encoded: HashMap::new() }
    }

    /// Where the encoded covers live, for whoever reads them.
    pub fn dir(&self) -> Option<PathBuf> {
        self.covers_dir.clone()
    }

    /// Takes a cover the scan has reported: where its artwork lives, and the bytes to draw it from.
    pub fn learn(&mut self, cover_id: u64, file: Arc<PathBuf>, encoded: Arc<[u8]>) {
        self.files.insert(cover_id, file);
        self.encoded.insert(cover_id, encoded);
    }

    /// The covers already held, so a rescan need not read and re-digest them to hand back what we
    /// have (see [`library::ScanOptions::known_covers`]).
    pub fn known(&self) -> HashSet<u64> {
        self.files.keys().copied().collect()
    }

    /// The artwork file a cover came from, for anything that wants to point elsewhere at it -- the
    /// desktop's now-playing art, for one.
    pub fn file_of(&self, cover_id: u64) -> Option<PathBuf> {
        self.files.get(&cover_id).map(|file| (**file).clone())
    }

    /// Builds a cover as half blocks filling `size`. `None` if it is not held, or will not decode.
    ///
    /// Around a tenth of a millisecond, which is cheap enough to do while a frame is being built --
    /// but not free, so callers hold the result rather than ask twice (see [`CoverMemo`]).
    fn render(&self, cover_id: u64, size: Size) -> Option<Protocol> {
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let encoded = self.encoded.get(&cover_id)?;
        let image = image::load_from_memory_with_format(encoded, library::THUMB_FORMAT).ok()?;
        picker().new_protocol(image, size, Resize::Fit(None)).ok()
    }
}

/// The cover one pane last drew, so that redrawing the same album at the same size does not build the
/// same blocks again -- a ticking clock redraws the frame about once a second, and nothing about the
/// picture has changed.
#[derive(Default)]
pub struct CoverMemo {
    held: Option<(u64, Size, Protocol)>,
}

impl CoverMemo {
    /// The blocks for `cover_id` at `size`, building them if what is held is for another album or
    /// another area. `None` when there is nothing to draw, which is the caller's cue to fall back to
    /// the album's accent colour.
    pub fn get(&mut self, covers: &Covers, cover_id: Option<u64>, size: Size) -> Option<&Protocol> {
        let cover_id = cover_id?;
        let fresh = matches!(&self.held, Some((id, held, _)) if *id == cover_id && *held == size);
        if !fresh {
            self.held = covers.render(cover_id, size).map(|protocol| (cover_id, size, protocol));
        }
        self.held.as_ref().map(|(_, _, protocol)| protocol)
    }
}

/// What encodes a cover, and the only thing this module wants from `ratatui-image`.
///
/// The font size is a lie, and deliberately: `new_protocol` fits an image to `cells x font`, and the
/// half-block encoder then fits *that* to `width x height*2`. Declaring a cell 1x2 makes the two
/// agree, so the image is shrunk once. At the usual 10x20 a cached cover is first blown up to several
/// hundred pixels a side and then crushed back down to a few dozen -- an upscale of half a megabyte,
/// to draw something that was always going to be a grid of blocks.
fn picker() -> Picker {
    // `halfblocks()` is the un-deprecated spelling but hardcodes that 10x20 cell, which is the whole
    // thing being avoided here.
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(1, CELL_ASPECT));
    picker.set_protocol_type(ProtocolType::Halfblocks);
    picker
}

/// The largest cell area within `space` that a square cover fills exactly. Cells are taller than they
/// are wide (see [`CELL_ASPECT`]), so that is twice as many columns as rows.
pub fn square(space: Size) -> Size {
    let width = space.width.min(space.height.saturating_mul(CELL_ASPECT));
    Size::new(width, width / CELL_ASPECT)
}

/// A cover encoded the way the cache stores one, for tests that need a library with artwork in it.
///
/// A vertical gradient rather than a flat colour, so the two halves of a cell differ and the encoder
/// actually emits half blocks -- given one colour it would emit spaces with a background, and a test
/// looking for blocks would find none.
#[cfg(test)]
pub fn test_cover_bytes() -> Arc<[u8]> {
    let img = image::RgbImage::from_fn(COVER_EDGE, COVER_EDGE, |_, y| image::Rgb([(y * 2) as u8, 40, 255 - (y * 2) as u8]));
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img).write_to(&mut encoded, library::THUMB_FORMAT).expect("encodes");
    Arc::from(encoded.into_inner())
}

#[cfg(test)]
mod test {
    use super::*;

    fn covers() -> Covers {
        let mut covers = Covers::new(Some(PathBuf::from("/covers")));
        covers.learn(7, Arc::new("/cover.jpg".into()), test_cover_bytes());
        covers
    }

    /// The point of the memo: the same album at the same size is built once, however many frames ask.
    #[test]
    fn a_repeated_ask_builds_nothing() {
        let covers = covers();
        let mut memo = CoverMemo::default();
        let size = Size::new(20, 10);

        let first = memo.get(&covers, Some(7), size).expect("a held cover draws") as *const Protocol;
        let again = memo.get(&covers, Some(7), size).expect("and still draws") as *const Protocol;
        assert_eq!(first, again, "the same blocks, not a freshly built set");
    }

    /// An encoding is good for the area it was built for and no other, so a resized pane rebuilds
    /// rather than stretching what it had.
    #[test]
    fn a_new_size_rebuilds() {
        let covers = covers();
        let mut memo = CoverMemo::default();

        let small = memo.get(&covers, Some(7), Size::new(20, 10)).expect("draws").size();
        let large = memo.get(&covers, Some(7), Size::new(40, 20)).expect("draws").size();
        assert_ne!(small, large, "the blocks were rebuilt for the new area");
    }

    /// A cover the scan has not reported has nothing to draw, and says so rather than leaving the
    /// previous album's picture up.
    #[test]
    fn an_unknown_cover_draws_nothing() {
        let covers = covers();
        let mut memo = CoverMemo::default();
        let size = Size::new(20, 10);

        assert!(memo.get(&covers, Some(7), size).is_some());
        assert!(memo.get(&covers, Some(999), size).is_none(), "not held");
        assert!(memo.get(&covers, None, size).is_none(), "no cover at all");
    }
}
