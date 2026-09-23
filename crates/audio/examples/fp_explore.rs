//! Print fingerprint comparisons between a synthetic song and its variants.
use cd_audio::fingerprint::{compare, compare_with_speed, fingerprint_samples};
use cd_audio::synth::{self, Song};

fn main() {
    let secs = 60.0;
    for seed in 1..=3u64 {
        let song = Song::from_seed(seed);
        let orig = song.render(secs);
        let fp = |a: &synth::Audio, s: f64| fingerprint_samples(&a.samples, synth::RATE, 2, s);
        let a = fp(&orig, 1.0);
        let variants: Vec<(&str, synth::Audio)> = vec![
            ("remaster", synth::remaster(&orig)),
            ("section 30-70%", synth::section(&orig, 0.3, 0.7)),
            ("cut 40-60%", synth::cut(&orig, 0.4, 0.6)),
            ("speed +2%", synth::speed(&orig, 1.02)),
            ("speed -4%", synth::speed(&orig, 0.96)),
            ("speed +8%", synth::speed(&orig, 1.08)),
            ("remix", song.remix(1).render(secs)),
            ("unrelated", Song::from_seed(seed + 100).render(secs)),
        ];
        for (name, v) in variants {
            let plain = compare(&a, &fp(&v, 1.0));
            let (best, sp) = compare_with_speed(&a, |s| Some(fp(&v, s)), v.duration_ms()).unwrap();
            println!(
                "seed {seed} {name:16} plain score {:.2} cov {:.2} | best score {:.2} cov {:.2} speed {:.3}",
                plain.score, plain.coverage, best.score, best.coverage, sp
            );
        }
    }
}
