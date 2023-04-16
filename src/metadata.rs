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

pub struct StaticMetadata<const Size: usize = 256> {
    fields: [(u16, u16); STATIC_METADATA_N_FIELDS], // (start, length)
    buf: [u8; Size],
}

const STATIC_METADATA_N_FIELDS: usize = 3;
impl<const Size: usize> StaticMetadata<Size> {
    const TITLE: usize = 0;
    const ARTIST: usize = 1;
    const ALBUM: usize = 2;

    fn collect_field(&mut self, field: usize, inp: impl Iterator<Item = u8>) {
        let mut field_buf = [0u8; Size];
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
        let new_size = min(Size - start, s.len());
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
            self.buf[start + old_size..min(Size, last_end + shift as usize)].rotate_right(shift as usize);
            for next in &mut self.fields[field + 1..] {
                next.0 = min(Size as u16, next.0 + shift as u16);
                next.1 = min(Size as u16 - next.0, next.1);
            }
        }
        self.buf[start..start + new_size].copy_from_slice(bs);
        // trim potentially broken utf8 at the end
        if let Some(last_nonempty) = self.fields.iter().rev().find(|f| f.1 > 0).cloned() {
            if last_nonempty.1 as usize == Size {
                let len: usize = Utf8Decoder::new(self.buf[last_nonempty.0 as usize..].iter().cloned())
                    .flatten()
                    .map(|c| c.len_utf8())
                    .sum();
                let trimmed = last_nonempty.1 as usize - len;
                for next in self.fields.iter_mut().rev() {
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

impl<const Size: usize> Default for StaticMetadata<Size> {
    fn default() -> Self {
        StaticMetadata { fields: [(0, 0); STATIC_METADATA_N_FIELDS], buf: [0u8; Size] }
    }
}

impl<const Size: usize> Metadata for StaticMetadata<Size> {
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
