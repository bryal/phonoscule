use core::cmp::min;
use core::mem::ManuallyDrop;
use embedded_io::{
    asynch::{Read, Seek},
    blocking::ReadExactError,
    Io, SeekFrom,
};

/// Like Seek, but forward from current only
pub trait Skip: Io {
    /// Returns how many bytes were actually skipped.
    ///
    /// Skip beyond the end of the stream is allowed. The returned count is then strictly less than the specified
    /// `nbytes`.
    async fn skip(&mut self, nbytes: u64) -> Result<u64, <Self as Io>::Error>;
}
impl<T: Skip> Skip for &mut T {
    async fn skip(&mut self, nbytes: u64) -> Result<u64, <T as Io>::Error> {
        T::skip(*self, nbytes).await
    }
}

pub struct Skippable<T>(pub T);
impl<T: Io> Io for Skippable<T> {
    type Error = T::Error;
}
impl<T: Read> Read for Skippable<T> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, T::Error> {
        self.0.read(buf).await
    }
}
impl<T: Io + Seek> Skip for Skippable<T> {
    async fn skip(&mut self, nskip: u64) -> Result<u64, <T as Io>::Error> {
        let start = self.0.stream_position().await?;
        let new = self.0.seek(SeekFrom::Current(nskip as i64)).await?;
        let nskipped = new - start;
        Ok(nskipped)
    }
}

pub trait ReadExt: Read + Sized {
    async fn read_u16_le(&mut self) -> Result<u16, ReadExactError<<Self as Io>::Error>> {
        let mut buf = [0u8; 2];
        self.read_exact(&mut buf).await.map(|_| u16::from_le_bytes(buf))
    }
    async fn read_u32_le(&mut self) -> Result<u32, ReadExactError<<Self as Io>::Error>> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf).await.map(|_| u32::from_le_bytes(buf))
    }

    fn take(self, count: u64) -> Take<Self> {
        Take::new(count, self)
    }

    fn take_exact(self, count: u64) -> TakeExact<Self> {
        TakeExact::new(count, self)
    }
}

impl<R: Read> ReadExt for R {}

#[must_use = "Must be manually destroyed with `TakeExact::skip_remaining`"]
pub struct TakeExact<R> {
    reader: ManuallyDrop<R>,
    count: u64,
}
impl<R> TakeExact<R> {
    pub fn new(count: u64, reader: R) -> Self {
        Self { count, reader: ManuallyDrop::new(reader) }
    }
}
impl<R: Read + Skip> TakeExact<R> {
    pub async fn skip_rest(mut self) -> Result<u64, R::Error> {
        let mut reader = unsafe { ManuallyDrop::take(&mut self.reader) };
        reader.skip(self.count).await
    }
}
impl<R: Io> Io for TakeExact<R> {
    type Error = R::Error;
}
impl<R: Read> Read for TakeExact<R> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, R::Error> {
        let max_n = min(self.count, buf.len() as u64) as usize;
        match self.reader.read(&mut buf[..max_n]).await {
            Ok(n) => {
                self.count -= n as u64;
                Ok(n)
            }
            err => err,
        }
    }
}
impl<R: Skip> Skip for TakeExact<R> {
    async fn skip(&mut self, nskip: u64) -> Result<u64, <Self as Io>::Error> {
        let nskipped = self.reader.skip(min(nskip, self.count)).await?;
        assert!(self.count >= nskipped);
        self.count -= nskipped;
        Ok(nskipped)
    }
}
impl<R> Drop for TakeExact<R> {
    fn drop(&mut self) {
        if self.count == 0 {
            core::mem::drop(unsafe { ManuallyDrop::take(&mut self.reader) })
        } else if !std::thread::panicking() {
            panic!("Must consume all input ({} remaining) or destroy with `TakeExact::skip_remaining`", self.count);
        }
    }
}

pub struct Take<R> {
    reader: R,
    count: u64,
}
impl<R> Take<R> {
    pub fn new(count: u64, reader: R) -> Self {
        Self { count, reader }
    }
}
impl<R: Read + Skip> Take<R> {
    pub async fn skip_rest(mut self) -> Result<u64, R::Error> {
        self.reader.skip(self.count).await
    }
}
impl<R: Io> Io for Take<R> {
    type Error = R::Error;
}
impl<R: Read> Read for Take<R> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, R::Error> {
        let max_n = min(self.count, buf.len() as u64) as usize;
        match self.reader.read(&mut buf[..max_n]).await {
            Ok(n) => {
                self.count -= n as u64;
                Ok(n)
            }
            err => err,
        }
    }
}
impl<R: Skip> Skip for Take<R> {
    async fn skip(&mut self, nskip: u64) -> Result<u64, <Self as Io>::Error> {
        let nskipped = self.reader.skip(min(nskip, self.count)).await?;
        assert!(self.count >= nskipped);
        self.count -= nskipped;
        Ok(nskipped)
    }
}
