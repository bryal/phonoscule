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
//! - With [`OggOpus::parse`] the total stream length is not known up front ([`Format::len_samples`]
//!   is `None`): the length lives in the granule position of the *last* Ogg page, and `parse` only
//!   requires `Read`. Use [`OggOpus::parse_seekable`] to get it via a scan of the stream's tail.
//! - Opus frames depend on decoder history, so [`FastForward`] decodes and discards rather than
//!   skips.

use crate::{
    metadata::Metadata,
    plumbing::{FastForward, Source},
    sample::{PcmS16Le, Stereo},
};
use core::cmp::min;
use embedded_io::SeekFrom;
use embedded_io_async::{Read, Seek};
use ogg::reading::{BasePacketReader, OggPage, PageParser};
use opuscule::{Channels, Decoder, SampleRate, Val, sample_to_i16};

/// Opus always decodes at 48 kHz, regardless of the input's original sample rate.
pub const SAMPLE_RATE: u32 = 48_000;
/// Largest decodable frame: 120 ms at 48 kHz, per channel.
const MAX_FRAME: usize = 5760;
/// An Ogg page is at most this many bytes (header + segment table + maximal body).
const MAX_PAGE_SIZE: u64 = 27 + 255 + 255 * 255;

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
    /// Total number of samples in the stream, when known (see [`OggOpus::parse_seekable`]).
    pub len_samples: Option<u64>,
}

impl Format {
    pub fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
}

/// The parsed Ogg Opus headers: metadata and format, plus what's needed to continue into
/// decoding. Parsing only this is much cheaper than a full [`OggOpus::parse`] (no decoder state,
/// no frame buffer), which matters when scanning a library for tags.
pub struct Headers<Md> {
    pub metadata: Md,
    pub format: Format,
    channels: Channels,
    serial: u32,
    packets: BasePacketReader,
}

impl<Md: Metadata> Headers<Md> {
    pub async fn parse<R: Read>(inp: &mut R) -> Option<Self> {
        let mut packets = BasePacketReader::new();
        // The first two packets of an Ogg Opus stream are the headers: OpusHead, then OpusTags.
        let head = next_packet(&mut packets, inp).await??;
        let (channels, pre_skip) = parse_opus_head(&head.data)?;
        let tags = next_packet(&mut packets, inp).await??;
        let mut metadata = Md::default();
        parse_opus_tags(&mut metadata, &tags.data);
        let format = Format { n_channels: channels.count() as u16, pre_skip, len_samples: None };
        Some(Headers { metadata, format, channels, serial: head.stream_serial(), packets })
    }
}

impl<Md, R> OggOpus<Md, R>
where
    Md: Metadata,
    R: Read,
{
    pub async fn parse(mut inp: R) -> Option<Self> {
        let headers = Headers::parse(&mut inp).await?;
        let Headers { metadata, format, channels, serial, packets } = headers;
        let samples = OpusReader {
            packets,
            inp,
            serial,
            decoder: Box::new(Decoder::new(SampleRate::Hz48000, channels)),
            channels,
            frame: Box::new([0 as Val; MAX_FRAME * 2]),
            frame_len: 0,
            frame_pos: 0,
            pre_skip: format.pre_skip as usize,
            total_pre_skip: format.pre_skip as u64,
            ended: false,
        };
        Some(OggOpus { metadata, format, samples })
    }

    /// Like [`OggOpus::parse`], but additionally determines [`Format::len_samples`] by scanning
    /// the tail of the stream for the last Ogg page and reading its granule position. Costs one
    /// extra read of up to ~64 KiB plus three seeks. If the scan fails the stream still plays,
    /// just with an unknown length.
    pub async fn parse_seekable(inp: R) -> Option<Self>
    where
        R: Seek,
    {
        let mut this = Self::parse(inp).await?;
        let samples = &mut this.samples;
        match scan_len_samples(&mut samples.inp, samples.serial, this.format.pre_skip).await {
            Some(len) => this.format.len_samples = Some(len),
            None => log::warn!("could not determine the stream length from the last ogg page"),
        }
        Some(this)
    }
}

/// Scans the tail of the stream for its last valid Ogg page and derives the total sample count
/// from its granule position (RFC 7845 §4). Restores the stream position afterwards.
async fn scan_len_samples<R: Read + Seek>(inp: &mut R, serial: u32, pre_skip: u16) -> Option<u64> {
    let pos = inp.stream_position().await.ok()?;
    let end = inp.seek(SeekFrom::End(0)).await.ok()?;
    // The file's last page ends at the end of the stream, so it starts within this window.
    let tail_start = end.saturating_sub(MAX_PAGE_SIZE);
    inp.seek(SeekFrom::Start(tail_start)).await.ok()?;
    let mut tail = vec![0u8; (end - tail_start) as usize];
    let read_result = inp.read_exact(&mut tail).await;
    // Rewind before anything else, so a scan failure leaves the reader usable.
    inp.seek(SeekFrom::Start(pos)).await.ok()?;
    read_result.ok()?;
    last_granule(&tail, serial).map(|granule| granule.saturating_sub(pre_skip as u64))
}

/// Finds the last valid Ogg page of stream `serial` in `tail` and returns its granule position.
fn last_granule(tail: &[u8], serial: u32) -> Option<u64> {
    for i in (0..tail.len().saturating_sub(27)).rev() {
        if &tail[i..i + 4] != b"OggS" {
            continue;
        }
        match page_granule(&tail[i..], serial) {
            // A granule of -1 means no packet ends on this page; keep looking at earlier pages.
            Some(granule) if granule != u64::MAX => return Some(granule),
            _ => continue,
        }
    }
    None
}

/// Returns the granule position of the Ogg page at the start of `page`, or `None` if it isn't a
/// complete, checksum-valid page belonging to stream `serial`. The `OggS` capture pattern can
/// appear in packet payloads, so candidates must be validated this strictly.
fn page_granule(page: &[u8], serial: u32) -> Option<u64> {
    let header: [u8; 27] = page.get(..27)?.try_into().ok()?;
    let page_serial = u32::from_le_bytes([header[14], header[15], header[16], header[17]]);
    if page_serial != serial {
        return None;
    }
    let granule = u64::from_le_bytes(header[6..14].try_into().ok()?);
    let (mut parser, n_segments) = PageParser::new(header).ok()?;
    let segments = page.get(27..27 + n_segments)?.to_vec();
    let n_data = parser.parse_segments(segments);
    let data = page.get(27 + n_segments..27 + n_segments + n_data)?.to_vec();
    parser.parse_packet_data(data).ok()?; // verifies the checksum
    Some(granule)
}

pub struct OpusReader<R> {
    packets: BasePacketReader,
    inp: R,
    /// Serial number of the logical bitstream we are decoding.
    serial: u32,
    // The decoder and frame buffer are boxed to keep `OpusReader` itself small: it moves through
    // async fns whose futures would otherwise embed multiple ~70 KiB copies of it -- enough to
    // overflow a default 2 MiB thread stack in debug builds.
    decoder: Box<Decoder>,
    channels: Channels,
    /// The current decoded frame, interleaved.
    frame: Box<[Val; MAX_FRAME * 2]>,
    /// Length of the current frame, in values (samples * channels).
    frame_len: usize,
    /// Read cursor into `frame`, in values.
    frame_pos: usize,
    /// Remaining encoder delay to drop from the front of the stream, in samples.
    pre_skip: usize,
    /// The stream's full encoder delay (granule positions include it).
    total_pre_skip: u64,
    ended: bool,
}

impl<R: Read + Seek> OpusReader<R> {
    /// Seeks to an absolute sample position (the unit of [`Format::len_samples`] and of the
    /// sample counts this reader outputs), by bisecting over the granule positions of the Ogg
    /// pages and then decoding at least 80 ms of pre-roll before the target so the (predictive)
    /// decoder converges, as RFC 7845 §4.4 recommends.
    ///
    /// Returns the position actually landed on: `target`, unless the stream ends earlier or the
    /// container has a gap right at the target. On `None`, the reader may be left mid-stream and
    /// should be discarded.
    pub async fn seek_samples(&mut self, target: u64) -> Option<u64> {
        /// 80 ms at 48 kHz.
        const PREROLL: u64 = 3840;
        let target_granule = target + self.total_pre_skip;
        let search_granule = target_granule.saturating_sub(PREROLL);

        // Bisect over byte offsets for the latest page with granule <= search_granule; "the
        // granule of the first audio page after an offset" is non-decreasing in the offset.
        let end = self.inp.seek(SeekFrom::End(0)).await.ok()?;
        let (mut lo, mut hi) = (0, end);
        while hi.saturating_sub(lo) > 2 * MAX_PAGE_SIZE {
            let mid = lo + (hi - lo) / 2;
            match probe_page(&mut self.inp, mid, self.serial).await {
                Some((_, granule)) if granule <= search_granule => lo = mid,
                _ => hi = mid,
            }
        }

        let (page_offset, landing_granule) = match probe_page(&mut self.inp, lo, self.serial).await {
            Some((offset, granule)) if granule <= search_granule => (offset, granule),
            // The target lies within the first pages of the stream (or probing failed entirely):
            // decoding from the very start is just as cheap.
            _ => {
                self.restart().await?;
                return self.fast_forward(target).await;
            }
        };

        // Start over demuxing & decoding from the landing page.
        self.inp.seek(SeekFrom::Start(page_offset)).await.ok()?;
        self.packets = BasePacketReader::new();
        self.packets.update_after_seek();
        *self.decoder = Decoder::new(SampleRate::Hz48000, self.channels);
        self.frame_len = 0;
        self.frame_pos = 0;
        self.pre_skip = 0;
        self.ended = false;

        // Decode up to the target, discarding output: this is the pre-roll. The position is only
        // known exactly where a page ends (its granule); in between, decoded samples are counted.
        // Packets of the landing page itself have no known position yet (`None`), but they all
        // end at or before its granule <= search_granule, so discarding them entirely is right.
        let ch = self.channels.count();
        let mut pos: Option<u64> = None;
        loop {
            let packet = match next_packet(&mut self.packets, &mut self.inp).await? {
                Some(packet) => packet,
                None => {
                    // The target is at or past the end of the stream.
                    self.ended = true;
                    return Some(pos.unwrap_or(landing_granule).saturating_sub(self.total_pre_skip));
                }
            };
            let nsamples = match self.decoder.decode(Some(&packet.data), &mut self.frame[..], false) {
                Ok(n) => n,
                Err(e) => {
                    log::error!("opus decode error while seeking: {e:?}");
                    return None;
                }
            };
            pos = match (pos, packet.last_in_page(), packet.absgp_page()) {
                (_, true, granule) if granule != u64::MAX => Some(granule),
                (Some(p), _, _) => Some(p + nsamples as u64),
                (None, _, _) => None,
            };
            if let Some(p) = pos
                && p > target_granule
            {
                // The target is inside this frame: keep its tail.
                let frame_start = p - nsamples as u64;
                let actual = frame_start.max(target_granule);
                self.frame_len = nsamples * ch;
                self.frame_pos = (actual - frame_start) as usize * ch;
                return Some(actual.saturating_sub(self.total_pre_skip));
            }
        }
    }

    /// Rewinds to the start of the audio, resetting all decoding state.
    async fn restart(&mut self) -> Option<()> {
        self.inp.seek(SeekFrom::Start(0)).await.ok()?;
        self.packets = BasePacketReader::new();
        *self.decoder = Decoder::new(SampleRate::Hz48000, self.channels);
        self.frame_len = 0;
        self.frame_pos = 0;
        self.pre_skip = self.total_pre_skip as usize;
        self.ended = false;
        // Skip the two header packets (OpusHead, OpusTags).
        next_packet(&mut self.packets, &mut self.inp).await??;
        next_packet(&mut self.packets, &mut self.inp).await??;
        Some(())
    }
}

/// Finds the first complete, checksum-valid audio page of stream `serial` at or after `offset`:
/// returns its byte offset and granule position. Header pages (granule 0) and pages on which no
/// packet ends (granule -1) are skipped. Scans a window big enough for several maximum-size
/// pages; a stream position past the last page yields `None`.
async fn probe_page<R: Read + Seek>(inp: &mut R, offset: u64, serial: u32) -> Option<(u64, u64)> {
    inp.seek(SeekFrom::Start(offset)).await.ok()?;
    let mut buf = vec![0u8; 3 * MAX_PAGE_SIZE as usize];
    let mut len = 0;
    while len < buf.len() {
        match inp.read(&mut buf[len..]).await {
            Ok(0) => break,
            Ok(n) => len += n,
            Err(_) => return None,
        }
    }
    let buf = &buf[..len];
    for i in 0..buf.len().saturating_sub(27) {
        if &buf[i..i + 4] == b"OggS"
            && let Some(granule) = page_granule(&buf[i..], serial)
            && granule != 0
            && granule != u64::MAX
        {
            return Some((offset + i as u64, granule));
        }
    }
    None
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
            let nsamples = match self.decoder.decode(Some(&packet.data), &mut self.frame[..], false) {
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
