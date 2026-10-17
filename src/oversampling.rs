//! Linear-phase 4x oversampling built from two cascaded half-band FIR stages.
//!
//! Used to run the soft saturator alias-free. Each stage is a Kaiser-windowed
//! half-band filter (`N = 4K + 3` taps): every other tap is zero and the centre
//! tap is exactly 0.5, so each polyphase branch is either a short dot product
//! or a pure delay. Coefficients are designed once at construction time; the
//! audio thread only runs fixed-size dot products on pre-allocated history.

use std::f64::consts::PI;

/// Stage 1 (1x <-> 2x): 127 taps, ~100 dB stopband, passband to ~19.8 kHz @ 44.1 kHz.
const STAGE1_TAPS: usize = 127;
/// Stage 2 (2x <-> 4x): 31 taps, ~100 dB stopband, passband well beyond base-rate Nyquist.
const STAGE2_TAPS: usize = 31;
/// Kaiser beta for ~100 dB stopband attenuation: 0.1102 * (100 - 8.7).
const KAISER_BETA: f64 = 10.06;

/// Round-trip latency of [`Oversampler4x`] in base-rate samples.
///
/// Stage 1 contributes `(N1 - 1) / 2 = 63` samples. Stage 2 contributes
/// `(N2 - 1) / 4 = 7.5` samples, padded to 8 with a 2-sample delay at the 4x rate
/// so the total stays an integer the host can compensate exactly.
pub const OVERSAMPLER_LATENCY: u32 = ((STAGE1_TAPS - 1) / 2 + (STAGE2_TAPS - 1) / 4 + 1) as u32;

/// Zeroth-order modified Bessel function of the first kind (power series).
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half_x = x * 0.5;
    for k in 1..64 {
        let f = half_x / f64::from(k);
        term *= f * f;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Design the non-zero ("even index") taps of a Kaiser-windowed half-band filter.
///
/// Returns `(N + 1) / 2` taps normalised to sum to exactly 0.5, so together with
/// the 0.5 centre tap the filter has unity DC gain in both polyphase branches.
fn design_halfband(num_taps: usize) -> Vec<f32> {
    debug_assert!(num_taps % 4 == 3, "half-band length must be 4K + 3");
    let centre = (num_taps - 1) / 2;
    let denom = bessel_i0(KAISER_BETA);
    let mut taps: Vec<f64> = (0..num_taps)
        .step_by(2)
        .map(|j| {
            let n = j as f64 - centre as f64;
            let ideal = (PI * n * 0.5).sin() / (PI * n);
            let r = 2.0 * j as f64 / (num_taps - 1) as f64 - 1.0;
            let window = bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / denom;
            ideal * window
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    for t in &mut taps {
        *t *= 0.5 / sum;
    }
    taps.iter().map(|&t| t as f32).collect()
}

/// Dot product with four independent accumulators so LLVM can emit NEON/SSE FMAs.
#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0.0f32; 4];
    let (chunks_a, rem_a) = a.as_chunks::<4>();
    let (chunks_b, rem_b) = b.as_chunks::<4>();
    let tail: f32 = rem_a.iter().zip(rem_b).map(|(x, y)| x * y).sum();
    for (ca, cb) in chunks_a.iter().zip(chunks_b) {
        for k in 0..4 {
            acc[k] += ca[k] * cb[k];
        }
    }
    (acc[0] + acc[1]) + (acc[2] + acc[3]) + tail
}

/// Fixed-length history with a mirrored backing store, so the newest `len`
/// samples are always one contiguous slice (newest first) with no wrap-around.
struct History {
    buf: Vec<f32>,
    len: usize,
    pos: usize,
}

impl History {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; 2 * len],
            len,
            pos: 0,
        }
    }

    #[inline]
    fn push(&mut self, x: f32) {
        self.pos = if self.pos == 0 {
            self.len - 1
        } else {
            self.pos - 1
        };
        self.buf[self.pos] = x;
        self.buf[self.pos + self.len] = x;
    }

    /// `[x[n], x[n-1], ..., x[n-len+1]]`.
    #[inline]
    fn slice(&self) -> &[f32] {
        &self.buf[self.pos..self.pos + self.len]
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }
}

/// One half-band stage, usable as a 2x interpolator or a 2x decimator.
struct HalfbandStage {
    /// Non-zero taps `h[0], h[2], ..., h[N-1]` (time-reversed == same, symmetric).
    taps: Vec<f32>,
    /// Centre-tap delay in low-rate samples: `K` where `centre = 2K + 1`.
    k: usize,
    /// Interpolator: input history. Decimator: even-phase history.
    even: History,
    /// Decimator only: odd-phase history (feeds the 0.5 centre tap).
    odd: History,
}

impl HalfbandStage {
    fn new(num_taps: usize) -> Self {
        let taps = design_halfband(num_taps);
        let k = (num_taps - 3) / 4;
        let m = taps.len();
        Self {
            taps,
            k,
            even: History::new(m),
            odd: History::new(k + 2),
        }
    }

    /// Interpolate one low-rate sample into two high-rate samples.
    #[inline]
    fn upsample(&mut self, x: f32) -> [f32; 2] {
        self.even.push(x);
        let hist = self.even.slice();
        // Zero-stuffing halves the energy; the factor 2 restores unity gain.
        let y0 = 2.0 * dot(&self.taps, hist);
        let y1 = hist[self.k]; // 2 * 0.5 * x[n - K]
        [y0, y1]
    }

    /// Decimate two high-rate samples (`v[2n]`, `v[2n+1]`) into one low-rate sample.
    #[inline]
    fn downsample(&mut self, v: [f32; 2]) -> f32 {
        self.even.push(v[0]);
        let y = dot(&self.taps, self.even.slice()) + 0.5 * self.odd.slice()[self.k];
        self.odd.push(v[1]);
        y
    }

    fn clear(&mut self) {
        self.even.clear();
        self.odd.clear();
    }
}

/// 4x up -> nonlinearity -> 4x down, per channel.
pub struct Oversampler4x {
    up1: HalfbandStage,
    up2: HalfbandStage,
    down2: HalfbandStage,
    down1: HalfbandStage,
    /// 2-sample delay at the 4x rate that rounds the latency up to an integer.
    pad: [f32; 2],
}

impl Default for Oversampler4x {
    fn default() -> Self {
        Self {
            up1: HalfbandStage::new(STAGE1_TAPS),
            up2: HalfbandStage::new(STAGE2_TAPS),
            down2: HalfbandStage::new(STAGE2_TAPS),
            down1: HalfbandStage::new(STAGE1_TAPS),
            pad: [0.0; 2],
        }
    }
}

impl Oversampler4x {
    pub fn reset(&mut self) {
        self.up1.clear();
        self.up2.clear();
        self.down2.clear();
        self.down1.clear();
        self.pad = [0.0; 2];
    }

    /// Run `f` at 4x the base rate on one base-rate sample.
    #[inline]
    pub fn process(&mut self, x: f32, mut f: impl FnMut(f32) -> f32) -> f32 {
        let [a, b] = self.up1.upsample(x);
        let [s0, s1] = self.up2.upsample(a);
        let [s2, s3] = self.up2.upsample(b);
        let (s0, s1, s2, s3) = (f(s0), f(s1), f(s2), f(s3));
        let (p0, p1) = (self.pad[0], self.pad[1]);
        self.pad = [s2, s3];
        let a = self.down2.downsample([p0, p1]);
        let b = self.down2.downsample([s0, s1]);
        self.down1.downsample([a, b])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halfband_taps_have_unity_dc() {
        for n in [STAGE1_TAPS, STAGE2_TAPS] {
            let taps = design_halfband(n);
            let sum: f32 = taps.iter().sum();
            assert!((sum - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn impulse_emerges_after_reported_latency() {
        let mut os = Oversampler4x::default();
        let mut out = Vec::new();
        for i in 0..256 {
            out.push(os.process(if i == 0 { 1.0 } else { 0.0 }, |s| s));
        }
        let peak = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap()
            .0;
        assert_eq!(peak as u32, OVERSAMPLER_LATENCY);
    }

    #[test]
    fn passband_is_flat_and_linear() {
        // A 1 kHz sine at 48 kHz must come back with < 0.01 dB error.
        let mut os = Oversampler4x::default();
        let sr = 48_000.0f32;
        let lat = OVERSAMPLER_LATENCY as usize;
        let n = 4800;
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin() * 0.5)
            .collect();
        let output: Vec<f32> = input.iter().map(|&x| os.process(x, |s| s)).collect();
        let max_err = (1000..n)
            .map(|i| (output[i] - input[i - lat]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_err < 0.5 * 0.0012, "max error {max_err}");
    }
}
