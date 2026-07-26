//! Cover art in the terminal, through whatever image protocol it speaks.
//!
//! One cover is resident at a time -- the one on screen -- so what this costs does not grow with the
//! library. The pixels come from the scan's thumbnail (see [`library::THUMB`]), already decoded, so
//! showing a cover is a resize and an encode, never an image decode.

use image::imageops::FilterType;
use phonoscule::library::{self, CoverArt};
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;

/// The cover currently held for display, and which album it belongs to.
pub struct Cover {
    pub album: u64,
    pub protocol: StatefulProtocol,
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

/// Builds the displayable cover for `art`, or `None` if the thumbnail isn't the size it claims.
pub fn build(picker: &Picker, album: u64, art: &CoverArt) -> Option<Cover> {
    let pixels = image::RgbaImage::from_raw(library::THUMB, library::THUMB, art.pixels.to_vec())?;
    Some(Cover { album, protocol: picker.new_resize_protocol(image::DynamicImage::ImageRgba8(pixels)) })
}

/// How a cover is fitted to the space it is given. `Scale` rather than `Fit`, which clamps to the
/// source resolution and so would leave a thumbnail sitting at its own 320 pixels in the middle of a
/// larger pane instead of filling it. Bilinear, since that upscaling is otherwise blocky.
pub fn resize() -> Resize {
    Resize::Scale(Some(FilterType::Triangle))
}

impl Cover {
    /// The area the artwork will actually occupy within `space`, proportions kept. Asked of the
    /// protocol rather than worked out here: only it knows how its pixels map onto cells.
    pub fn size_in(&self, space: Size) -> Size {
        self.protocol.size_for(resize(), space)
    }
}

/// The largest square area, in cells, fitting within `space` -- for the placeholder shown when there
/// is no artwork to ask. Square *in pixels*: cells are about twice as tall as they are wide, so a
/// square block of cells comes out stretched.
pub fn square(picker: &Picker, space: Size) -> Size {
    let font = picker.font_size();
    let (fw, fh) = (u32::from(font.width.max(1)), u32::from(font.height.max(1)));
    let width = u32::from(space.width).min(u32::from(space.height) * fh / fw);
    let height = width * fw / fh;
    Size::new(width.try_into().unwrap_or(u16::MAX), height.try_into().unwrap_or(u16::MAX))
}
