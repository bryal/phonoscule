pub trait Sink<Sample> {
    async fn write_samples(&mut self, buf: &[Sample]) -> Option<usize>;
}

pub trait Source<Sample> {
    /// Returns None on error or end of input
    async fn read_samples(&mut self, buf: &mut [Sample]) -> Option<usize>;
}

pub trait SinkInput<Sample> {
    async fn write_samples_to<S: Sink<Sample>>(&mut self, sink: &mut S) -> Option<u64>;
}
impl<Sample, I> SinkInput<Sample> for &mut I
where
    I: SinkInput<Sample>,
{
    async fn write_samples_to<S: Sink<Sample>>(&mut self, sink: &mut S) -> Option<u64> {
        I::write_samples_to(*self, sink).await
    }
}

pub trait SourceOutput<Sample> {
    async fn read_samples_from<S: Source<Sample>>(&mut self, source: &mut S) -> Option<u64>;
}
impl<Sample, O> SourceOutput<Sample> for &mut O
where
    O: SourceOutput<Sample>,
{
    async fn read_samples_from<S: Source<Sample>>(&mut self, source: &mut S) -> Option<u64> {
        O::read_samples_from(*self, source).await
    }
}

pub trait FastForward {
    async fn fast_forward(&mut self, nsamples: u64) -> Option<u64>;
}

pub struct ConnectSource<I, O> {
    source: I,
    out: O,
}

impl<I, O> ConnectSource<I, O> {
    pub fn to_output(source: I, out: O) -> Self {
        Self { source, out }
    }

    /// Access the source, e.g. to seek it between pulls.
    pub fn source_mut(&mut self) -> &mut I {
        &mut self.source
    }

    pub async fn pull<Sample>(&mut self) -> Option<u64>
    where
        I: Source<Sample>,
        O: SourceOutput<Sample>,
    {
        self.out.read_samples_from(&mut self.source).await
    }
}

pub struct ConnectSink<I, O> {
    inp: I,
    sink: O,
}

impl<I, O> ConnectSink<I, O> {
    pub fn from_input(inp: I, sink: O) -> Self {
        Self { inp, sink }
    }

    pub async fn push<Sample>(&mut self) -> Option<u64>
    where
        I: SinkInput<Sample>,
        O: Sink<Sample>,
    {
        self.inp.write_samples_to(&mut self.sink).await
    }
}
