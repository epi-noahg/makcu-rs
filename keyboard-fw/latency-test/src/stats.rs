//! Distribution statistics for a set of latency samples (microseconds).
//!
//! Jitter is reported as the sample standard deviation, which is immune to any
//! constant offset in the measurement (e.g. the fixed serial-channel delay of
//! the SYNC clock-alignment path). Min/median/p95/p99/max bound the tails.

#[derive(Debug, Clone, Default)]
pub struct Samples {
    v: Vec<f64>,
}

#[derive(Debug, Clone, Copy)]
pub struct Summary {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    /// Sample standard deviation — the jitter metric.
    pub stddev: f64,
}

impl Samples {
    pub fn new() -> Self {
        Samples { v: Vec::new() }
    }

    pub fn push(&mut self, x: f64) {
        self.v.push(x);
    }

    pub fn len(&self) -> usize {
        self.v.len()
    }

    pub fn mean(&self) -> f64 {
        if self.v.is_empty() {
            return 0.0;
        }
        self.v.iter().sum::<f64>() / self.v.len() as f64
    }

    /// Sample standard deviation (Bessel-corrected, n-1).
    pub fn stddev(&self) -> f64 {
        let n = self.v.len();
        if n < 2 {
            return 0.0;
        }
        let m = self.mean();
        let var = self.v.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (n - 1) as f64;
        var.sqrt()
    }

    /// Linear-interpolated percentile (`p` in 0..=100) on a sorted copy.
    pub fn percentile(&self, p: f64) -> f64 {
        if self.v.is_empty() {
            return 0.0;
        }
        let mut s = self.v.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if s.len() == 1 {
            return s[0];
        }
        let rank = (p / 100.0) * (s.len() - 1) as f64;
        let lo = rank.floor() as usize;
        let hi = rank.ceil() as usize;
        if lo == hi {
            s[lo]
        } else {
            let frac = rank - lo as f64;
            s[lo] * (1.0 - frac) + s[hi] * frac
        }
    }

    pub fn summary(&self) -> Summary {
        let mut s = self.v.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        Summary {
            count: s.len(),
            min: s.first().copied().unwrap_or(0.0),
            max: s.last().copied().unwrap_or(0.0),
            mean: self.mean(),
            median: self.percentile(50.0),
            p95: self.percentile(95.0),
            p99: self.percentile(99.0),
            stddev: self.stddev(),
        }
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "n={:<6} min={:>8.1} med={:>8.1} mean={:>8.1} p95={:>8.1} p99={:>8.1} max={:>8.1} jitter(sd)={:>7.1}  [µs]",
            self.count, self.min, self.median, self.mean, self.p95, self.p99, self.max, self.stddev
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_stats() {
        let mut s = Samples::new();
        for x in [1.0, 2.0, 3.0, 4.0, 5.0] {
            s.push(x);
        }
        let sum = s.summary();
        assert_eq!(sum.count, 5);
        assert_eq!(sum.min, 1.0);
        assert_eq!(sum.max, 5.0);
        assert!((sum.mean - 3.0).abs() < 1e-9);
        assert!((sum.median - 3.0).abs() < 1e-9);
        // population values 1..5: sample sd = sqrt(2.5) ≈ 1.5811
        assert!((sum.stddev - 1.5811388).abs() < 1e-5);
    }

    #[test]
    fn percentile_interpolation() {
        let mut s = Samples::new();
        for x in 0..=100 {
            s.push(x as f64);
        }
        assert!((s.percentile(50.0) - 50.0).abs() < 1e-9);
        assert!((s.percentile(95.0) - 95.0).abs() < 1e-9);
        assert!((s.percentile(99.0) - 99.0).abs() < 1e-9);
        assert_eq!(s.percentile(0.0), 0.0);
        assert_eq!(s.percentile(100.0), 100.0);
    }

    #[test]
    fn jitter_is_offset_invariant() {
        // A constant added to every sample must not change the stddev (jitter).
        let mut a = Samples::new();
        let mut b = Samples::new();
        for x in [10.0, 12.0, 9.0, 11.0, 13.0] {
            a.push(x);
            b.push(x + 5000.0);
        }
        assert!((a.stddev() - b.stddev()).abs() < 1e-9);
    }

    #[test]
    fn empty_is_safe() {
        let s = Samples::new();
        let sum = s.summary();
        assert_eq!(sum.count, 0);
        assert_eq!(sum.stddev, 0.0);
    }
}
