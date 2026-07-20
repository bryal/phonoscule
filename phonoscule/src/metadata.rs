//! Metadata tags, delivered as borrowed values.
//!
//! Parsers push each recognized tag to a caller-supplied `FnMut(Tag<'_>)` as they encounter it,
//! borrowing the text from their own working memory (the Ogg tags packet, or a scratch buffer for
//! WAV's streamed INFO fields). Storage is entirely the consumer's business: copy into `String`s,
//! into fixed buffers on embedded targets, or ignore tags outright -- there is no intermediate
//! metadata object, and so no policy here about truncation or which of a repeated tag wins.

use core::cmp::min;
use embedded_io_async::Read;
use utf8_decode::Decoder as Utf8Decoder;

/// One metadata tag, borrowed from the parser's working memory: valid only for the duration of
/// the callback. Deliberately exhaustive -- a new variant should break consumers into deciding
/// how to store it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag<'s> {
    Title(&'s str),
    Artist(&'s str),
    Album(&'s str),
    AlbumArtist(&'s str),
    Genre(&'s str),
    /// The track's position within its album, as the raw tag text: usually plain digits, but "3/12"
    /// (position of total) style values occur in the wild, so consumers should parse leniently.
    TrackNumber(&'s str),
    /// The disc the track belongs to on a multi-disc album, as the raw tag text (see
    /// [`TrackNumber`](Tag::TrackNumber) about lenient parsing).
    DiscNumber(&'s str),
    /// The recording/release date, as the raw tag text: often a bare year, but ISO dates like
    /// "2019-05-03" are common, so consumers wanting a year should parse leniently.
    Date(&'s str),
}

/// Reads a `size`-byte text field from `inp` into `scratch`, decoding UTF-8 (invalid sequences
/// become replacement characters) and stopping at a NUL terminator or when `scratch` fills.
/// Returns the text and how many input bytes were consumed -- at most `size`, possibly fewer;
/// skipping the remainder is the caller's job (it owns the enclosing chunk structure).
pub(crate) async fn read_text<'b, R: Read>(
    scratch: &'b mut [u8],
    size: usize,
    inp: &mut R,
) -> Result<(&'b str, usize), R::Error> {
    let mut written = 0;
    let mut consumed = 0;
    let mut chunk = [0u8; 80];
    'read: while consumed < size {
        let n = inp.read(&mut chunk[..min(size - consumed, 80)]).await?;
        if n == 0 {
            break; // Unexpected EOF: return what we have.
        }
        consumed += n;
        for c in Utf8Decoder::new(chunk[..n].iter().cloned()).map(|c| c.unwrap_or('\u{FFFD}')) {
            if c == '\0' || c.len_utf8() > scratch.len() - written {
                break 'read;
            }
            c.encode_utf8(&mut scratch[written..]);
            written += c.len_utf8();
        }
    }
    Ok((core::str::from_utf8(&scratch[..written]).expect("only whole chars are written"), consumed))
}

#[cfg(test)]
mod test {
    use super::*;

    fn read(scratch: &mut [u8], input: &[u8]) -> (std::string::String, usize) {
        smol::block_on(async {
            let mut inp = crate::io::Skippable(embedded_io_adapters::futures_03::FromFutures::new(input));
            let (s, n) = read_text(scratch, input.len(), &mut inp).await.unwrap();
            (s.into(), n)
        })
    }

    #[test]
    fn decodes_and_stops_at_nul() {
        let mut scratch = [0u8; 64];
        let (s, n) = read(&mut scratch, "Caf\u{e9} au lait\0padding".as_bytes());
        assert_eq!(s, "Caf\u{e9} au lait");
        assert!(n <= "Caf\u{e9} au lait\0padding".len());
    }

    #[test]
    fn replaces_invalid_utf8() {
        let mut scratch = [0u8; 64];
        let (s, _) = read(&mut scratch, b"a\xFFb");
        assert_eq!(s, "a\u{FFFD}b");
    }

    #[test]
    fn truncates_at_scratch_capacity_on_a_char_boundary() {
        let mut scratch = [0u8; 4];
        let (s, _) = read(&mut scratch, "abcツ".as_bytes()); // the 3-byte char doesn't fit after "abc"
        assert_eq!(s, "abc");
    }
}
