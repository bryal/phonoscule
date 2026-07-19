//! PCM WAV reader
//!
//! Simple and `no_std` friendly. Only requires an (embedded-io) async Reader as input.
//!
//! Not standard complete. Makes some inflexible assumptions about the format that nontheless are quite reasonable and
//! very common in .wav files in the wild. Assumptions / limitations include:
//! - All headers / metadata / information must come before the `data` chunk.
//! - Cue point and playlist chunks are ignored.
//! - Audio data must be uncompressed PCM.
//! - PCM data must be little-endian.
//! - Audio must be stereo (2 channels) or mono (1 channel)
//! - "Valid bits per sample" is equal to "bits per sample"

use crate::{
    io::{ReadExt, Skip, Take},
    metadata::{Tag, read_text},
    sample::*,
};
use embedded_io_async::Read;

pub struct Wav<R> {
    pub format: Format,
    pub samples: MultiReader<R>,
}

impl<R> Wav<Take<R>>
where
    R: Read + Skip,
{
    /// Parses the headers, pushing each metadata tag to `on_tag` as it is encountered (see
    /// [`Tag`]), and returns the stream positioned at the audio data.
    pub async fn parse(mut inp: R, mut on_tag: impl FnMut(Tag<'_>)) -> Option<Self> {
        if FourCc::parse(&mut inp).await?.as_str() != "RIFF" {
            return None;
        }
        let size = inp.read_u32_le().await.ok()? as usize;
        let mut inp = inp.take(size as u64);
        let mut format = None::<Format>;
        let data_size = {
            if FourCc::parse(&mut inp).await?.as_str() != "WAVE" {
                return None;
            }
            loop {
                let chunk_id = FourCc::parse(&mut inp).await?;
                let chunk_size = inp.read_u32_le().await.ok()? as usize;
                log::trace!("chunk id: {}, size: {}", chunk_id.as_str(), chunk_size);
                {
                    match chunk_id.as_str() {
                        "fmt " => {
                            let mut inp = (&mut inp).take_exact(chunk_size as u64);
                            let format_code = inp.read_u16_le().await.ok()?;
                            let n_channels = inp.read_u16_le().await.ok()?;
                            let blocks_per_sec = inp.read_u32_le().await.ok()?;
                            let avg_bytes_per_sec = inp.read_u32_le().await.ok()?;
                            let block_size = inp.read_u16_le().await.ok()?;
                            let bits_per_sample = inp.read_u16_le().await.ok()?;
                            let ext_size = inp.read_u16_le().await.ok();
                            let valid_bits_per_sample = inp.read_u16_le().await.ok();
                            let _speaker_position_mask = inp.read_u32_le().await.ok();
                            let sub_format_code = inp.read_u16_le().await.ok();
                            let mut guid_constant = [0u8; 14];
                            let guid_constant = inp.read_exact(&mut guid_constant).await.ok().map(move |_| guid_constant);
                            log::trace!(
                                "fc: {}, ch: {}, blps: {}, avg: {}, blsz: {}, bps: {}, es: {:?}, sf: {:?}, gc: {:?}",
                                format_code,
                                n_channels,
                                blocks_per_sec,
                                avg_bytes_per_sec,
                                block_size,
                                bits_per_sample,
                                ext_size,
                                sub_format_code,
                                guid_constant
                            );
                            if let Some(v) = valid_bits_per_sample
                                && v != bits_per_sample
                            {
                                log::error!(
                                    "Valid bits per sample not equal to bits per sample is unsupported. {} != {}",
                                    v,
                                    bits_per_sample
                                );
                                return None;
                            }
                            let float = match (format_code, sub_format_code) {
                                (WAVE_FORMAT_PCM, _) | (WAVE_FORMAT_EXTENSIBLE, Some(WAVE_FORMAT_PCM)) => false,
                                (WAVE_FORMAT_IEEE_FLOAT, _) | (WAVE_FORMAT_EXTENSIBLE, Some(WAVE_FORMAT_IEEE_FLOAT)) => true,
                                (c, Some(s)) => {
                                    log::error!("Unsupported format. Format code = {:X}, sub format code = {:X}", c, s);
                                    return None;
                                }
                                (c, None) => {
                                    log::error!("Unsupported format. Format code = {:X}", c);
                                    return None;
                                }
                            };
                            inp.skip_rest().await.ok()?;
                            format = Some(Format { n_channels, float, bits_per_sample, block_size, size: 0, blocks_per_sec });
                        }
                        // The story with metadata in WAV files doesn't look great. There's the standard RIFF/LIST-INFO
                        // method, but most applications only support writing this, and not reading. Then there's ID3v2,
                        // which is non-standard but generally better supported. See
                        // `https://github.com/Borewit/music-metadata/wiki/RIFF-WAVE`
                        "LIST" => {
                            let mut inp = (&mut inp).take_exact(chunk_size as u64);
                            let tag = FourCc::parse(&mut inp).await?;
                            if tag.as_str() == "INFO" {
                                parse_metadata(&mut on_tag, &mut inp).await?
                            } else {
                                log::trace!("ignored LIST chunk with tag {:?}", tag.as_str());
                            }
                            inp.skip_rest().await.ok()?;
                        }
                        "data" => break chunk_size,
                        id => {
                            let inp = (&mut inp).take_exact(chunk_size as u64);
                            log::trace!("ignored chunk {:?}", id);
                            inp.skip_rest().await.ok()?;
                        }
                    }
                }
                inp.skip(chunk_size as u64 & 1).await.ok()?; // there's a padding byte when chunk size is not even
            }
        };
        let mut format = format?;
        format.size = data_size as u64;
        let samples = match (format.float, format.bits_per_sample, format.block_size, format.n_channels) {
            (false, 16, 4, 2) => {
                log::debug!("Format matches Stereo PCM S16");
                Some(MultiReader::StereoPcmS16(FormatReader::new(inp)))
            }
            (false, 24, 6, 2) => {
                log::debug!("Format matches Stereo PCM S24");
                Some(MultiReader::StereoPcmS24(FormatReader::new(inp)))
            }
            (_, _, _, _) => {
                log::error!(
                    "Unsupported format: {} bit {}-channel {} (block size = {})",
                    format.bits_per_sample,
                    format.n_channels,
                    if format.float { "float" } else { "signed/unsigned" },
                    format.block_size
                );
                None
            }
        }?;
        Some(Wav { format, samples })
    }
}

#[derive(Clone, Debug)]
pub struct Format {
    pub float: bool,
    pub bits_per_sample: u16,
    pub block_size: u16,
    pub blocks_per_sec: u32,
    pub n_channels: u16,
    pub size: u64,
}

impl Format {
    pub fn sample_rate(&self) -> u32 {
        self.blocks_per_sec
    }

    pub fn len_samples(&self) -> u64 {
        self.size / ((self.bits_per_sample as u64 / 8) * self.n_channels as u64)
    }
}

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// One INFO field's scratch: WAV streams field bytes, so they must land somewhere before a
/// borrowed [`Tag`] can be pushed. Per field and reused, so no field's length affects another;
/// generous for LIST-INFO in the wild, and longer fields simply truncate.
const TAG_SCRATCH: usize = 512;

async fn parse_metadata<R>(on_tag: &mut impl FnMut(Tag<'_>), inp: &mut R) -> Option<()>
where
    R: Read + Skip,
{
    let mut scratch = [0u8; TAG_SCRATCH];
    while let Some(chunk_id) = FourCc::parse(inp).await {
        let mut buf = [0; 4];
        inp.read_exact(&mut buf).await.ok()?;
        let chunk_size = u32::from_le_bytes(buf) as usize;
        log::trace!("INFO subchunk id: {}, size: {}", chunk_id.as_str(), chunk_size);
        let nread = match chunk_id.as_str() {
            id @ ("INAM" | "IPRD" | "IART" | "IGNR") => {
                let (text, consumed) = read_text(&mut scratch, chunk_size, inp).await.ok()?;
                on_tag(match id {
                    "INAM" => Tag::Title(text),
                    "IPRD" => Tag::Album(text),
                    "IART" => Tag::Artist(text),
                    _ => Tag::Genre(text),
                });
                consumed
            }
            id => {
                log::trace!("ignored INFO subchunk {:?}", id);
                0
            }
        };
        let padding_size = chunk_size & 1;
        inp.skip(chunk_size as u64 + padding_size as u64 - nread as u64).await.ok()?;
    }
    Some(())
}

#[derive(Clone, Copy)]
pub struct FourCc([u8; 4]);

impl FourCc {
    async fn parse<R: Read>(inp: &mut R) -> Option<Self> {
        let mut bs = [0; 4];
        inp.read_exact(&mut bs).await.ok()?;
        bs.iter().all(|c| c.is_ascii() && !c.is_ascii_control()).then_some(Self(bs))
    }

    pub fn as_str(&self) -> &str {
        unsafe { core::str::from_utf8_unchecked(&self.0) }
    }
}
