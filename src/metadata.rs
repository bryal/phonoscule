use core::cmp::min;
use utf8_decode::Decoder as Utf8Decoder;

pub trait Metadata: Default {
    fn title(&self) -> &str;
    fn album(&self) -> &str;
    fn artist(&self) -> &str;
    fn collect_title(&mut self, inp: impl Iterator<Item = u8>);
    fn collect_album(&mut self, inp: impl Iterator<Item = u8>);
    fn collect_artist(&mut self, inp: impl Iterator<Item = u8>);
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

    fn collect_field(&mut self, field: usize, inp: impl Iterator<Item = u8>) {
        let mut field_buf = [0u8; BUF_SIZE];
        let mut inp = Utf8Decoder::new(inp).map(|c| c.unwrap_or('�')).take_while(|&c| c != '\0');
        let mut i = 0;
        for c in inp {
            let n = c.len_utf8();
            if n > field_buf[i..].len() {
                break;
            }
            c.encode_utf8(&mut field_buf[i..]);
            i += n;
        }
        let s = unsafe { std::str::from_utf8_unchecked(&field_buf[..i]) };
        log::debug!("set field {} to {:?}", field, s);
        self.set_field(field, s)
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
            self.buf[start + new_size..last_end].rotate_left(shift.abs() as usize);
            for next in &mut self.fields[field + 1..] {
                next.0 -= shift.abs() as u16;
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
    fn collect_title(&mut self, inp: impl Iterator<Item = u8>) {
        self.collect_field(Self::TITLE, inp)
    }
    fn collect_album(&mut self, inp: impl Iterator<Item = u8>) {
        self.collect_field(Self::ALBUM, inp)
    }
    fn collect_artist(&mut self, inp: impl Iterator<Item = u8>) {
        self.collect_field(Self::ARTIST, inp)
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
        md.collect_artist("Foo".bytes());
        assert_eq!(md.title(), "");
        assert_eq!(md.album(), "");
    }

    #[test]
    fn set_fields() {
        let mut md = StaticMetadata::<32>::default();
        md.collect_title("Ok Song".bytes());
        md.collect_artist("Ok Artist".bytes());
        md.collect_album("Ok Album".bytes());
        assert_eq!(md.title(), "Ok Song");
        assert_eq!(md.artist(), "Ok Artist");
        assert_eq!(md.album(), "Ok Album");
        md.collect_title("ツ".bytes());
        assert_eq!('ツ'.len_utf8(), 3);
        assert_eq!(md.title(), "ツ");
        assert_eq!(md.artist(), "Ok Artist");
    }

    #[test]
    fn set_fields_overflow() {
        let mut md1 = StaticMetadata::<32>::default();
        md1.collect_title("Great Song".bytes());
        md1.collect_artist("Great Artist".bytes());
        md1.collect_album("Great Album".bytes());
        assert_eq!(md1.title(), "Great Song");
        assert_eq!(md1.artist(), "Great Artist");
        assert_eq!(md1.album(), "Great Albu"); // Note that `m` has been cut off -- total length became greater than 32

        // Same thing, different order
        let mut md2 = StaticMetadata::<32>::default();
        md2.collect_album("Great Album".bytes());
        md2.collect_artist("Great Artist".bytes());
        md2.collect_title("Great Song".bytes());
        assert_eq!(md1, md2);
    }

    #[test]
    fn shrink_field() {
        let mut md = StaticMetadata::<32>::default();
        md.collect_title("Foo".bytes());
        md.collect_artist("Bar".bytes());
        md.collect_album("Baz".bytes());
        md.collect_artist("Y".bytes());
        md.collect_title("X".bytes());
        assert_eq!(md.title(), "X");
        assert_eq!(md.artist(), "Y");
        assert_eq!(md.album(), "Baz");
    }

    #[test]
    fn grow_field() {
        let mut md = StaticMetadata::<32>::default();
        md.collect_title("X".bytes());
        md.collect_artist("Y".bytes());
        md.collect_album("Z".bytes());
        md.collect_title("Foo".bytes());
        md.collect_artist("Bar".bytes());
        assert_eq!(md.title(), "Foo");
        assert_eq!(md.artist(), "Bar");
        assert_eq!(md.album(), "Z");
    }

    #[test]
    fn grow_field_overflow() {
        let mut md = StaticMetadata::<20>::default();
        md.collect_title("Ok Song".bytes());
        md.collect_artist("Great Artist".bytes());
        assert_eq!(md.artist(), "Great Artist");
        md.collect_title("Great Song".bytes());
        assert_eq!(md.title(), "Great Song");
        assert_eq!(md.artist(), "Great Arti");
    }

    #[test]
    fn grow_field_overflow_broken_utf8() {
        let mut md = StaticMetadata::<6>::default();
        md.collect_title("Foo".bytes());
        md.collect_artist("ツ".bytes());
        assert_eq!(md.title(), "Foo");
        assert_eq!(md.artist(), "ツ");
        assert_eq!(md.album(), "");
        md.collect_title("Foo!".bytes());
        assert_eq!(md.title(), "Foo!");
        assert_eq!(md.artist(), "");
        assert_eq!(md.album(), "");
    }
}
