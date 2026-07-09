use core::cmp::min;
use embedded_io_async::Read;
use utf8_decode::Decoder as Utf8Decoder;

pub trait Metadata: Default {
    fn title(&self) -> &str;
    fn album(&self) -> &str;
    fn artist(&self) -> &str;
    async fn read_title<R: Read>(&mut self, size: usize, inp: &mut R) -> Result<usize, R::Error>;
    async fn read_album<R: Read>(&mut self, size: usize, inp: &mut R) -> Result<usize, R::Error>;
    async fn read_artist<R: Read>(&mut self, size: usize, inp: &mut R) -> Result<usize, R::Error>;
    fn set_title(&mut self, inp: &str);
    fn set_album(&mut self, inp: &str);
    fn set_artist(&mut self, inp: &str);
}

#[derive(Clone)]
pub struct StaticMetadata<const BUF_SIZE: usize = 256> {
    fields: [(u16, u16); STATIC_METADATA_N_FIELDS], // (start, length)
    buf: [u8; BUF_SIZE],
}

const STATIC_METADATA_N_FIELDS: usize = 3;
impl<const BUF_SIZE: usize> StaticMetadata<BUF_SIZE> {
    const TITLE: usize = 0;
    const ARTIST: usize = 1;
    const ALBUM: usize = 2;

    async fn read_field<R: Read>(&mut self, field: usize, size: usize, inp: &mut R) -> Result<usize, R::Error> {
        let mut field_buf = [0u8; BUF_SIZE];
        let mut i = 0;
        let mut inp_buf = [0u8; 80];
        let mut remaining = size;
        'outer: while remaining > 0 {
            let n = inp.read(&mut inp_buf[..min(remaining, 80)]).await?;
            if n == 0 {
                // Really this should be an unexpected EOF error, but we can't construct that with these types...
                break;
            }
            remaining -= n;
            let chars = Utf8Decoder::new(inp_buf.into_iter()).map(|c| c.unwrap_or('�')).take_while(|&c| c != '\0');
            for c in chars {
                let n = c.len_utf8();
                if n > field_buf[i..].len() {
                    break 'outer;
                }
                c.encode_utf8(&mut field_buf[i..]);
                i += n;
            }
        }
        let s = unsafe { std::str::from_utf8_unchecked(&field_buf[..i]) };
        log::trace!("set field {} to {:?}", field, s);
        self.set_field(field, s);
        Ok(size - remaining)
    }

    fn set_field(&mut self, field: usize, s: &str) {
        let start = self.fields[field].0 as usize;
        let old_size = self.fields[field].1 as usize;
        let new_size = min(BUF_SIZE - start, s.len());
        self.fields[field].1 = new_size as u16;
        let bs = &s.as_bytes()[..new_size];
        let shift = new_size as isize - old_size as isize;
        let last = self.fields.last().unwrap();
        let last_end = last.0 as usize + last.1 as usize;
        if shift < 0 {
            self.buf[start + new_size..last_end].rotate_left(shift.unsigned_abs());
            for next in &mut self.fields[field + 1..] {
                next.0 -= shift.unsigned_abs() as u16;
            }
        } else {
            self.buf[start + old_size..min(BUF_SIZE, last_end + shift as usize)].rotate_right(shift as usize);
            for next in &mut self.fields[field + 1..] {
                next.0 = min(BUF_SIZE as u16, next.0 + shift as u16);
                next.1 = min(BUF_SIZE as u16 - next.0, next.1);
            }
        }
        self.buf[start..start + new_size].copy_from_slice(bs);
        // trim potentially broken utf8 at the end
        if let Some(last_nonempty) = self.fields.iter_mut().rev().find(|f| f.1 > 0) {
            if last_nonempty.0 as usize + last_nonempty.1 as usize == BUF_SIZE {
                let len: usize = Utf8Decoder::new(self.buf[last_nonempty.0 as usize..].iter().cloned())
                    .flatten()
                    .map(|c| c.len_utf8())
                    .sum();
                last_nonempty.1 = len as u16;
                let trimmed = last_nonempty.1 as usize - len;
                for next in self.fields.iter_mut().rev().take_while(|f| f.1 == 0) {
                    next.0 -= trimmed as u16;
                }
            }
        }
    }

    fn field_str(&self, field: usize) -> &str {
        let (i, n) = self.fields[field];
        unsafe { std::str::from_utf8_unchecked(&self.buf[i as usize..i as usize + n as usize]) }
    }
}

impl<const BUF_SIZE: usize> Metadata for StaticMetadata<BUF_SIZE> {
    fn title(&self) -> &str {
        self.field_str(Self::TITLE)
    }
    fn album(&self) -> &str {
        self.field_str(Self::ALBUM)
    }
    fn artist(&self) -> &str {
        self.field_str(Self::ARTIST)
    }
    async fn read_title<R: Read>(&mut self, size: usize, inp: &mut R) -> Result<usize, R::Error> {
        self.read_field(Self::TITLE, size, inp).await
    }
    async fn read_album<R: Read>(&mut self, size: usize, inp: &mut R) -> Result<usize, R::Error> {
        self.read_field(Self::ALBUM, size, inp).await
    }
    async fn read_artist<R: Read>(&mut self, size: usize, inp: &mut R) -> Result<usize, R::Error> {
        self.read_field(Self::ARTIST, size, inp).await
    }
    fn set_title(&mut self, inp: &str) {
        self.set_field(Self::TITLE, inp)
    }
    fn set_album(&mut self, inp: &str) {
        self.set_field(Self::ALBUM, inp)
    }
    fn set_artist(&mut self, inp: &str) {
        self.set_field(Self::ARTIST, inp)
    }
}

impl<const BUF_SIZE: usize> Default for StaticMetadata<BUF_SIZE> {
    fn default() -> Self {
        StaticMetadata { fields: [(0, 0); STATIC_METADATA_N_FIELDS], buf: [0u8; BUF_SIZE] }
    }
}

impl<const BUF_SIZE: usize> PartialEq for StaticMetadata<BUF_SIZE> {
    fn eq(&self, rhs: &Self) -> bool {
        self.fields == rhs.fields
            && self.buf[..self.fields.last().unwrap().1 as usize] == rhs.buf[..rhs.fields.last().unwrap().1 as usize]
    }
}

impl<const BUF_SIZE: usize> Eq for StaticMetadata<BUF_SIZE> {}

impl<const BUF_SIZE: usize> std::fmt::Debug for StaticMetadata<BUF_SIZE> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        fmt.debug_struct("StaticMetadata")
            .field("BUF_SIZE", &BUF_SIZE)
            .field("title", &self.title())
            .field("artist", &self.artist())
            .field("album", &self.album())
            .finish()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn empty_fields() {
        let mut md = StaticMetadata::<32>::default();
        assert_eq!(md.title(), "");
        assert_eq!(md.artist(), "");
        assert_eq!(md.album(), "");
        md.set_artist("Foo");
        assert_eq!(md.title(), "");
        assert_eq!(md.album(), "");
    }

    #[test]
    fn set_fields() {
        let mut md = StaticMetadata::<32>::default();
        md.set_title("Ok Song");
        md.set_artist("Ok Artist");
        md.set_album("Ok Album");
        assert_eq!(md.title(), "Ok Song");
        assert_eq!(md.artist(), "Ok Artist");
        assert_eq!(md.album(), "Ok Album");
        md.set_title("ツ");
        assert_eq!('ツ'.len_utf8(), 3);
        assert_eq!(md.title(), "ツ");
        assert_eq!(md.artist(), "Ok Artist");
    }

    #[test]
    fn set_fields_overflow() {
        let mut md1 = StaticMetadata::<32>::default();
        md1.set_title("Great Song");
        md1.set_artist("Great Artist");
        md1.set_album("Great Album");
        assert_eq!(md1.title(), "Great Song");
        assert_eq!(md1.artist(), "Great Artist");
        assert_eq!(md1.album(), "Great Albu"); // Note that `m` has been cut off -- total length became greater than 32

        // Same thing, different order
        let mut md2 = StaticMetadata::<32>::default();
        md2.set_album("Great Album");
        md2.set_artist("Great Artist");
        md2.set_title("Great Song");
        assert_eq!(md1, md2);
    }

    #[test]
    fn shrink_field() {
        let mut md = StaticMetadata::<32>::default();
        md.set_title("Foo");
        md.set_artist("Bar");
        md.set_album("Baz");
        md.set_artist("Y");
        md.set_title("X");
        assert_eq!(md.title(), "X");
        assert_eq!(md.artist(), "Y");
        assert_eq!(md.album(), "Baz");
    }

    #[test]
    fn grow_field() {
        let mut md = StaticMetadata::<32>::default();
        md.set_title("X");
        md.set_artist("Y");
        md.set_album("Z");
        md.set_title("Foo");
        md.set_artist("Bar");
        assert_eq!(md.title(), "Foo");
        assert_eq!(md.artist(), "Bar");
        assert_eq!(md.album(), "Z");
    }

    #[test]
    fn grow_field_overflow() {
        let mut md = StaticMetadata::<20>::default();
        md.set_title("Ok Song");
        md.set_artist("Great Artist");
        assert_eq!(md.artist(), "Great Artist");
        md.set_title("Great Song");
        assert_eq!(md.title(), "Great Song");
        assert_eq!(md.artist(), "Great Arti");
    }

    #[test]
    fn grow_field_overflow_broken_utf8() {
        let mut md = StaticMetadata::<6>::default();
        md.set_title("Foo");
        md.set_artist("ツ");
        assert_eq!(md.title(), "Foo");
        assert_eq!(md.artist(), "ツ");
        assert_eq!(md.album(), "");
        md.set_title("Foo!");
        assert_eq!(md.title(), "Foo!");
        assert_eq!(md.artist(), "");
        assert_eq!(md.album(), "");
    }
}
