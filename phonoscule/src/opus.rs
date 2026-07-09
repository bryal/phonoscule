//! OGG Opus reader
//!
//! Demuxes the Ogg container with the `ogg` crate's push-based page parser and decodes the Opus
//! stream with the pure-Rust `opuscule` decoder. Only requires an (embedded-io) async Reader as
//! input.
//!
//! Assumptions / limitations:
//! - Channel mapping family 0 only, i.e. mono or stereo. Surround would need `opuscule`'s
//!   multistream decoder.
//! - A single logical bitstream; chained or multiplexed Ogg streams are not supported.
//! - The total stream length is not known up front. That would require seeking to the last Ogg
//!   page for its granule position, and we only require `Read`.
//! - Opus frames depend on decoder history, so [`FastForward`] decodes and discards rather than
//!   skips.

use crate::{
    metadata::Metadata,
    plumbing::{FastForward, Source},
    sample::{PcmS16Le, Stereo},
};
use core::cmp::min;
use embedded_io_async::Read;
use ogg::reading::{BasePacketReader, OggPage, PageParser};
use opuscule::{Channels, Decoder, SampleRate, Val, sample_to_i16};

/// Opus always decodes at 48 kHz, regardless of the input's original sample rate.
pub const SAMPLE_RATE: u32 = 48_000;
/// Largest decodable frame: 120 ms at 48 kHz, per channel.
const MAX_FRAME: usize = 5760;

pub struct OggOpus<Md, R> {
    pub metadata: Md,
    pub format: Format,
    pub samples: OpusReader<R>,
}

#[derive(Clone, Debug)]
pub struct Format {
    pub n_channels: u16,
    /// Encoder delay dropped from the front of the stream, in 48 kHz samples.
    pub pre_skip: u16,
}

impl Format {
    pub fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
}

impl<Md, R> OggOpus<Md, R>
where
    Md: Metadata,
    R: Read,
{
    pub async fn parse(mut inp: R) -> Option<Self> {
        let mut packets = BasePacketReader::new();
        // The first two packets of an Ogg Opus stream are the headers: OpusHead, then OpusTags.
        let head = next_packet(&mut packets, &mut inp).await??;
        let (channels, pre_skip) = parse_opus_head(&head.data)?;
        let tags = next_packet(&mut packets, &mut inp).await??;
        let mut metadata = Md::default();
        parse_opus_tags(&mut metadata, &tags.data);
        let format = Format { n_channels: channels.count() as u16, pre_skip };
        let samples = OpusReader {
            packets,
            inp,
            decoder: Decoder::new(SampleRate::Hz48000, channels),
            channels,
            frame: [0 as Val; MAX_FRAME * 2],
            frame_len: 0,
            frame_pos: 0,
            pre_skip: pre_skip as usize,
            ended: false,
        };
        Some(OggOpus { metadata, format, samples })
    }
}

pub struct OpusReader<R> {
    packets: BasePacketReader,
    inp: R,
    decoder: Decoder,
    channels: Channels,
    /// The current decoded frame, interleaved.
    frame: [Val; MAX_FRAME * 2],
    /// Length of the current frame, in values (samples * channels).
    frame_len: usize,
    /// Read cursor into `frame`, in values.
    frame_pos: usize,
    /// Remaining encoder delay to drop from the front of the stream, in samples.
    pre_skip: usize,
    ended: bool,
}

impl<R: Read> OpusReader<R> {
    /// Decodes the next packet into `frame`. Returns `Some(false)` at the end of the stream.
    async fn next_frame(&mut self) -> Option<bool> {
        loop {
            if self.ended {
                return Some(false);
            }
            let packet = match next_packet(&mut self.packets, &mut self.inp).await? {
                Some(packet) => packet,
                None => {
                    self.ended = true;
                    return Some(false);
                }
            };
            let nsamples = match self.decoder.decode(Some(&packet.data), &mut self.frame, false) {
                Ok(n) => n,
                Err(e) => {
                    log::error!("opus decode error: {e:?}");
                    return None;
                }
            };
            let ch = self.channels.count();
            self.frame_len = nsamples * ch;
            self.frame_pos = 0;
            if self.pre_skip > 0 {
                let nskip = min(self.pre_skip, nsamples);
                self.frame_pos = nskip * ch;
                self.pre_skip -= nskip;
            }
            if self.frame_pos < self.frame_len {
                return Some(true);
            }
        }
    }
}

impl<R: Read> Source<Stereo<PcmS16Le>> for OpusReader<R> {
    async fn read_samples(&mut self, buf: &mut [Stereo<PcmS16Le>]) -> Option<usize> {
        if self.frame_pos >= self.frame_len && !self.next_frame().await? {
            return Some(0);
        }
        let ch = self.channels.count();
        let avail = (self.frame_len - self.frame_pos) / ch;
        let n = min(avail, buf.len());
        for out in buf[..n].iter_mut() {
            let left = sample_to_i16(self.frame[self.frame_pos]);
            let right = if ch == 2 { sample_to_i16(self.frame[self.frame_pos + 1]) } else { left };
            *out = Stereo::new(PcmS16Le::new(left), PcmS16Le::new(right));
            self.frame_pos += ch;
        }
        Some(n)
    }
}

impl<R: Read> FastForward for OpusReader<R> {
    async fn fast_forward(&mut self, nsamples: u64) -> Option<u64> {
        let ch = self.channels.count();
        let mut remaining = nsamples;
        while remaining > 0 {
            if self.frame_pos < self.frame_len {
                let avail = ((self.frame_len - self.frame_pos) / ch) as u64;
                let ntake = min(avail, remaining);
                self.frame_pos += ntake as usize * ch;
                remaining -= ntake;
            } else if !self.next_frame().await? {
                break;
            }
        }
        Some(nsamples - remaining)
    }
}

/// Reads the next packet, pulling Ogg pages from `inp` as needed.
///
/// Returns `None` on error (logged), `Some(None)` at a clean end of stream.
async fn next_packet<R: Read>(packets: &mut BasePacketReader, inp: &mut R) -> Option<Option<ogg::Packet>> {
    loop {
        if let Some(packet) = packets.read_packet() {
            return Some(Some(packet));
        }
        let page = match read_page(inp).await? {
            Some(page) => page,
            None => return Some(None),
        };
        if let Err(e) = packets.push_page(page) {
            log::error!("bad ogg page: {e}");
            return None;
        }
    }
}

/// Reads one Ogg page. Returns `None` on error (logged), `Some(None)` at a clean end of stream.
async fn read_page<R: Read>(inp: &mut R) -> Option<Option<OggPage>> {
    let mut header = [0u8; 27];
    let mut nread = 0;
    while nread < header.len() {
        match inp.read(&mut header[nread..]).await {
            Ok(0) if nread == 0 => return Some(None), // end of stream between two pages
            Ok(0) => {
                log::error!("unexpected end of stream inside an ogg page header");
                return None;
            }
            Ok(n) => nread += n,
            Err(e) => {
                log::error!("read error in ogg page header: {e:?}");
                return None;
            }
        }
    }
    let (mut parser, n_segments) = match PageParser::new(header) {
        Ok(x) => x,
        Err(e) => {
            log::error!("bad ogg page header: {e}");
            return None;
        }
    };
    let mut segments = vec![0u8; n_segments];
    read_exact_logged(inp, &mut segments).await?;
    let n_data = parser.parse_segments(segments);
    let mut data = vec![0u8; n_data];
    read_exact_logged(inp, &mut data).await?;
    match parser.parse_packet_data(data) {
        Ok(page) => Some(Some(page)),
        Err(e) => {
            log::error!("bad ogg page: {e}");
            None
        }
    }
}

async fn read_exact_logged<R: Read>(inp: &mut R, buf: &mut [u8]) -> Option<()> {
    match inp.read_exact(buf).await {
        Ok(()) => Some(()),
        Err(e) => {
            log::error!("read error inside ogg page: {e:?}");
            None
        }
    }
}

/// Parses an `OpusHead` identification header (RFC 7845 §5.1) into the channel layout and the
/// pre-skip sample count.
fn parse_opus_head(data: &[u8]) -> Option<(Channels, u16)> {
    if data.len() < 19 || &data[..8] != b"OpusHead" {
        log::error!("first ogg packet is not OpusHead");
        return None;
    }
    let version = data[8];
    if version >> 4 != 0 {
        log::error!("incompatible OpusHead version {version}");
        return None;
    }
    let mapping_family = data[18];
    if mapping_family != 0 {
        log::error!("unsupported channel mapping family {mapping_family} (only mono/stereo is supported)");
        return None;
    }
    let channels = match data[9] {
        1 => Channels::Mono,
        2 => Channels::Stereo,
        n => {
            log::error!("unsupported channel count {n}");
            return None;
        }
    };
    let pre_skip = u16::from_le_bytes([data[10], data[11]]);
    Some((channels, pre_skip))
}

/// Parses an `OpusTags` packet (RFC 7845 §5.2, Vorbis comments) into `metadata`. Unknown or
/// malformed comments are skipped; a malformed packet just leaves the remaining fields empty.
fn parse_opus_tags(metadata: &mut impl Metadata, data: &[u8]) {
    fn read_u32(data: &[u8], pos: &mut usize) -> Option<usize> {
        let bs = data.get(*pos..*pos + 4)?;
        *pos += 4;
        Some(u32::from_le_bytes([bs[0], bs[1], bs[2], bs[3]]) as usize)
    }
    fn read_bytes<'a>(data: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
        let len = read_u32(data, pos)?;
        let bs = data.get(*pos..*pos + len)?;
        *pos += len;
        Some(bs)
    }
    let mut parse = || -> Option<()> {
        if data.get(..8)? != b"OpusTags" {
            log::warn!("second ogg packet is not OpusTags");
            return None;
        }
        let mut pos = 8;
        let _vendor = read_bytes(data, &mut pos)?;
        let n_comments = read_u32(data, &mut pos)?;
        for _ in 0..n_comments {
            let comment = read_bytes(data, &mut pos)?;
            let Ok(comment) = core::str::from_utf8(comment) else {
                log::trace!("skipped non-utf8 comment");
                continue;
            };
            let Some((key, value)) = comment.split_once('=') else {
                log::trace!("skipped malformed comment {comment:?}");
                continue;
            };
            if key.eq_ignore_ascii_case("TITLE") {
                metadata.set_title(value)
            } else if key.eq_ignore_ascii_case("ARTIST") {
                metadata.set_artist(value)
            } else if key.eq_ignore_ascii_case("ALBUM") {
                metadata.set_album(value)
            } else {
                log::trace!("ignored comment {key}");
            }
        }
        Some(())
    };
    if parse().is_none() {
        log::warn!("truncated or malformed OpusTags packet");
    }
}
