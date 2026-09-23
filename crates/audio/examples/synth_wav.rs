//! Write a synthetic song to WAV: synth_wav <out.wav> <seed> <seconds>
fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().expect("usage: synth_wav <out.wav> <seed> <seconds>");
    let seed: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let secs: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30.0);
    cd_audio::synth::Song::from_seed(seed)
        .render(secs)
        .write_wav(std::path::Path::new(&out))
        .expect("write wav");
}
