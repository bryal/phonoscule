#![allow(incomplete_features)]
#![feature(iter_next_chunk, let_chains, async_fn_in_trait)]

pub mod io;
pub mod metadata;
pub mod plumbing;
pub mod sample;
pub mod wav;

#[cfg(test)]
mod test {
    use super::{io::Skippable, metadata::*, plumbing::*, sample, wav::*};
    use embedded_io::adapters::FromTokio;
    use std::sync::Once;
    use tokio::{fs::File, io::BufReader};

    static INIT: Once = Once::new();

    fn init() {
        INIT.call_once(|| {
            simple_logger::init().unwrap();
        })
    }

    #[tokio::test(flavor = "current_thread")]
    async fn parse_a_wav_file() {
        init();
        let f = Skippable(FromTokio::new(BufReader::new(File::open("assets/Listless.wav").await.unwrap())));
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
    }
}
