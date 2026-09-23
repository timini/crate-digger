//! Tempo from the onset envelope's autocorrelation.

use crate::frames::Frame;

/// Estimated tempo in BPM and a 0..1 confidence. `fps` is frames per second.
pub fn estimate(frames: &[Frame], fps: f32) -> Option<(f32, f32)> {
    if frames.len() < (fps * 8.0) as usize {
        return None;
    }
    // Onset envelope minus its local (two-second, centred) mean, keeping
    // only rises. A window of several beats removes slow loudness changes
    // without blurring the beat grid.
    let env: Vec<f32> = frames.iter().map(|f| f.onset).collect();
    let half = fps as usize;
    let mut prefix = Vec::with_capacity(env.len() + 1);
    prefix.push(0.0f64);
    for v in &env {
        prefix.push(prefix.last().unwrap() + *v as f64);
    }
    let detrended: Vec<f32> = (0..env.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(half), (i + half + 1).min(env.len()));
            let mean = ((prefix[b] - prefix[a]) / (b - a) as f64) as f32;
            (env[i] - mean).max(0.0)
        })
        .collect();

    let lag_for = |bpm: f32| fps * 60.0 / bpm;
    let (min_lag, max_lag) = (lag_for(200.0).floor() as usize, lag_for(60.0).ceil() as usize);
    let n = detrended.len();
    // Autocorrelation up to eight beats at the slowest tempo.
    let top = (max_lag * 8 + 2).min(n - 1);
    let mut ac = vec![0f32; top + 1];
    for (lag, a) in ac.iter_mut().enumerate().skip(1) {
        *a = (0..n - lag)
            .map(|i| detrended[i] * detrended[i + lag])
            .sum::<f32>()
            / (n - lag) as f32;
    }
    // Comb score: the true beat also repeats at two to eight beats, which
    // off-beat patterns (at 2/3, 4/5 or 4/3 of the tempo) mostly do not. A log-normal prior an octave wide
    // around 120 BPM settles half- and double-tempo ties.
    let at = |x: f32| -> f32 {
        let i = x.floor() as usize;
        if i + 1 >= ac.len() {
            return 0.0;
        }
        let f = x - i as f32;
        ac[i] * (1.0 - f) + ac[i + 1] * f
    };
    let score = |lag: f32| -> f32 {
        let comb: f32 = (1..=8).map(|k| at(lag * k as f32)).sum();
        let bpm = 60.0 * fps / lag;
        comb * (-0.5 * ((bpm / 120.0).log2() / 0.9).powi(2)).exp()
    };
    // Beats are rarely a whole number of frames long, and a small error
    // grows eightfold across the comb, so search fractional lags.
    let (lo, hi) = (min_lag.max(2) as f32, max_lag.min(ac.len() / 8) as f32);
    let steps = ((hi - lo) / 0.05) as usize;
    let best = (0..=steps)
        .map(|i| lo + i as f32 * 0.05)
        .max_by(|a, b| score(*a).total_cmp(&score(*b)))?;
    if at(best) <= 0.0 {
        return None;
    }
    let bpm = 60.0 * fps / best;
    let best_bin = best.round() as usize;

    let mut sorted: Vec<f32> = ac[min_lag.max(1)..=max_lag.min(ac.len() - 1)].to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let median = sorted[sorted.len() / 2];
    let confidence = ((ac[best_bin] - median) / ac[best_bin].max(1e-12)).clamp(0.0, 1.0);
    Some(((bpm * 10.0).round() / 10.0, confidence))
}
