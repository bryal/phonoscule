//! Cover art in the terminal, through whatever image protocol it speaks.
//!
//! Covers are kept in a bounded cache (see the cache module), so what this costs does not grow with
//! the size of the library -- the point of the player, which is meant for machines that have not got
//! the memory to hold a library's worth of artwork.
//!
//! What is cached is the *encoded* cover, ready for the terminal, because that is the expensive part:
//! reading a thumbnail off disk costs tens of microseconds and building a protocol from it about as
//! much, while the resize and encode behind them cost a few milliseconds. So it happens off the UI
//! thread and the covers arrive as messages ([`Load`]); until one does, a view draws the album's
//! accent colour, which is known long before any pixels are.

use crate::cache::Lru;
use image::imageops::FilterType;
use phonoscule::library;
use ratatui::layout::Size;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, ResizeEncodeRender};
use std::collections::HashSet;
use std::path::PathBuf;

/// How many encoded covers are held. Thirty-two of them is twelve megabytes of half blocks or fifty
/// of kitty, which is a fair price for scrolling back over ground just covered; the library's own
/// size does not enter into it.
const THUMB_CAPACITY: usize = 32;

/// How far either side of the browser's cursor covers are loaded before they are asked for, and kept
/// from being evicted. Small, because a cover that is not there yet costs nothing but a coloured
/// block: this is for the neighbours a single keypress reaches, not for guessing where the user is
/// headed.
pub const PIN_RADIUS: usize = 2;

/// The covers held for display, and the terminal's way of drawing them.
pub struct Covers {
    pub picker: Picker,
    /// Where thumbnails are read from, or `None` if there is no cache directory to read (in which
    /// case covers simply never appear -- see [`want`](Self::want)).
    covers_dir: Option<PathBuf>,
    thumbs: Lru<StatefulProtocol>,
    /// Covers to load, taken by the event loop after each round of messages.
    wanted: Vec<Request>,
    /// Ids that must not be evicted: the browser's cursor and its neighbours.
    pinned: HashSet<u64>,
}

/// One cover to load and encode, off the UI thread.
pub struct Request {
    pub cover_id: u64,
    /// The area it is being encoded for. An encoding is good for one size, so this is part of the
    /// request rather than settled afterwards.
    pub size: Size,
}

/// A loaded, encoded cover on its way back to the cache.
pub struct Load {
    pub cover_id: u64,
    /// `None` if the thumbnail could not be read -- never cached, or the file went away.
    pub protocol: Option<StatefulProtocol>,
}

impl Covers {
    pub fn new(picker: Picker, covers_dir: Option<PathBuf>) -> Self {
        Covers { picker, covers_dir, thumbs: Lru::new(THUMB_CAPACITY), wanted: Vec::new(), pinned: HashSet::new() }
    }

    /// The encoded cover for `id`, if it is held. Marks it as just used, so it is the last thing
    /// evicted.
    pub fn get(&mut self, id: u64) -> Option<&mut StatefulProtocol> {
        self.thumbs.get(id)
    }

    /// Asks for a cover to be loaded and encoded for `size`, unless it is already held or on its way.
    /// Cheap and idempotent, so a caller can ask on every frame.
    pub fn want(&mut self, cover_id: u64, size: Size) {
        if self.covers_dir.is_none() || size.width == 0 || size.height == 0 {
            return;
        }
        if self.thumbs.start_loading(cover_id) {
            self.wanted.push(Request { cover_id, size });
        }
    }

    /// Names the covers that must stay held: the ones a single keypress can reach.
    pub fn pin(&mut self, ids: impl IntoIterator<Item = u64>) {
        self.pinned = ids.into_iter().collect();
    }

    /// The loads to start, handed to whoever runs them.
    pub fn take_wanted(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.wanted)
    }

    /// Takes in a finished load.
    pub fn absorb(&mut self, load: Load) {
        match load.protocol {
            Some(protocol) => self.thumbs.insert(load.cover_id, protocol, &self.pinned),
            // Leave it uncached and retryable: the thumbnail may appear once the scan writes it.
            None => self.thumbs.give_up(load.cover_id),
        }
    }

    /// Where thumbnails are read from, for the loader.
    pub fn dir(&self) -> Option<PathBuf> {
        self.covers_dir.clone()
    }
}

/// Reads a cached thumbnail and encodes it for `size`. The expensive half runs on the blocking pool:
/// a few milliseconds of resizing and encoding has no business on the thread drawing frames.
pub async fn load(picker: Picker, dir: PathBuf, request: Request) -> Load {
    let Request { cover_id, size } = request;
    let Some(pixels) = library::read_thumbnail(&dir, cover_id).await else {
        return Load { cover_id, protocol: None };
    };
    let protocol = smol::unblock(move || {
        let image = image::RgbaImage::from_raw(library::THUMB, library::THUMB, pixels.to_vec())?;
        let mut protocol = picker.new_resize_protocol(image::DynamicImage::ImageRgba8(image));
        // Encode here, rather than leaving the first render to do it.
        protocol.resize_encode(&resize(), size);
        Some(protocol)
    })
    .await;
    Load { cover_id, protocol }
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
