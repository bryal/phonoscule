#![feature(iter_next_chunk, let_chains)]

pub mod metadata;
pub mod pcm;
pub mod wav;

#[cfg(test)]
mod test {
    use super::{metadata::*, pcm::*, wav::*};
    use std::{
        fs::File,
        io::{BufReader, Read},
        sync::Once,
    };

    static INIT: Once = Once::new();

    fn init() {
        INIT.call_once(|| {
            simple_logger::init().unwrap();
        })
    }

    #[test]
    fn parse_a_wav_file() {
        init();
        let f = BufReader::new(File::open("assets/Listless.wav").unwrap());
        let mut wav = WavStream::<StaticMetadata, _>::parse(f.bytes().map(|b| b.unwrap())).unwrap();
        assert_eq!(wav.format.n_channels, 2);
        assert_eq!(wav.metadata.title(), "Listless");
        assert_eq!(wav.metadata.album(), "Listless/Second Skin 2019 Single");
        assert_eq!(wav.metadata.artist(), "Siamese Twins");
        assert!(matches!(wav.format_samples(), Some(Samples::StereoS16(_))));
    }
}
