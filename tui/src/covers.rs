//! Cover art in the terminal, through whatever image protocol it speaks.
//!
//! One cover is resident at a time -- the one on screen -- so what this costs does not grow with the
//! library. The pixels come from the scan's thumbnail (see [`library::THUMB`]), already decoded, so
//! showing a cover is a resize and an encode, never an image decode.

use phonoscule::library::{self, CoverArt};
use ratatui::layout::Size;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

/// The cover currently held for display, and which album it belongs to.
pub struct Cover {
    pub album: u64,
    pub protocol: StatefulProtocol,
}

/// Asks the terminal what it can do. Must run after the alternate screen is up but before terminal
/// events are read, since it writes a query to stdout and reads the reply from stdin.
///
/// A terminal that answers nothing gets half blocks, which need no protocol at all.
pub fn picker() -> Picker {
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

/// Builds the displayable cover for `art`, or `None` if the thumbnail isn't the size it claims.
pub fn build(picker: &Picker, album: u64, art: &CoverArt) -> Option<Cover> {
    let pixels = image::RgbaImage::from_raw(library::THUMB, library::THUMB, art.pixels.to_vec())?;
    Some(Cover { album, protocol: picker.new_resize_protocol(image::DynamicImage::ImageRgba8(pixels)) })
}

/// The area, in cells, to draw a cover of `width` cells in: as tall as it is wide *in pixels*, so
/// the artwork comes out square rather than stretched by the cell aspect ratio.
pub fn square(picker: &Picker, width: u16) -> Size {
    let font = picker.font_size();
    let height = (u32::from(width) * u32::from(font.width)).div_ceil(u32::from(font.height.max(1)));
    Size::new(width, height.try_into().unwrap_or(u16::MAX))
}
