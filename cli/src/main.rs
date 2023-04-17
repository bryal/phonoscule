#![feature(iter_array_chunks)]

use phonoscule::{metadata::*, wav::*};
use std::{
    fs::File,
    io::{BufReader, Read},
};

const PLAYBACK_SAMPLE_RATE: u32 = 48000;

fn main() {
    let player = pulse_simple::Playback::<[i16; 2]>::new(
        "phonoscule-cli",
        "CLI-based application based on the Phonoscule music player library",
        None,
        PLAYBACK_SAMPLE_RATE,
    );

    let f = BufReader::new(File::open("assets/Listless.wav").unwrap());
    let mut wav = WavStream::<StaticMetadata, _>::parse(f.bytes().map(|b| b.unwrap())).unwrap();
    let mut chunks =
        wav.format_samples().expect("format should be supported").convert::<[i16; 2]>().array_chunks::<256>();
    for samples in &mut chunks {
        player.write(&samples)
    }
    player.write(chunks.into_remainder().unwrap().as_slice())
}
