use crate::{io::*, plumbing::*};
use core::{cmp::min, mem::size_of};
use embedded_io_async::Read;

#[derive(Copy, Clone, Default, Debug)]
#[repr(C)]
pub struct PcmS16Le([u8; 2]);

impl PcmS16Le {
    pub fn new(v: i16) -> Self {
        Self(v.to_le_bytes())
    }

    pub fn get(self) -> i16 {
        i16::from_le_bytes(self.0)
    }
}

#[derive(Copy, Clone, Default, Debug)]
#[repr(C)]
pub struct PcmS24Le([u8; 3]);

/// Unsigned 8-bit PCM, as WAV stores its 8-bit samples (midpoint 128).
#[derive(Copy, Clone, Default, Debug)]
#[repr(C)]
pub struct PcmU8([u8; 1]);

#[derive(Copy, Clone, Default, Debug)]
#[repr(C)]
pub struct PcmS32Le([u8; 4]);

/// 32-bit float PCM, samples nominally in [-1.0, 1.0].
#[derive(Copy, Clone, Default, Debug)]
#[repr(C)]
pub struct PcmF32Le([u8; 4]);

/// A PCM sample scalar reducible to the 16-bit samples the players output. Kept as byte arrays so
/// a run of frames reads straight off disk (see [`FormatReader`]); the reduction is applied only
/// on the way out, sample by sample.
pub trait ToS16: Copy + Default {
    fn to_s16(self) -> PcmS16Le;
}

impl ToS16 for PcmU8 {
    fn to_s16(self) -> PcmS16Le {
        // Recenter [0, 255] around zero, then widen to 16-bit.
        PcmS16Le::new((i16::from(self.0[0]) - 128) << 8)
    }
}
impl ToS16 for PcmS16Le {
    fn to_s16(self) -> PcmS16Le {
        self
    }
}
impl ToS16 for PcmS24Le {
    fn to_s16(self) -> PcmS16Le {
        // Keep the high two of the three little-endian bytes (truncate the low 8 bits).
        let [_, b1, b2] = self.0;
        PcmS16Le([b1, b2])
    }
}
impl ToS16 for PcmS32Le {
    fn to_s16(self) -> PcmS16Le {
        let [_, _, b2, b3] = self.0;
        PcmS16Le([b2, b3])
    }
}
impl ToS16 for PcmF32Le {
    fn to_s16(self) -> PcmS16Le {
        let f = f32::from_le_bytes(self.0);
        PcmS16Le::new((f.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
    }
}

/// Stereo sample in interleaved format (L1 R1 L2 R2 L3 R3 ...)
#[derive(Copy, Clone, Default, Debug)]
#[repr(C)]
pub struct Stereo<Sample> {
    left: Sample,
    right: Sample,
}

impl<Sample> Stereo<Sample> {
    pub fn new(left: Sample, right: Sample) -> Self {
        Self { left, right }
    }
}

impl<Sample: Copy> Stereo<Sample> {
    pub fn left(self) -> Sample {
        self.left
    }
    pub fn right(self) -> Sample {
        self.right
    }
}

pub struct FormatReader<Sample, R> {
    reader: R,
    sample_type: core::marker::PhantomData<Sample>,
}

impl<Sample, R> FormatReader<Sample, R> {
    pub(crate) fn new(reader: R) -> Self {
        Self { reader, sample_type: core::marker::PhantomData }
    }
}

impl<Sample, R> FastForward for FormatReader<Sample, R>
where
    R: Skip,
{
    async fn fast_forward(&mut self, nsamples: u64) -> Option<u64> {
        let mut n_bytes_to_skip = nsamples * size_of::<Sample>() as u64;
        let mut tot_skipped_bytes = 0;
        loop {
            let nskipped = self.reader.skip(n_bytes_to_skip).await.ok()?;
            tot_skipped_bytes += nskipped;
            if tot_skipped_bytes % size_of::<Sample>() as u64 == 0 {
                return Some(tot_skipped_bytes / size_of::<Sample>() as u64);
            }
            n_bytes_to_skip -= nskipped
        }
    }
}

impl<Sample, R> Source<Sample> for FormatReader<Sample, R>
where
    R: Read,
{
    async fn read_samples(&mut self, out: &mut [Sample]) -> Option<usize> {
        let len_bytes = core::mem::size_of_val(out);
        let mut out = unsafe { core::slice::from_raw_parts_mut(out.as_mut_ptr() as *mut u8, len_bytes) };
        let mut tot_read_bytes = 0;
        loop {
            let nread = self.reader.read(out).await.ok()?;
            tot_read_bytes += nread;
            if tot_read_bytes % size_of::<Sample>() == 0 {
                return Some(tot_read_bytes / size_of::<Sample>());
            }
            out = &mut out[nread..]
        }
    }
}

/// A decoded PCM stream in one of the WAV sample formats, mono or stereo. Reads out as the
/// interleaved [`Stereo<PcmS16Le>`] the players consume: the native S16-stereo case is a direct
/// read, everything else converts per sample ([`ToS16`]), and mono is duplicated to both channels.
pub enum MultiReader<R> {
    MonoU8(FormatReader<PcmU8, R>),
    StereoU8(FormatReader<Stereo<PcmU8>, R>),
    MonoS16(FormatReader<PcmS16Le, R>),
    StereoS16(FormatReader<Stereo<PcmS16Le>, R>),
    MonoS24(FormatReader<PcmS24Le, R>),
    StereoS24(FormatReader<Stereo<PcmS24Le>, R>),
    MonoS32(FormatReader<PcmS32Le, R>),
    StereoS32(FormatReader<Stereo<PcmS32Le>, R>),
    MonoF32(FormatReader<PcmF32Le, R>),
    StereoF32(FormatReader<Stereo<PcmF32Le>, R>),
}

impl<R> Source<Stereo<PcmS16Le>> for MultiReader<R>
where
    R: Read,
{
    async fn read_samples(&mut self, buf: &mut [Stereo<PcmS16Le>]) -> Option<usize> {
        match self {
            // Native output type: read straight into the caller's buffer, no conversion.
            MultiReader::StereoS16(r) => r.read_samples(buf).await,
            MultiReader::StereoU8(r) => read_stereo(r, buf).await,
            MultiReader::StereoS24(r) => read_stereo(r, buf).await,
            MultiReader::StereoS32(r) => read_stereo(r, buf).await,
            MultiReader::StereoF32(r) => read_stereo(r, buf).await,
            MultiReader::MonoU8(r) => read_mono(r, buf).await,
            MultiReader::MonoS16(r) => read_mono(r, buf).await,
            MultiReader::MonoS24(r) => read_mono(r, buf).await,
            MultiReader::MonoS32(r) => read_mono(r, buf).await,
            MultiReader::MonoF32(r) => read_mono(r, buf).await,
        }
    }
}

/// Reads a chunk of stereo frames of the native format and reduces each channel to S16.
async fn read_stereo<X, R>(r: &mut FormatReader<Stereo<X>, R>, out: &mut [Stereo<PcmS16Le>]) -> Option<usize>
where
    X: ToS16,
    R: Read,
{
    const TMP: usize = 64;
    let mut tmp = [Stereo::<X>::default(); TMP];
    let n = min(TMP, out.len());
    let nread = r.read_samples(&mut tmp[..n]).await?;
    for (o, s) in out.iter_mut().zip(tmp.into_iter().take(nread)) {
        *o = Stereo::new(s.left.to_s16(), s.right.to_s16());
    }
    Some(nread)
}

/// Reads a chunk of mono frames of the native format, reduces each to S16, and duplicates it to
/// both output channels.
async fn read_mono<X, R>(r: &mut FormatReader<X, R>, out: &mut [Stereo<PcmS16Le>]) -> Option<usize>
where
    X: ToS16,
    R: Read,
{
    const TMP: usize = 64;
    let mut tmp = [X::default(); TMP];
    let n = min(TMP, out.len());
    let nread = r.read_samples(&mut tmp[..n]).await?;
    for (o, x) in out.iter_mut().zip(tmp.into_iter().take(nread)) {
        let v = x.to_s16();
        *o = Stereo::new(v, v);
    }
    Some(nread)
}

impl<R> FastForward for MultiReader<R>
where
    R: Skip,
{
    async fn fast_forward(&mut self, nsamples: u64) -> Option<u64> {
        // Each reader skips by its own frame size, so the frame count stays exact per format.
        match self {
            MultiReader::MonoU8(r) => r.fast_forward(nsamples).await,
            MultiReader::StereoU8(r) => r.fast_forward(nsamples).await,
            MultiReader::MonoS16(r) => r.fast_forward(nsamples).await,
            MultiReader::StereoS16(r) => r.fast_forward(nsamples).await,
            MultiReader::MonoS24(r) => r.fast_forward(nsamples).await,
            MultiReader::StereoS24(r) => r.fast_forward(nsamples).await,
            MultiReader::MonoS32(r) => r.fast_forward(nsamples).await,
            MultiReader::StereoS32(r) => r.fast_forward(nsamples).await,
            MultiReader::MonoF32(r) => r.fast_forward(nsamples).await,
            MultiReader::StereoF32(r) => r.fast_forward(nsamples).await,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::io::Skippable;
    use embedded_io_adapters::futures_03::FromFutures;

    /// Every input sample format reduces to the expected 16-bit value.
    #[test]
    fn to_s16_conversions() {
        assert_eq!(PcmS16Le::new(12_345).to_s16().get(), 12_345);
        // 8-bit is unsigned, centered on 128 and widened to 16-bit.
        assert_eq!(PcmU8([128]).to_s16().get(), 0);
        assert_eq!(PcmU8([255]).to_s16().get(), 127 << 8);
        assert_eq!(PcmU8([0]).to_s16().get(), -32768);
        // 24- and 32-bit keep the high two little-endian bytes (0x123456 / 0x12345678 -> 0x1234).
        assert_eq!(PcmS24Le([0x56, 0x34, 0x12]).to_s16().get(), 0x1234);
        assert_eq!(PcmS32Le([0x78, 0x56, 0x34, 0x12]).to_s16().get(), 0x1234);
        // Float scales [-1, 1] to full range and clamps beyond it.
        assert_eq!(PcmF32Le(1.0f32.to_le_bytes()).to_s16().get(), i16::MAX);
        assert_eq!(PcmF32Le((-1.0f32).to_le_bytes()).to_s16().get(), -i16::MAX);
        assert_eq!(PcmF32Le(0.0f32.to_le_bytes()).to_s16().get(), 0);
        assert_eq!(PcmF32Le(2.0f32.to_le_bytes()).to_s16().get(), i16::MAX);
    }

    /// Reads a whole [`MultiReader`] over `bytes` to interleaved S16 (left, right) pairs.
    fn decode(reader: MultiReader<Skippable<FromFutures<smol::io::Cursor<&[u8]>>>>) -> Vec<(i16, i16)> {
        smol::block_on(async {
            let mut reader = reader;
            let mut out = Vec::new();
            loop {
                let mut buf = [Stereo::<PcmS16Le>::default(); 8];
                let n = reader.read_samples(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                out.extend(buf[..n].iter().map(|s| (s.left().get(), s.right().get())));
            }
            out
        })
    }

    fn reader(bytes: &[u8]) -> Skippable<FromFutures<smol::io::Cursor<&[u8]>>> {
        Skippable(FromFutures::new(smol::io::Cursor::new(bytes)))
    }

    /// A mono stream duplicates each sample into both output channels.
    #[test]
    fn mono_duplicates_to_both_channels() {
        let data = [1000i16, -2000, 3000].iter().flat_map(|s| s.to_le_bytes()).collect::<Vec<_>>();
        let out = decode(MultiReader::MonoS16(FormatReader::new(reader(&data))));
        assert_eq!(out, [(1000, 1000), (-2000, -2000), (3000, 3000)]);
    }

    /// A stereo 24-bit stream keeps left/right distinct and truncates to 16-bit.
    #[test]
    fn stereo_s24_reads_both_channels() {
        // Two frames, each L then R as little-endian 24-bit; the high two bytes survive to S16.
        let data: Vec<u8> = vec![0x56, 0x34, 0x12, 0x21, 0x43, 0x65, 0x80, 0x00, 0x00, 0x7f, 0xff, 0xff];
        let out = decode(MultiReader::StereoS24(FormatReader::new(reader(&data))));
        assert_eq!(out, [(0x1234, 0x6543), (0, -1)]);
    }
}
