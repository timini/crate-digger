//! The map's numerical work, independent of the database: exact nearest
//! neighbours, DBSCAN and a seeded 2D layout. Everything here is
//! deterministic for a fixed input order and configuration.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Unit vectors of one feature version, one row per track.
pub struct Matrix {
    pub dims: usize,
    pub data: Vec<f32>,
}

impl Matrix {
    pub fn len(&self) -> usize {
        self.data.len().checked_div(self.dims).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn row(&self, i: usize) -> &[f32] {
        &self.data[i * self.dims..(i + 1) * self.dims]
    }
}

/// Cosine distance between unit vectors: 1 minus the dot product, never below 0.
pub fn distance(a: &[f32], b: &[f32]) -> f32 {
    // Eight accumulators so the compiler can vectorise the loop.
    let mut acc = [0f32; 8];
    let (ca, ra) = a.as_chunks::<8>();
    let (cb, rb) = b.as_chunks::<8>();
    for (x, y) in ca.iter().zip(cb) {
        for k in 0..8 {
            acc[k] += x[k] * y[k];
        }
    }
    let tail: f32 = ra.iter().zip(rb).map(|(x, y)| x * y).sum();
    (1.0 - (acc.iter().sum::<f32>() + tail)).max(0.0)
}

/// Stops a build early; checked between rows and epochs.
pub struct Control<'a> {
    pub cancel: &'a AtomicBool,
    /// Percent done, 0 to 100.
    pub progress: &'a AtomicU32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl Control<'_> {
    fn check(&self) -> Result<(), Cancelled> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }

    fn report(&self, percent: u32) {
        self.progress.store(percent.min(100), Ordering::Relaxed);
    }
}

fn threads() -> usize {
    // Leave a core for playback and the interface.
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(1)
        .max(1)
}

/// Run `f` for every row on several threads and return the results in row order.
fn per_row<T: Send>(
    n: usize,
    ctl: &Control,
    span: (u32, u32),
    f: impl Fn(usize) -> T + Sync,
) -> Result<Vec<T>, Cancelled> {
    let workers = threads().min(n.max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::atomic::AtomicUsize::new(0);
    let mut parts: Vec<Vec<(usize, T)>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut out = vec![];
                    loop {
                        if ctl.cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        out.push((i, f(i)));
                        let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if d.is_multiple_of(64) {
                            ctl.report(span.0 + ((span.1 - span.0) as usize * d / n) as u32);
                        }
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("map worker"))
            .collect()
    });
    ctl.check()?;
    let mut all: Vec<(usize, T)> = parts.iter_mut().flat_map(std::mem::take).collect();
    all.sort_by_key(|(i, _)| *i);
    Ok(all.into_iter().map(|(_, t)| t).collect())
}

/// A neighbour: row index and cosine distance.
pub type Neighbour = (u32, f32);

/// The `k` nearest other rows of every row, nearest first. Exact: every
/// pair is compared, with ties broken by row index.
pub fn knn(m: &Matrix, k: usize, ctl: &Control) -> Result<Vec<Vec<Neighbour>>, Cancelled> {
    let n = m.len();
    per_row(n, ctl, (0, 45), |i| {
        let a = m.row(i);
        let mut best: Vec<Neighbour> = Vec::with_capacity(k + 1);
        for j in 0..n {
            if j == i {
                continue;
            }
            let d = distance(a, m.row(j));
            if best.len() == k && d >= best[k - 1].1 {
                continue;
            }
            let at = best.partition_point(|&(bj, bd)| bd < d || (bd == d && (bj as usize) < j));
            best.insert(at, (j as u32, d));
            best.truncate(k);
        }
        best
    })
}

/// DBSCAN's eps chosen from the data: the median distance from each track
/// to its (min_samples - 1)th nearest neighbour, so about half the tracks
/// are core points.
pub fn auto_eps(knn: &[Vec<Neighbour>], min_samples: usize) -> Option<f32> {
    let at = min_samples.saturating_sub(2);
    let mut d: Vec<f32> = knn.iter().filter_map(|l| l.get(at).map(|x| x.1)).collect();
    if d.is_empty() {
        return None;
    }
    d.sort_by(f32::total_cmp);
    Some(d[d.len() / 2])
}

struct UnionFind(Vec<u32>);

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind((0..n as u32).collect())
    }
    fn find(&mut self, mut x: u32) -> u32 {
        while self.0[x as usize] != x {
            self.0[x as usize] = self.0[self.0[x as usize] as usize];
            x = self.0[x as usize];
        }
        x
    }
    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        // The smaller index becomes the root, so results do not depend on order.
        if ra < rb {
            self.0[rb as usize] = ra;
        } else if rb < ra {
            self.0[ra as usize] = rb;
        }
    }
}

/// DBSCAN on cosine distance. A row is a core point when at least
/// `min_samples` rows, itself included, are within `eps`. Clusters are the
/// connected core points plus border rows, each border row joining its
/// nearest core point (ties by index). Other rows are noise (None).
/// Clusters are numbered from 0 in order of their first row.
///
/// `knn` must hold at least `min_samples - 1` neighbours per row.
pub fn dbscan(
    m: &Matrix,
    knn: &[Vec<Neighbour>],
    eps: f32,
    min_samples: usize,
    ctl: &Control,
) -> Result<Vec<Option<u32>>, Cancelled> {
    let n = m.len();
    let need = min_samples.saturating_sub(1);
    let core: Vec<bool> = knn
        .iter()
        .map(|l| need == 0 || l.get(need - 1).is_some_and(|x| x.1 <= eps))
        .collect();
    // Per row: core rows list their core neighbours within eps; other rows
    // their nearest core point within eps.
    let links = per_row(n, ctl, (45, 70), |i| {
        let a = m.row(i);
        let mut out = vec![];
        let mut nearest: Option<Neighbour> = None;
        for j in 0..n {
            if j == i || !core[j] || (core[i] && j < i) {
                continue;
            }
            let d = distance(a, m.row(j));
            if d > eps {
                continue;
            }
            if core[i] {
                out.push(j as u32);
            } else if nearest.is_none_or(|(_, nd)| d < nd) {
                nearest = Some((j as u32, d));
            }
        }
        if let Some((j, _)) = nearest {
            out.push(j);
        }
        out
    })?;
    let mut uf = UnionFind::new(n);
    for (i, l) in links.iter().enumerate() {
        if core[i] {
            for &j in l {
                uf.union(i as u32, j);
            }
        }
    }
    let mut label_of_root = std::collections::HashMap::new();
    let mut labels = vec![None; n];
    for i in 0..n {
        let root = if core[i] {
            uf.find(i as u32)
        } else if let Some(&j) = links[i].first() {
            uf.find(j)
        } else {
            continue;
        };
        let next = label_of_root.len() as u32;
        labels[i] = Some(*label_of_root.entry(root).or_insert(next));
    }
    Ok(labels)
}

/// SplitMix64: small, seeded and the same on every platform.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    }
}

/// The first two principal components, by power iteration from a seeded start.
fn pca2(m: &Matrix, seed: u64) -> Vec<[f32; 2]> {
    let (n, dims) = (m.len(), m.dims);
    let mut mean = vec![0f64; dims];
    for i in 0..n {
        for (s, x) in mean.iter_mut().zip(m.row(i)) {
            *s += *x as f64;
        }
    }
    mean.iter_mut().for_each(|s| *s /= n as f64);
    let mut rng = Rng(seed);
    let mut axes: Vec<Vec<f64>> = vec![];
    for _ in 0..2 {
        let mut v: Vec<f64> = (0..dims).map(|_| rng.unit() as f64).collect();
        for _ in 0..30 {
            // v = C v, with C the covariance, without forming C.
            let mut next = vec![0f64; dims];
            for i in 0..n {
                let row = m.row(i);
                let p: f64 = row
                    .iter()
                    .zip(&mean)
                    .zip(&v)
                    .map(|((x, mu), vi)| (*x as f64 - mu) * vi)
                    .sum();
                for ((o, x), mu) in next.iter_mut().zip(row).zip(&mean) {
                    *o += p * (*x as f64 - mu);
                }
            }
            for a in &axes {
                let p: f64 = next.iter().zip(a).map(|(x, y)| x * y).sum();
                next.iter_mut().zip(a).for_each(|(x, y)| *x -= p * y);
            }
            let norm = next.iter().map(|x| x * x).sum::<f64>().sqrt();
            if norm < 1e-12 {
                break;
            }
            v = next.into_iter().map(|x| x / norm).collect();
        }
        axes.push(v);
    }
    (0..n)
        .map(|i| {
            let row = m.row(i);
            let p = |a: &[f64]| -> f32 {
                row.iter()
                    .zip(&mean)
                    .zip(a)
                    .map(|((x, mu), ai)| (*x as f64 - mu) * ai)
                    .sum::<f64>() as f32
            };
            [p(&axes[0]), p(&axes[1])]
        })
        .collect()
}

/// Fuzzy neighbour weights as in UMAP: each row's distances are smoothed
/// so that its weights sum to log2(k), then made symmetric.
fn edge_weights(knn: &[Vec<Neighbour>], k: usize) -> Vec<(u32, u32, f32)> {
    let target = (k.max(2) as f32).log2();
    let mut directed = std::collections::HashMap::<(u32, u32), f32>::new();
    for (i, list) in knn.iter().enumerate() {
        let list = &list[..list.len().min(k)];
        let Some(rho) = list.first().map(|x| x.1) else {
            continue;
        };
        let (mut lo, mut hi, mut sigma) = (0f32, f32::INFINITY, 1f32);
        for _ in 0..64 {
            let sum: f32 = list.iter().map(|x| (-(x.1 - rho).max(0.0) / sigma).exp()).sum();
            if (sum - target).abs() < 1e-5 {
                break;
            }
            if sum > target {
                hi = sigma;
                sigma = (lo + hi) / 2.0;
            } else {
                lo = sigma;
                sigma = if hi.is_finite() {
                    (lo + hi) / 2.0
                } else {
                    sigma * 2.0
                };
            }
        }
        let sigma = sigma
            .max(1e-3 * list.iter().map(|x| x.1).sum::<f32>() / list.len() as f32)
            .max(1e-6);
        for &(j, d) in list {
            directed.insert((i as u32, j), (-(d - rho).max(0.0) / sigma).exp());
        }
    }
    let mut edges: Vec<(u32, u32, f32)> = vec![];
    for (&(i, j), &w) in &directed {
        let back = directed.get(&(j, i)).copied();
        if back.is_some() && j < i {
            continue; // counted from the other side
        }
        let b = back.unwrap_or(0.0);
        let (a, c) = if i < j { (i, j) } else { (j, i) };
        edges.push((a, c, w + b - w * b));
    }
    edges.sort_by_key(|e| (e.0, e.1));
    edges
}

/// 2D display coordinates: UMAP's layout objective (min_dist 0.1, spread
/// 1), optimised from a PCA start with a fixed seed. Screen distance only
/// approximates neighbourhoods; it does not measure similarity.
pub fn layout(
    m: &Matrix,
    knn: &[Vec<Neighbour>],
    k: usize,
    epochs: usize,
    seed: u64,
    ctl: &Control,
) -> Result<Vec<[f32; 2]>, Cancelled> {
    let n = m.len();
    if n <= 2 {
        return Ok((0..n).map(|i| [i as f32 * 2.0, 0.0]).collect());
    }
    let mut y = pca2(m, seed);
    let scale = y
        .iter()
        .flat_map(|p| p.iter().map(|v| v.abs()))
        .fold(0f32, f32::max);
    let scale = if scale > 0.0 { 10.0 / scale } else { 1.0 };
    let mut rng = Rng(seed ^ 0x5eed);
    for p in &mut y {
        // A little jitter separates identical rows.
        p[0] = p[0] * scale + rng.unit() * 1e-3;
        p[1] = p[1] * scale + rng.unit() * 1e-3;
    }
    let edges = edge_weights(knn, k);
    let wmax = edges.iter().map(|e| e.2).fold(0f32, f32::max);
    if wmax <= 0.0 {
        return Ok(y);
    }
    let per_sample: Vec<f32> = edges
        .iter()
        .map(|e| if e.2 > 0.0 { wmax / e.2 } else { f32::INFINITY })
        .collect();
    let mut next_at = per_sample.clone();
    let (a, b) = (1.577f32, 0.8951f32);
    const NEGATIVE: usize = 5;
    let clip = |g: f32| g.clamp(-4.0, 4.0);
    for epoch in 0..epochs {
        ctl.check()?;
        ctl.report(70 + (29 * epoch / epochs.max(1)) as u32);
        let alpha = 1.0 - epoch as f32 / epochs as f32;
        for (e, &(i, j, _)) in edges.iter().enumerate() {
            if next_at[e] > (epoch + 1) as f32 {
                continue;
            }
            next_at[e] += per_sample[e];
            let (i, j) = (i as usize, j as usize);
            let d = [y[i][0] - y[j][0], y[i][1] - y[j][1]];
            let d2 = d[0] * d[0] + d[1] * d[1];
            if d2 > 0.0 {
                let coeff = -2.0 * a * b * d2.powf(b - 1.0) / (a * d2.powf(b) + 1.0);
                for k in 0..2 {
                    let g = clip(coeff * d[k]) * alpha;
                    y[i][k] += g;
                    y[j][k] -= g;
                }
            }
            for _ in 0..NEGATIVE {
                let o = rng.below(n);
                if o == i {
                    continue;
                }
                let d = [y[i][0] - y[o][0], y[i][1] - y[o][1]];
                let d2 = d[0] * d[0] + d[1] * d[1];
                let coeff = 2.0 * b / ((0.001 + d2) * (a * d2.powf(b) + 1.0));
                for k in 0..2 {
                    let g = if coeff > 0.0 { clip(coeff * d[k]) } else { 4.0 };
                    y[i][k] += g * alpha;
                }
            }
        }
    }
    Ok(y)
}
