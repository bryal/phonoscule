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
    use smol::{fs::File, io::BufReader};
    use std::sync::Once;

    static INIT: Once = Once::new();

    fn init() {
        INIT.call_once(|| {
            simple_logger::init().unwrap();
        })
    }

    #[test]
    fn parse_a_wav_file() {
        init();
        smol::block_on(async {
            let f =
                Skippable(FromFutures::new(BufReader::new(File::open("../assets/Listless-s16.wav").await.unwrap())));
            let wav = Wav::<StaticMetadata, _>::parse(f).await.unwrap();
            assert_eq!(wav.metadata.title(), "Listless");
            assert_eq!(wav.metadata.album(), "Listless/Second Skin 2019 Single");
            assert_eq!(wav.metadata.artist(), "Siamese Twins");
            let mut samples = match wav.samples {
                sample::MultiReader::StereoPcmS16(s) => s,
                _ => panic!("unexpected format, {:?}", wav.format),
            };
            let mut nleft = wav.format.len_samples();
            loop {
                let mut buf = [Default::default(); 32];
                let nread = samples.read_samples(&mut buf).await.unwrap();
                if nleft == 0 {
                    assert!(nread == 0, "wav format says no left, but we read another {nread}");
                    break;
                } else {
                    assert!(
                        nread > 0 && nread as u64 <= nleft,
                        "wav format says there are {nleft} left, but we read {nread}"
                    );
                    nleft -= nread as u64;
                }
            }
        })
    }

    /// Builds a valid, decodable Ogg Opus stream of stereo silence: `n_packets` zero-length
    /// (DTX) CELT frames of 20 ms each. Granule positions count raw decoded samples, of which
    /// the first `PRE_SKIP` are dropped (RFC 7845 §4).
    fn silence_opus(n_packets: usize) -> Vec<u8> {
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
        tags.extend(0u32.to_le_bytes()); // no comments

        let serial = 0x5eed;
        let mut out = Vec::new();
        let mut writer = ogg::PacketWriter::new(std::io::Cursor::new(&mut out));
        writer.write_packet(head, serial, ogg::PacketWriteEndInfo::EndPage, 0).unwrap();
        writer.write_packet(tags, serial, ogg::PacketWriteEndInfo::EndPage, 0).unwrap();
        for i in 0..n_packets {
            let end = if i + 1 == n_packets {
                ogg::PacketWriteEndInfo::EndStream
            } else {
                ogg::PacketWriteEndInfo::NormalPacket
            };
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
            let data = silence_opus(9000); // 3 minutes of silence
            let f = FromFutures::new(smol::io::Cursor::new(&data[..]));
            let opus = OggOpus::<StaticMetadata, _>::parse_seekable(f).await.unwrap();
            let len = opus.format.len_samples.unwrap();
            assert_eq!(len, 9000 * 960 - 312); // total decoded minus the pre-skip
            let mut samples = opus.samples;

            type TestReader<'a> = OpusReader<FromFutures<smol::io::Cursor<&'a [u8]>>>;
            let remaining_after_seek_to = async |samples: &mut TestReader<'_>, target: u64| {
                let pos = samples.seek_samples(target).await.unwrap();
                let mut remaining: u64 = 0;
                loop {
                    let mut buf = [Default::default(); 512];
                    let nread =
                        Source::<sample::Stereo<sample::PcmS16Le>>::read_samples(samples, &mut buf).await.unwrap();
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
            let f =
                Skippable(FromFutures::new(BufReader::new(File::open("../assets/Listless.opus").await.unwrap())));
            let opus = OggOpus::<StaticMetadata, _>::parse_seekable(f).await.unwrap();
            assert_eq!(opus.metadata.title(), "Listless");
            assert_eq!(opus.metadata.album(), "Listless/Second Skin 2019 Single");
            assert_eq!(opus.metadata.artist(), "Siamese Twins");
            assert_eq!(opus.format.n_channels, 2);
            let len = opus.format.len_samples.expect("tail scan should find the last page granule");
            let mut samples = opus.samples;
            let mut total: u64 = 0;
            loop {
                let mut buf = [Default::default(); 512];
                let nread =
                    Source::<sample::Stereo<sample::PcmS16Le>>::read_samples(&mut samples, &mut buf).await.unwrap();
                if nread == 0 {
                    break;
                }
                total += nread as u64;
            }
            // The track is 2:46 at 48 kHz.
            let rate = opus.format.sample_rate() as u64;
            assert!(total > 160 * rate && total < 170 * rate, "unexpected total sample count {total}");
            // We decode every frame in full while the granule-derived length excludes the final
            // frame's end padding, so the decoded total may exceed it by less than one frame.
            assert!(len <= total && total - len < 5760, "len_samples {len} vs decoded total {total}");
        })
    }
}
