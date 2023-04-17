//! Streaming PCM WAV reader
//!
//! Simple and `no_std` friendly. Only requires a byte iterator as input. Seeking not supported.
//!
//! Not standard complete. Makes some inflexible assumptions about the format that nontheless are quite reasonable and
//! very common in .wav files in the wild. Assumptions / limitations include:
//! - All headers / metadata / information must come before the `data` chunk.
//! - Cue point and playlist chunks are ignored.
//! - Audio data must be uncompressed PCM.
//! - PCM data must be little-endian.
//! - Audio must be stereo (2 channels) or mono (1 channel)
//! - "Valid bits per sample" is equal to "bits per sample"

use crate::{metadata::*, pcm::*};
use core::iter::Take;

pub struct WavStream<M, I> {
    pub format: Format,
    pub metadata: M,
    pub data: I,
}

impl<M, I> WavStream<M, I>
where
    M: Metadata,
    I: Iterator<Item = u8>,
{
    pub fn parse(mut inp: I) -> Option<WavStream<M, Take<I>>> {
        if FourCc::parse(&mut inp)?.as_str() != "RIFF" {
            return None;
        }
        let size = u32::from_le_bytes(inp.next_chunk().ok()?) as usize;
        let mut format = None::<Format>;
        let mut metadata = M::default();
        let data_size = {
            let mut inp = (&mut inp).take(size);
            if FourCc::parse(&mut inp)?.as_str() != "WAVE" {
                return None;
            }
            loop {
                let chunk_id = FourCc::parse(&mut inp)?;
                let chunk_size = u32::from_le_bytes(inp.next_chunk().ok()?) as usize;
                log::debug!("chunk id: {}, size: {}", chunk_id.as_str(), chunk_size);
                let mut outer_inp = &mut inp;
                let mut inp = (&mut outer_inp).take(chunk_size);
                match chunk_id.as_str() {
                    "fmt " => {
                        let format_code = u16::from_le_bytes(inp.next_chunk().ok()?);
                        let n_channels = u16::from_le_bytes(inp.next_chunk().ok()?);
                        let blocks_per_sec = u32::from_le_bytes(inp.next_chunk().ok()?);
                        let avg_bytes_per_sec = u32::from_le_bytes(inp.next_chunk().ok()?);
                        let _block_size = u16::from_le_bytes(inp.next_chunk().ok()?);
                        let bits_per_sample = u16::from_le_bytes(inp.next_chunk().ok()?);
                        let ext_size = inp.next_chunk().ok().map(u16::from_le_bytes);
                        let valid_bits_per_sample = inp.next_chunk().ok().map(u16::from_le_bytes);
                        let _speaker_position_mask = inp.next_chunk().ok().map(u32::from_le_bytes);
                        let sub_format_code = inp.next_chunk().ok().map(u16::from_le_bytes);
                        let guid_constant = inp.next_chunk::<14>().ok();
                        log::debug!(
                            "fc: {}, ch: {}, blps: {}, avg: {}, bps: {}, es: {:?}, sf: {:?}, gc: {:?}",
                            format_code,
                            n_channels,
                            blocks_per_sec,
                            avg_bytes_per_sec,
                            bits_per_sample,
                            ext_size,
                            sub_format_code,
                            guid_constant
                        );
                        if let Some(v) = valid_bits_per_sample && v != bits_per_sample {
                            log::error!("Valid bits per sample not equal to bits per sample is unsupported. {} != {}", v, bits_per_sample);
                            return None             
                        }
                        let float = match (format_code, sub_format_code) {
                            (WAVE_FORMAT_PCM, _) |  (WAVE_FORMAT_EXTENSIBLE, Some(WAVE_FORMAT_PCM)) => false,
                            (WAVE_FORMAT_IEEE_FLOAT, _) | (WAVE_FORMAT_EXTENSIBLE, Some(WAVE_FORMAT_IEEE_FLOAT)) => true,
                            (c, Some(s)) => {
                                log::error!("Unsupported format. Format code = {:X}, sub format code = {:X}", c, s);
                                return None
                            }
                            (c, None) => {
                                log::error!("Unsupported format. Format code = {:X}", c);
                                return None
                            }
                        };
                        format = Some(Format { n_channels, float, bits_per_sample });
                    }
                    // The story with metadata in WAV files doesn't look great. There's the standard RIFF/LIST-INFO
                    // method, but most applications only support writing this, and not reading. Then there's ID3v2,
                    // which is non-standard but generally better supported. See
                    // `https://github.com/Borewit/music-metadata/wiki/RIFF-WAVE`
                    "LIST" => {
                        let tag = FourCc::parse(&mut inp)?;
                        if tag.as_str() == "INFO" {
                            parse_metadata(&mut metadata, &mut inp)?
                        } else {
                            log::debug!("ignored LIST chunk with tag {:?}", tag.as_str());
                        }
                    }
                    "data" => break chunk_size,
                    id => {
                        log::debug!("ignored chunk {:?}", id);
                    }
                }
                for _ in inp {}
                for _ in (0..(chunk_size & 1)).zip(&mut outer_inp) {} // there's a padding byte when chunk size is not even
            }
        };
        let format = format?;
        Some(WavStream { format, metadata, data: inp.take(data_size) })
    }

    pub fn format_samples(&mut self) -> Option<Samples<&mut I>> {
        let f = &self.format;
        match (f.float, f.bits_per_sample, f.n_channels) {
            (false, 16, 2) => Some(Samples::StereoS16(PcmReader::new(&mut self.data))),
            (_, _, _) => {
                log::error!("Unsupported format: {} bit {}-channel {}", f.bits_per_sample, f.n_channels, if f.float { "float" } else { "signed/unsigned" });
                None
            }
        }
    }
}

pub struct Format {
    pub float: bool,
    pub bits_per_sample: u16,
    pub n_channels: u16,
}

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

fn parse_metadata(m: &mut impl Metadata, mut inp: impl Iterator<Item = u8>) -> Option<()> {
    while let Some(chunk_id) = FourCc::parse(&mut inp) {
        let chunk_size = u32::from_le_bytes(inp.next_chunk().ok()?) as usize;
        log::debug!("INFO subchunk id: {}, size: {}", chunk_id.as_str(), chunk_size);
        let mut outer_inp = &mut inp;
        let mut inp = (&mut outer_inp).take(chunk_size);
        match chunk_id.as_str() {
            "INAM" => m.collect_title(&mut inp),
            "IPRD" => m.collect_album(&mut inp),
            "IART" => m.collect_artist(&mut inp),
            id => log::debug!("ignored INFO subchunk {:?}", id),
        }
        for _ in inp {}
        for _ in (0..(chunk_size & 1)).zip(&mut outer_inp) {} // padding
    }
    Some(())
}

#[derive(Clone, Copy)]
pub struct FourCc([u8; 4]);

impl FourCc {
    fn parse<I: Iterator<Item = u8>>(mut inp: I) -> Option<Self> {
        let bs = inp.next_chunk::<4>().ok()?;
        bs.iter().all(|c| c.is_ascii() && !c.is_ascii_control()).then_some(Self(bs))
    }

    pub fn as_str(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.0) }
    }
}
