// Same choice as `embedded-io-async`: plain `async fn` in public traits, leaving `Send`-ness of
// the returned futures up to the implementor.
#![allow(async_fn_in_trait)]

pub mod io;
pub mod metadata;
pub mod opus;
pub mod plumbing;
pub mod sample;
pub mod wav;

#[cfg(test)]
mod test {
    use super::{
        io::Skippable,
        metadata::*,
        opus::{OggOpus, OpusReader},
        plumbing::*,
        sample,
        wav::*,
    };
    use embedded_io_adapters::futures_03::FromFutures;
    use std::sync::Once;

    static INIT: Once = Once::new();

    fn init() {
        INIT.call_once(|| {
            simple_logger::init().unwrap();
        })
    }

    /// Collects pushed tags for assertions.
    #[derive(Default)]
    struct Tags {
        title: String,
        artist: String,
        album: String,
    }

    impl Tags {
        fn set(&mut self, tag: Tag<'_>) {
            match tag {
                Tag::Title(s) => self.title = s.into(),
                Tag::Artist(s) => self.artist = s.into(),
                Tag::Album(s) => self.album = s.into(),
                Tag::AlbumArtist(_) | Tag::Genre(_) | Tag::TrackNumber(_) | Tag::DiscNumber(_) | Tag::Date(_) => {}
            }
        }
    }

    /// A minimal valid WAV: 48 kHz stereo 16-bit PCM silence with LIST-INFO tags and `n_frames`
    /// of audio. Built in-memory so the tests own no audio files.
    fn wav_bytes(title: &str, artist: &str, album: &str, n_frames: u32) -> Vec<u8> {
        fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut out = Vec::with_capacity(8 + body.len() + 1);
            out.extend(id);
            out.extend((body.len() as u32).to_le_bytes());
            out.extend(body);
            if body.len() % 2 == 1 {
                out.push(0); // chunks are padded to even sizes
            }
            out
        }
        fn info(id: &[u8; 4], value: &str) -> Vec<u8> {
            chunk(id, &[value.as_bytes(), b"\0"].concat())
        }
        let mut fmt = Vec::new();
        fmt.extend(1u16.to_le_bytes()); // WAVE_FORMAT_PCM
        fmt.extend(2u16.to_le_bytes()); // channels
        fmt.extend(48000u32.to_le_bytes()); // blocks per second
        fmt.extend((48000u32 * 4).to_le_bytes()); // avg bytes per second
        fmt.extend(4u16.to_le_bytes()); // block size
        fmt.extend(16u16.to_le_bytes()); // bits per sample
        let list = [&b"INFO"[..], &info(b"INAM", title), &info(b"IART", artist), &info(b"IPRD", album)].concat();
        let data = vec![0u8; n_frames as usize * 4];
        let riff = [&b"WAVE"[..], &chunk(b"fmt ", &fmt), &chunk(b"LIST", &list), &chunk(b"data", &data)].concat();
        let mut out = Vec::new();
        out.extend(b"RIFF");
        out.extend((riff.len() as u32).to_le_bytes());
        out.extend(riff);
        out
    }

    #[test]
    fn parse_a_wav_file() {
        init();
        smol::block_on(async {
            let data = wav_bytes("Silent Night", "Nobody", "Quiet Sessions", 4800);
            let f = Skippable(FromFutures::new(smol::io::Cursor::new(&data[..])));
            let mut tags = Tags::default();
            let wav = Wav::parse(f, |tag| tags.set(tag)).await.unwrap();
            assert_eq!(tags.title, "Silent Night");
            assert_eq!(tags.artist, "Nobody");
            assert_eq!(tags.album, "Quiet Sessions");
            let mut samples = match wav.samples {
                sample::MultiReader::StereoS16(s) => s,
                _ => panic!("unexpected format, {:?}", wav.format),
            };
            assert_eq!(wav.format.len_samples(), 4800);
            let mut nleft = wav.format.len_samples();
            loop {
                let mut buf = [Default::default(); 32];
                let nread = samples.read_samples(&mut buf).await.unwrap();
                if nleft == 0 {
                    assert!(nread == 0, "wav format says none left, but we read another {nread}");
                    break;
                } else {
                    assert!(nread > 0 && nread as u64 <= nleft, "wav format says {nleft} left, but we read {nread}");
                    nleft -= nread as u64;
                }
            }
        })
    }

    /// Builds a valid, decodable Ogg Opus stream of stereo silence with the given Vorbis
    /// `comments` (key, value): `n_packets` zero-length (DTX) CELT frames of 20 ms each. Granule
    /// positions count raw decoded samples, of which the first `PRE_SKIP` are dropped (RFC 7845 §4).
    fn silence_opus(n_packets: usize, comments: &[(&str, &str)]) -> Vec<u8> {
        const PRE_SKIP: u64 = 312;
        let mut head = Vec::new();
        head.extend(b"OpusHead");
        head.push(1); // version
        head.push(2); // channels
        head.extend((PRE_SKIP as u16).to_le_bytes());
        head.extend(48000u32.to_le_bytes()); // input sample rate
        head.extend(0u16.to_le_bytes()); // output gain
        head.push(0); // channel mapping family 0

        let mut tags = Vec::new();
        tags.extend(b"OpusTags");
        let vendor = b"phonoscule-test";
        tags.extend((vendor.len() as u32).to_le_bytes());
        tags.extend(vendor);
        tags.extend((comments.len() as u32).to_le_bytes());
        for (key, value) in comments {
            let comment = format!("{key}={value}");
            tags.extend((comment.len() as u32).to_le_bytes());
            tags.extend(comment.as_bytes());
        }

        let serial = 0x5eed;
        let mut out = Vec::new();
        let mut writer = ogg::PacketWriter::new(std::io::Cursor::new(&mut out));
        writer.write_packet(head, serial, ogg::PacketWriteEndInfo::EndPage, 0).unwrap();
        writer.write_packet(tags, serial, ogg::PacketWriteEndInfo::EndPage, 0).unwrap();
        for i in 0..n_packets {
            let end =
                if i + 1 == n_packets { ogg::PacketWriteEndInfo::EndStream } else { ogg::PacketWriteEndInfo::NormalPacket };
            // 0xFC: CELT fullband 20 ms, stereo, code 0 -- with no payload it's one zero-length
            // (DTX) frame, which decodes to a frame of silence.
            writer.write_packet(vec![0xfc], serial, end, (i as u64 + 1) * 960).unwrap();
        }
        drop(writer);
        out
    }

    /// The reader moves through async fns whose futures embed several copies of it; if it grows
    /// big again (say an unboxed decoder or frame buffer), those futures can overflow default
    /// 2 MiB thread stacks in debug builds.
    #[test]
    fn opus_reader_stays_small() {
        assert!(std::mem::size_of::<OpusReader<std::fs::File>>() < 512);
    }

    #[test]
    fn seek_a_generated_opus_stream() {
        init();
        smol::block_on(async {
            let minute = 60 * 48000u64;
            let data = silence_opus(9000, &[]); // 3 minutes of silence
            let f = FromFutures::new(smol::io::Cursor::new(&data[..]));
            let opus = OggOpus::parse_seekable(f, |_| {}).await.unwrap();
            let len = opus.format.len_samples.unwrap();
            assert_eq!(len, 9000 * 960 - 312); // total decoded minus the pre-skip
            let mut samples = opus.samples;

            type TestReader<'a> = OpusReader<FromFutures<smol::io::Cursor<&'a [u8]>>>;
            let remaining_after_seek_to = async |samples: &mut TestReader<'_>, target: u64| {
                let pos = samples.seek_samples(target).await.unwrap();
                let mut remaining: u64 = 0;
                loop {
                    let mut buf = [Default::default(); 512];
                    let nread = Source::<sample::Stereo<sample::PcmS16Le>>::read_samples(samples, &mut buf).await.unwrap();
                    if nread == 0 {
                        break;
                    }
                    remaining += nread as u64;
                }
                (pos, remaining)
            };

            // Bisection seek forward.
            let (pos, remaining) = remaining_after_seek_to(&mut samples, 2 * minute).await;
            assert_eq!(pos, 2 * minute);
            assert_eq!(remaining, len - pos);
            // Seek backward (and to a target so early it takes the decode-from-start path).
            let (pos, remaining) = remaining_after_seek_to(&mut samples, 1000).await;
            assert_eq!(pos, 1000);
            assert_eq!(remaining, len - pos);
            // Seek past the end of the stream.
            let (pos, remaining) = remaining_after_seek_to(&mut samples, len + minute).await;
            assert_eq!(pos, len);
            assert_eq!(remaining, 0);
        })
    }

    #[test]
    fn parse_an_opus_file() {
        init();
        smol::block_on(async {
            let comments = [("TITLE", "Silent Night"), ("ARTIST", "Nobody"), ("ALBUM", "Quiet Sessions")];
            let data = silence_opus(500, &comments); // 10 s
            let f = FromFutures::new(smol::io::Cursor::new(&data[..]));
            let mut tags = Tags::default();
            let opus = OggOpus::parse_seekable(f, |tag| tags.set(tag)).await.unwrap();
            assert_eq!(tags.title, "Silent Night");
            assert_eq!(tags.artist, "Nobody");
            assert_eq!(tags.album, "Quiet Sessions");
            assert_eq!(opus.format.n_channels, 2);
            let len = opus.format.len_samples.expect("tail scan should find the last page granule");
            assert_eq!(len, 500 * 960 - 312); // total decoded minus the pre-skip
            let mut samples = opus.samples;
            let mut total: u64 = 0;
            loop {
                let mut buf = [Default::default(); 512];
                let nread = Source::<sample::Stereo<sample::PcmS16Le>>::read_samples(&mut samples, &mut buf).await.unwrap();
                if nread == 0 {
                    break;
                }
                total += nread as u64;
            }
            // We decode every frame in full while the granule-derived length excludes the final
            // frame's end padding, so the decoded total may exceed it by less than one frame.
            assert!(len <= total && total - len < 5760, "len_samples {len} vs decoded total {total}");
        })
    }
}
