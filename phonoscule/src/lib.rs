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
    use super::{io::Skippable, metadata::*, opus::OggOpus, plumbing::*, sample, wav::*};
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
