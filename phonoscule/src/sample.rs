use crate::{io::*, plumbing::*};
use core::{cmp::min, mem::size_of};
use embedded_io_async::Read;

#[derive(Copy, Clone, Default, Debug)]
#[repr(C)]
pub struct PcmS16Le([u8; 2]);

#[derive(Copy, Clone, Default, Debug)]
#[repr(C)]
pub struct PcmS24Le([u8; 3]);

/// Stereo sample in interleaved format (L1 R1 L2 R2 L3 R3 ...)
#[derive(Copy, Clone, Default, Debug)]
#[repr(C)]
pub struct Stereo<Sample> {
    left: Sample,
    right: Sample,
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

pub enum MultiReader<R> {
    StereoPcmS16(FormatReader<Stereo<PcmS16Le>, R>),
    StereoPcmS24(FormatReader<Stereo<PcmS24Le>, R>),
}

impl<R> Source<Stereo<PcmS16Le>> for MultiReader<R>
where
    R: Read,
{
    async fn read_samples(&mut self, buf: &mut [Stereo<PcmS16Le>]) -> Option<usize> {
        match self {
            MultiReader::StereoPcmS16(r) => r.read_samples(buf).await,
            _ => self.read_convert(buf).await,
        }
    }
}
impl<R> Source<Stereo<PcmS24Le>> for MultiReader<R>
where
    R: Read,
{
    async fn read_samples(&mut self, buf: &mut [Stereo<PcmS24Le>]) -> Option<usize> {
        match self {
            MultiReader::StereoPcmS24(r) => r.read_samples(buf).await,
            _ => self.read_convert(buf).await,
        }
    }
}

impl<R> MultiReader<R> {
    async fn read_convert<Sample>(&mut self, buf: &mut [Sample]) -> Option<usize>
    where
        R: Read,
        Sample: ConvertFrom<Stereo<PcmS16Le>> + ConvertFrom<Stereo<PcmS24Le>>,
    {
        async fn f<Sample, R, S>(r: &mut FormatReader<S, R>, buf: &mut [Sample]) -> Option<usize>
        where
            R: Read,
            Sample: ConvertFrom<S>,
            S: Default + Copy,
        {
            const TMP_SIZE: usize = 64;
            let mut tmp = [Default::default(); TMP_SIZE];
            let nmax = min(TMP_SIZE, buf.len());
            let nread = r.read_samples(&mut tmp[..nmax]).await?;
            for (x, y) in tmp.into_iter().zip(buf) {
                *y = Sample::convert_from(x)
            }
            Some(nread)
        }
        match self {
            MultiReader::StereoPcmS16(r) => f(r, buf).await,
            MultiReader::StereoPcmS24(r) => f(r, buf).await,
        }
    }
}

impl<R> FastForward for MultiReader<R>
where
    R: Skip,
{
    async fn fast_forward(&mut self, nsamples: u64) -> Option<u64> {
        match self {
            MultiReader::StereoPcmS16(r) => r.fast_forward(nsamples).await,
            MultiReader::StereoPcmS24(r) => r.fast_forward(nsamples).await,
        }
    }
}

pub trait ConvertFrom<S> {
    fn convert_from(sample: S) -> Self;
}

impl<S0, S1> ConvertFrom<Stereo<S0>> for Stereo<S1>
where
    S1: ConvertFrom<S0>,
{
    fn convert_from(s: Stereo<S0>) -> Self {
        Stereo { left: S1::convert_from(s.left), right: S1::convert_from(s.right) }
    }
}

impl ConvertFrom<PcmS16Le> for PcmS16Le {
    fn convert_from(x: PcmS16Le) -> Self {
        x
    }
}
impl ConvertFrom<PcmS24Le> for PcmS16Le {
    fn convert_from(PcmS24Le([_, x1, x2]): PcmS24Le) -> Self {
        PcmS16Le([x1, x2])
    }
}

impl ConvertFrom<PcmS16Le> for PcmS24Le {
    fn convert_from(PcmS16Le([x0, x1]): PcmS16Le) -> Self {
        PcmS24Le([0, x0, x1])
    }
}
impl ConvertFrom<PcmS24Le> for PcmS24Le {
    fn convert_from(x: PcmS24Le) -> Self {
        x
    }
}
