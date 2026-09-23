use cd_audio::synth::Song;
fn main() {
    let dir = std::env::temp_dir().join("cd-analyzer-explore");
    std::fs::create_dir_all(&dir).unwrap();
    const NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
    for seed in 1..=12u64 {
        let s = Song::from_seed(seed);
        let p = dir.join(format!("s{seed}.wav"));
        s.render(100.0).write_wav(&p).unwrap();
        let a = cd_analyzer::analyse(&p, &[], &|_| {}).unwrap();
        let truth = format!("{} minor", NAMES[((9 + s.key) % 12) as usize]);
        println!(
            "seed {seed:2} bpm true {:5.1} got {:?} (conf {:.2}) | key true {truth:9} got {:?} | lufs {:?} | {} ms {} KB",
            s.bpm,
            a.tempo_bpm,
            a.tempo_confidence,
            a.key.as_ref().map(|k| (&k.name, &k.camelot, (k.confidence * 100.0).round())),
            a.loudness_lufs.map(|l| (l * 10.0).round() / 10.0),
            a.stats.wall_ms,
            a.stats.peak_rss_kb
        );
    }
}
