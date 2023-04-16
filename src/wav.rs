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

use crate::metadata::*;

pub struct WavStream<M> {
    pub format: Format,
    pub metadata: M,
}

impl<M> WavStream<M>
where
    M: Metadata,
{
    pub fn parse<I>(mut inp: I) -> Option<WavStream<M>>
    where
        I: Iterator<Item = u8>,
    {
        if FourCc::parse(&mut inp)?.as_str() != "RIFF" {
            return None;
        }
        let size = u32::from_le_bytes(inp.next_chunk().ok()?) as usize;
        let mut format = None::<Format>;
        let mut metadata = M::default();
        let _data_size = {
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
                        let _valid_bits_per_sample = inp.next_chunk().ok().map(u16::from_le_bytes);
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
                        format = Some(Format { n_channels: n_channels as usize });
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
        Some(WavStream { format, metadata })
    }
}

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

pub struct Format {
    n_channels: usize,
}

// pub struct Metadata<const MetaSize: usize = 256> {
//     title_start: u16,
//     title_len: u8,
// }

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

// use std::iter::Take;

// pub struct Riff {
//     pub tag: FourCc,
//     pub chunks: Chunks,
// }

// pub struct StreamingRiff<I>
// where
//     I: Iterator<Item = u8>,
// {
//     pub tag: FourCc,
//     pub chunks: StreamingChunks<I>,
// }

// impl Riff {
//     pub fn parse<I>(mut inp: I) -> Option<StreamingRiff<I>>
//     where
//         I: Iterator<Item = u8>,
//     {
//         if inp.next_chunk::<4>().ok()? != *b"RIFF" {
//             return None;
//         }
//         let riff_len = u32::from_le_bytes(inp.next_chunk::<4>().ok()?) as usize;
//         let tag = FourCc::parse(&mut inp)?;
//         let chunks = StreamingChunks::new(riff_len - 4, inp);
//         Some(StreamingRiff { tag, chunks })
//     }
// }

// impl<I> StreamingRiff<I>
// where
//     I: Iterator<Item = u8>,
// {
//     pub fn strict(mut self) -> Riff {
//         Riff { tag: self.tag, chunks: self.chunks.strict() }
//     }
// }

// pub type Chunks = Vec<Chunk>;

// pub struct StreamingChunks<I>
// where
//     I: Iterator<Item = u8>,
// {
//     inp: I,
//     prev_padding: bool,
//     remaining: usize,
// }

// impl<I> StreamingChunks<I>
// where
//     I: Iterator<Item = u8>,
// {
//     fn new(size: usize, inp: I) -> Self {
//         Self { inp, prev_padding: false, remaining: size }
//     }

//     pub fn strict(&mut self) -> Chunks {
//         std::iter::from_fn(|| self.next().map(|c| c.unwrap().strict())).collect()
//     }

//     pub fn next<'i>(&'i mut self) -> Option<Option<StreamingChunk<&'i mut I>>>
//     where
//         I: Iterator<Item = u8>,
//     {
//         if self.prev_padding {
//             self.inp.next();
//             self.prev_padding = false;
//         }
//         (self.remaining > 0).then(|| {
//             let c = StreamingChunk::parse(&mut self.inp)?;
//             let id_size = 4;
//             let size_size = 4;
//             let inner_size = match c {
//                 StreamingChunk::List { size, .. } => size,
//                 StreamingChunk::Chunk { size, .. } => size,
//             } as usize;
//             let padding_size = inner_size & 1;
//             self.remaining = self.remaining.saturating_sub(id_size + size_size + inner_size + padding_size);
//             if padding_size > 0 {
//                 self.prev_padding = true;
//             }
//             Some(c)
//         })
//     }
// }

// impl<I> Drop for StreamingChunks<I>
// where
//     I: Iterator<Item = u8>,
// {
//     fn drop(&mut self) {
//         while let Some(_) = self.next() {}
//     }
// }

// pub enum Chunk {
//     List { tag: FourCc, size: u32, chunks: Chunks },
//     Chunk { id: FourCc, size: u32, data: Vec<u8> },
// }

// impl Chunk {
//     pub fn id(&self) -> FourCc {
//         match self {
//             Chunk::List { .. } => FourCc(*b"LIST"),
//             Chunk::Chunk { id, .. } => *id,
//         }
//     }
// }

// pub enum StreamingChunk<I>
// where
//     I: Iterator<Item = u8>,
// {
//     List { tag: FourCc, size: u32, chunks: StreamingChunks<I> },
//     Chunk { id: FourCc, size: u32, data: Take<I> },
// }

// impl<I> StreamingChunk<I>
// where
//     I: Iterator<Item = u8>,
// {
//     fn parse(mut inp: I) -> Option<Self> {
//         let id = FourCc::parse(&mut inp)?;
//         let size = u32::from_le_bytes(inp.next_chunk::<4>().ok()?);
//         Some(if id.as_str() == "LIST" {
//             let tag = FourCc::parse(&mut inp)?;
//             StreamingChunk::List { tag, size, chunks: StreamingChunks::new(size as usize, inp) }
//         } else {
//             StreamingChunk::Chunk { id, size, data: inp.take(size as usize) }
//         })
//     }

//     pub fn id(&self) -> FourCc {
//         match self {
//             StreamingChunk::List { .. } => FourCc(*b"LIST"),
//             StreamingChunk::Chunk { id, .. } => *id,
//         }
//     }

//     pub fn strict(&mut self) -> Chunk {
//         match self {
//             StreamingChunk::List { tag, size, ref mut chunks } =>
//                 Chunk::List { tag: *tag, size: *size, chunks: chunks.strict() },
//             StreamingChunk::Chunk { id, size, ref mut data } =>
//                 Chunk::Chunk { id: *id, size: *size, data: data.collect() },
//         }
//     }
// }

// impl<I> Drop for StreamingChunk<I>
// where
//     I: Iterator<Item = u8>,
// {
//     fn drop(&mut self) {
//         if let StreamingChunk::Chunk { data, .. } = self {
//             for _ in data {}
//         }
//     }
// }

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
