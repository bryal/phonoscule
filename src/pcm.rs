use core::marker::Sized;

pub enum Samples<I> {
    StereoS16(PcmReader<I, [i16; 2]>),
}

impl<I> Samples<I>
where
    I: Iterator<Item = u8>,
{
    pub fn convert<S>(self) -> PcmReader<I, S>
    where
        S: ConvertPcm<[i16; 2]>,
    {
        match self {
            Samples::StereoS16(r) => PcmReader::from_fn(r.data, |inp| <[i16; 2]>::collect(inp).map(S::convert_from)),
        }
    }
}

pub struct PcmReader<I, S> {
    data: I,
    collect: fn(&mut I) -> Option<S>,
}

impl<I, S> PcmReader<I, S>
where
    I: Iterator<Item = u8>,
{
    pub fn new(data: I) -> Self
    where
        S: ReadPcm,
    {
        Self { data, collect: |inp| S::collect(inp) }
    }

    pub fn from_fn(data: I, collect: fn(&mut I) -> Option<S>) -> Self {
        Self { data, collect }
    }
}

impl<I, S> Iterator for PcmReader<I, S>
where
    I: Iterator<Item = u8>,
{
    type Item = S;
    fn next(&mut self) -> Option<S> {
        (self.collect)(&mut self.data)
    }
}

pub trait ReadPcm: Sized {
    fn collect(inp: impl Iterator<Item = u8>) -> Option<Self>;
}

impl ReadPcm for [i16; 2] {
    fn collect(mut inp: impl Iterator<Item = u8>) -> Option<Self> {
        Some([i16::from_le_bytes(inp.next_chunk().ok()?), i16::from_le_bytes(inp.next_chunk().ok()?)])
    }
}

// struct PcmConverter<I, S> {
//     source: I,
//     target_type: PhantomData<S>,
// }

// impl<I, Sin, Sout> PcmConverter<I, Sout>
// where
//     I: Iterator<Item = Sin>,
//     Sin: ConvertPcm<Sout>,
// {
//     fn new(samples: I) -> Self {}
// }

pub trait ConvertPcm<T> {
    fn convert_from(sample: T) -> Self;
}

impl<T> ConvertPcm<T> for T {
    fn convert_from(sample: Self) -> Self {
        sample
    }
}
