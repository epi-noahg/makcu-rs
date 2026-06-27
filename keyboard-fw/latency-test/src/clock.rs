//! Aligns LEFT's `esp_timer` clock (firmware µs) to the host clock (ns) using
//! the SYNC beacons, so a firmware capture timestamp can be expressed on the
//! host timeline and compared against the HID arrival time.
//!
//! Two clocks on independent crystals differ in **offset** and **rate** (a few
//! ppm). We estimate them separately:
//!
//!   * slope (rate) — least squares over the **whole** retained history. A long
//!     baseline makes the slope estimate robust to the per-beacon serial jitter
//!     (slope error ≈ y-noise / time-span).
//!   * offset       — median over the **most recent** beacons, so slow drift is
//!     tracked while spikes are rejected.
//!
//! Within a beacon interval (50 ms) residual drift is < ~2 ns, so mapping is
//! effectively exact for the latencies we measure. A constant serial-path delay
//! D on the SYNC channel shows up as a constant bias in absolute latency (it
//! cancels out of jitter); see the README.

const DEFAULT_HISTORY: usize = 4096;
const DEFAULT_OFFSET_WINDOW: usize = 32;

pub struct ClockModel {
    base_fw_us: i64,
    base_host_ns: i64,
    have_base: bool,
    // Deltas relative to base, kept small for f64 precision.
    dx: Vec<f64>, // fw µs - base
    dy: Vec<f64>, // host ns - base
    history: usize,
    offset_window: usize,
}

impl ClockModel {
    pub fn new() -> Self {
        Self::with_params(DEFAULT_HISTORY, DEFAULT_OFFSET_WINDOW)
    }

    pub fn with_params(history: usize, offset_window: usize) -> Self {
        ClockModel {
            base_fw_us: 0,
            base_host_ns: 0,
            have_base: false,
            dx: Vec::new(),
            dy: Vec::new(),
            history,
            offset_window,
        }
    }

    /// Record a SYNC beacon: its firmware emit time and the host time at which
    /// the record arrived.
    pub fn add(&mut self, fw_us: i64, host_ns: i64) {
        if !self.have_base {
            self.base_fw_us = fw_us;
            self.base_host_ns = host_ns;
            self.have_base = true;
        }
        self.dx.push((fw_us - self.base_fw_us) as f64);
        self.dy.push((host_ns - self.base_host_ns) as f64);
        if self.dx.len() > self.history {
            let drop = self.dx.len() - self.history;
            self.dx.drain(0..drop);
            self.dy.drain(0..drop);
        }
    }

    pub fn points(&self) -> usize {
        self.dx.len()
    }

    /// ns-per-µs rate (≈ 1000). `None` until there is usable span.
    pub fn slope(&self) -> Option<f64> {
        let n = self.dx.len();
        if n < 2 {
            return None;
        }
        let xbar = self.dx.iter().sum::<f64>() / n as f64;
        let ybar = self.dy.iter().sum::<f64>() / n as f64;
        let mut sxx = 0.0;
        let mut sxy = 0.0;
        for i in 0..n {
            let dxc = self.dx[i] - xbar;
            sxx += dxc * dxc;
            sxy += dxc * (self.dy[i] - ybar);
        }
        if sxx <= 0.0 {
            return None; // all beacons at the same instant — no span yet
        }
        Some(sxy / sxx)
    }

    /// Median of recent residuals `dy - slope*dx`, the current offset term.
    fn offset(&self, slope: f64) -> f64 {
        let n = self.dx.len();
        let start = n.saturating_sub(self.offset_window);
        let mut res: Vec<f64> = (start..n).map(|i| self.dy[i] - slope * self.dx[i]).collect();
        res.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let m = res.len();
        if m == 0 {
            0.0
        } else if m % 2 == 1 {
            res[m / 2]
        } else {
            0.5 * (res[m / 2 - 1] + res[m / 2])
        }
    }

    /// Map a firmware timestamp (µs) to host time (ns). `None` until aligned.
    pub fn map_fw_us_to_host_ns(&self, fw_us: i64) -> Option<f64> {
        let slope = self.slope()?;
        let c = self.offset(slope);
        let dx = (fw_us - self.base_fw_us) as f64;
        Some(self.base_host_ns as f64 + slope * dx + c)
    }
}

impl Default for ClockModel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_slope_and_offset_with_noise() {
        // Truth: host_ns = 1000.04 * fw_us + 5_000_000 (+ serial jitter).
        let a_true = 1000.04_f64;
        let b_true = 5_000_000.0_f64;
        let mut m = ClockModel::new();
        // Deterministic pseudo-noise so the test is stable.
        let mut seed = 12345u64;
        let mut noise = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as f64 / (1u64 << 31) as f64 - 1.0) * 100_000.0 // ±100 µs of ns jitter
        };
        // 60 s of beacons at 50 ms.
        for k in 0..1200i64 {
            let fw = 1_000_000 + k * 50_000; // µs
            let host = (a_true * fw as f64 + b_true + noise()).round() as i64;
            m.add(fw, host);
        }
        let slope = m.slope().unwrap();
        assert!((slope - a_true).abs() < 0.02, "slope {} off", slope);

        // Map points right where real keypresses fall: between the most recent
        // beacons, ~25 ms after the last one. Average the absolute error over a
        // sweep so the assertion is stable under the median-offset estimator.
        let mut worst = 0.0_f64;
        for j in 0..40i64 {
            let fw_test = 1_000_000 + 1199 * 50_000 - j * 1_000; // last 40 ms
            let host_mapped = m.map_fw_us_to_host_ns(fw_test).unwrap();
            let host_truth = a_true * fw_test as f64 + b_true;
            worst = worst.max((host_mapped - host_truth).abs());
        }
        assert!(worst < 100_000.0, "worst near-recent map error {} ns", worst);
    }

    #[test]
    fn unaligned_returns_none() {
        let m = ClockModel::new();
        assert!(m.map_fw_us_to_host_ns(123).is_none());
        let mut m2 = ClockModel::new();
        m2.add(1000, 1_000_000);
        assert!(m2.map_fw_us_to_host_ns(1000).is_none()); // single point: no span
    }

    #[test]
    fn history_is_bounded() {
        let mut m = ClockModel::with_params(100, 16);
        for k in 0..1000i64 {
            m.add(k * 1000, k * 1_000_000);
        }
        assert!(m.points() <= 100);
        // Still maps sanely (slope ≈ 1000 ns/µs).
        let s = m.slope().unwrap();
        assert!((s - 1000.0).abs() < 1.0);
    }
}
