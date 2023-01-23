const PLAYBACK_SAMPLE_RATE: u32 = 48000;

fn main() {
    let player = pulse_simple::Playback::<[i16; 2]>::new(
        "phonoscule-cli",
        "CLI-based application based on the Phonoscule music player library",
        None,
        PLAYBACK_SAMPLE_RATE,
    );

    // To begin with, let's just play a simple sine wave -- a pure tone -- using pulse-simple.
    for i in 0.. {
        let samples = std::array::from_fn::<_, 512, _>(|j| {
            let freq = 60.0;
            let vol = 0.2;
            let x =
                (i * 512 + j) as f64 * freq * std::f64::consts::TAU / PLAYBACK_SAMPLE_RATE as f64;
            let y = (x.sin() * vol * i16::MAX as f64) as i16;
            [y, y]
        });
        player.write(&samples)
    }
}
