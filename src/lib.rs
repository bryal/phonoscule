#![feature(iter_next_chunk)]

mod metadata;
mod wav;

#[cfg(test)]
mod test {
    use super::{metadata::*, wav::*};
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
        let wav = WavStream::<StaticMetadata>::parse(f.bytes().map(|b| b.unwrap())).unwrap();
        assert_eq!(wav.metadata.title(), "Listless");
        assert_eq!(wav.metadata.album(), "Listless/Second Skin 2019 Single");
        assert_eq!(wav.metadata.artist(), "Siamese Twins");
    }
}
