//! DSP core for Magnitude.
//!
//! Signal flow (per channel, recovered from the original 2006 SynthMaker schematic):
//!
//! ```text
//! in ─┬─────────────────────────────── lookahead delay ── dry ──┐
//!     │                                                         ▼
//!     └─ [SC HPF] ─ |x| ─ [link] ─ envelope ─ gain computer ─ g ─► dry·(1 − a + a·g) ─ out gain ─ soft clip ─ out
//! ```
//!
//! The envelope follower, transfer curve and soft clipper reproduce the original
//! equations; the lookahead glide, sidechain HPF, stereo link, Color control and 4x
//! oversampled clipper are modern additions that default to the original behaviour.

use crate::oversampling::{OVERSAMPLER_LATENCY, Oversampler4x};

/// Compression threshold from the original schematic (`Threshold` = -50.0).
pub const THRESHOLD_DB: f32 = -50.0;

/// Compression ratio from the original schematic (`Ratio` = 0.5994, applied in dB space).
pub const RATIO: f32 = 0.5994;

/// Attack time from the original schematic (`Attack` = 0.0002 s).
pub const ATTACK_TIME_SEC: f32 = 0.0002;

/// Release time from the original schematic (`Release` = 0.25 s).
pub const RELEASE_TIME_SEC: f32 = 0.25;

/// Longest lookahead the plugin offers.
pub const MAX_LOOKAHEAD_MS: f32 = 100.0;

/// Time constant of the click-free glide between lookahead settings.
const LOOKAHEAD_GLIDE_SEC: f64 = 0.03;

/// Sidechain HPF frequencies at or below this are treated as "off".
pub const SC_HPF_OFF_HZ: f32 = 20.5;

/// One-pole coefficient using SynthMaker's `co_eff_calc` convention: `time` is how
/// long the follower takes to decay to 1 % (-40 dB), i.e. `c = 0.01^(1 / (time·fs))`.
///
/// The schematic caches the coefficients computed at 44.1 kHz (0.593257 and
/// 0.999582), which match this formula exactly. Using `exp(-1 / (time·fs))` instead
/// makes the release ~4.6x slower than the original (250 ms vs ~54 ms time constant).
#[must_use]
pub fn one_percent_coefficient(time_sec: f32, sample_rate: f64) -> f32 {
    (0.01f64.ln() / (f64::from(time_sec) * sample_rate)).exp() as f32
}

/// Static gain of the compressed (wet) path for a detector level in linear units.
///
/// Above the threshold the level is reduced by `(1 - RATIO)` dB per dB of overshoot,
/// i.e. `g = (env / thr)^-(1 - RATIO)`.
#[inline]
#[must_use]
pub fn wet_gain(env: f32, inv_threshold: f32) -> f32 {
    let rel = env * inv_threshold;
    if rel <= 1.0 {
        1.0
    } else {
        (-(1.0 - RATIO) * rel.log2()).exp2()
    }
}

/// Static input -> output curve of the whole processor (before the soft clipper),
/// in dB. Used by the editor's transfer display.
#[must_use]
pub fn static_curve_db(input_db: f32, amount: f32) -> f32 {
    let inv_thr = 10f32.powf(-THRESHOLD_DB / 20.0);
    let g = wet_gain(10f32.powf(input_db / 20.0), inv_thr);
    input_db + 20.0 * (1.0 - amount + amount * g).max(1e-6).log10()
}

/// Original soft saturator generalised with the schematic's `clip` input:
/// `y = x / (1 + clip·|x|)`. The original runs with `clip = 1.0`.
#[inline]
fn soft_clip(x: f32, clip: f32) -> f32 {
    x / (1.0 + clip * x.abs())
}

/// RBJ 2nd-order Butterworth high-pass (TDF-II, f64 state) for the detector path.
#[derive(Default, Clone, Copy)]
struct Highpass {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Highpass {
    fn set(&mut self, freq: f32, sample_rate: f64) {
        let w0 = 2.0 * std::f64::consts::PI * f64::from(freq) / sample_rate;
        let alpha = w0.sin() / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let cos = w0.cos();
        let a0 = 1.0 + alpha;
        self.b0 = (1.0 + cos) * 0.5 / a0;
        self.b1 = -(1.0 + cos) / a0;
        self.b2 = self.b0;
        self.a1 = -2.0 * cos / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let x = f64::from(x);
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y as f32
    }

    fn clear(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// Settings that change at most once per block.
#[derive(Clone, Copy, Debug)]
pub struct BlockSettings {
    /// Lookahead in milliseconds (0..=100).
    pub lookahead_ms: f32,
    /// Stereo detector link, 0 (independent, as the original) ..= 1 (fully linked).
    pub link: f32,
    /// Sidechain high-pass cutoff in Hz; `<= SC_HPF_OFF_HZ` disables it.
    pub sc_hpf_hz: f32,
    /// Run the soft clipper 4x oversampled.
    pub hq: bool,
}

impl Default for BlockSettings {
    fn default() -> Self {
        Self {
            lookahead_ms: 50.0,
            link: 0.0,
            sc_hpf_hz: 20.0,
            hq: false,
        }
    }
}

/// Smoothed per-sample controls.
#[derive(Clone, Copy, Debug)]
pub struct Controls {
    /// Parallel amount, 0 (dry) ..= 1 (fully compressed).
    pub amount: f32,
    /// Output gain, linear.
    pub out_gain: f32,
    /// Soft clipper drive, 0 (clean) ..= 1 (original).
    pub color: f32,
}

/// One processed stereo frame.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub left: f32,
    pub right: f32,
    /// Wet-path gain (linear, <= 1) applied to each channel.
    pub gain: [f32; 2],
    /// Detector envelope (linear) of each channel.
    pub env: [f32; 2],
}

/// Complete DSP state; every buffer is allocated in [`MagnitudeDsp::reset`].
pub struct MagnitudeDsp {
    sample_rate: f64,
    delay: [Vec<f32>; 2],
    delay_mask: usize,
    write_pos: usize,
    /// Lookahead target in whole samples (what the host is told as latency).
    delay_target: f32,
    /// Gliding lookahead in (fractional) samples.
    delay_current: f32,
    delay_glide: f32,
    env: [f32; 2],
    c_attack: f32,
    c_release: f32,
    inv_threshold: f32,
    link: f32,
    sc_hpf: [Highpass; 2],
    sc_hpf_hz: f32,
    hq: bool,
    oversampler: [Oversampler4x; 2],
}

impl Default for MagnitudeDsp {
    fn default() -> Self {
        Self::new(44_100.0)
    }
}

impl MagnitudeDsp {
    /// Create a DSP instance with buffers sized for `sample_rate`.
    #[must_use]
    pub fn new(sample_rate: f64) -> Self {
        let mut dsp = Self {
            sample_rate: 0.0,
            delay: [Vec::new(), Vec::new()],
            delay_mask: 0,
            write_pos: 0,
            delay_target: 0.0,
            delay_current: 0.0,
            delay_glide: 0.0,
            env: [0.0; 2],
            c_attack: 0.0,
            c_release: 0.0,
            inv_threshold: 10f32.powf(-THRESHOLD_DB / 20.0),
            link: 0.0,
            sc_hpf: [Highpass::default(); 2],
            sc_hpf_hz: 0.0,
            hq: false,
            oversampler: [Oversampler4x::default(), Oversampler4x::default()],
        };
        dsp.reset(sample_rate);
        dsp
    }

    /// Re-size buffers for `sample_rate` and clear all state. Not real-time safe.
    pub fn reset(&mut self, sample_rate: f64) {
        self.sample_rate = if sample_rate > 1000.0 {
            sample_rate
        } else {
            44_100.0
        };
        // Power-of-two ring so wrap-around is a mask; +4 for the interpolator taps.
        let max_delay =
            (f64::from(MAX_LOOKAHEAD_MS) * 0.001 * self.sample_rate).ceil() as usize + 4;
        let len = max_delay.next_power_of_two();
        for buf in &mut self.delay {
            buf.clear();
            buf.resize(len, 0.0);
        }
        self.delay_mask = len - 1;
        self.write_pos = 0;
        self.delay_current = self.delay_target;
        self.delay_glide = (1.0 - (-1.0 / (LOOKAHEAD_GLIDE_SEC * self.sample_rate)).exp()) as f32;
        self.env = [0.0; 2];
        self.c_attack = one_percent_coefficient(ATTACK_TIME_SEC, self.sample_rate);
        self.c_release = one_percent_coefficient(RELEASE_TIME_SEC, self.sample_rate);
        for f in &mut self.sc_hpf {
            f.clear();
        }
        self.sc_hpf_hz = 0.0;
        for os in &mut self.oversampler {
            os.reset();
        }
    }

    #[must_use]
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Lookahead in whole samples for a given setting.
    #[must_use]
    pub fn lookahead_samples(&self, lookahead_ms: f32) -> f32 {
        (f64::from(lookahead_ms.clamp(0.0, MAX_LOOKAHEAD_MS)) * 0.001 * self.sample_rate).round()
            as f32
    }

    /// Total latency to report to the host, in samples.
    #[must_use]
    pub fn latency(&self) -> u32 {
        self.delay_target as u32 + if self.hq { OVERSAMPLER_LATENCY } else { 0 }
    }

    /// Apply block-rate settings. Call before each block (cheap when unchanged).
    pub fn set_block(&mut self, s: &BlockSettings) {
        let target = self.lookahead_samples(s.lookahead_ms);
        if target != self.delay_target {
            self.delay_target = target;
        }
        self.link = s.link.clamp(0.0, 1.0);
        let hz = s.sc_hpf_hz.clamp(0.0, (self.sample_rate * 0.45) as f32);
        if hz != self.sc_hpf_hz {
            let was_off = self.sc_hpf_hz <= SC_HPF_OFF_HZ;
            self.sc_hpf_hz = hz;
            if hz > SC_HPF_OFF_HZ {
                for f in &mut self.sc_hpf {
                    f.set(hz, self.sample_rate);
                    if was_off {
                        f.clear();
                    }
                }
            }
        }
        if s.hq != self.hq {
            self.hq = s.hq;
            for os in &mut self.oversampler {
                os.reset();
            }
        }
    }

    /// Snap the lookahead glide to its target (e.g. right after a reset).
    pub fn settle(&mut self) {
        self.delay_current = self.delay_target;
    }

    #[inline]
    fn read_delay(&self, ch: usize) -> f32 {
        let buf = &self.delay[ch];
        let m = self.delay_mask;
        let d = self.delay_current;
        let di = d as usize;
        let frac = d - di as f32;
        let idx = self.write_pos.wrapping_sub(di) & m;
        if frac == 0.0 {
            return buf[idx];
        }
        // Read position lies between x[idx - 1] (older) and x[idx] (newer).
        let x0 = buf[idx];
        let xm1 = buf[idx.wrapping_sub(1) & m];
        if di == 0 {
            return x0 + frac * (xm1 - x0);
        }
        // 4-point, 3rd-order Hermite between x0 and xm1.
        let x1 = buf[(idx + 1) & m];
        let xm2 = buf[idx.wrapping_sub(2) & m];
        let c1 = 0.5 * (xm1 - x1);
        let c2 = x1 - 2.5 * x0 + 2.0 * xm1 - 0.5 * xm2;
        let c3 = 0.5 * (xm2 - x1) + 1.5 * (x0 - xm1);
        ((c3 * frac + c2) * frac + c1) * frac + x0
    }

    /// Process one stereo frame.
    #[inline]
    pub fn process_frame(&mut self, in_l: f32, in_r: f32, c: &Controls) -> Frame {
        // --- Sidechain detector on the undelayed input ---
        let (sl, sr) = if self.sc_hpf_hz > SC_HPF_OFF_HZ {
            (self.sc_hpf[0].process(in_l), self.sc_hpf[1].process(in_r))
        } else {
            (in_l, in_r)
        };
        let (al, ar) = (sl.abs(), sr.abs());
        let peak = al.max(ar);
        let det = [al + self.link * (peak - al), ar + self.link * (peak - ar)];

        let mut gain = [1.0f32; 2];
        for ch in 0..2 {
            // Original follower: c = in >= out ? attack : release;
            // out = c·(out − in) + in − 1e-8, with in = |x| + 1e-8.
            let norm = det[ch] + 1e-8;
            let coef = if norm >= self.env[ch] {
                self.c_attack
            } else {
                self.c_release
            };
            self.env[ch] = (coef * (self.env[ch] - norm) + norm - 1e-8).max(0.0);
            gain[ch] = wet_gain(self.env[ch], self.inv_threshold);
        }

        // --- Lookahead delay (audio path only) ---
        self.write_pos = (self.write_pos + 1) & self.delay_mask;
        self.delay[0][self.write_pos] = in_l;
        self.delay[1][self.write_pos] = in_r;
        if self.delay_current != self.delay_target {
            self.delay_current += (self.delay_target - self.delay_current) * self.delay_glide;
            if (self.delay_current - self.delay_target).abs() < 1e-3 {
                self.delay_current = self.delay_target;
            }
        }
        let dry = [self.read_delay(0), self.read_delay(1)];

        // --- Parallel mix + output gain: dry·(1 − a) + dry·g·a ---
        let a = c.amount.clamp(0.0, 1.0);
        let mixed = [
            dry[0] * (1.0 + a * (gain[0] - 1.0)) * c.out_gain,
            dry[1] * (1.0 + a * (gain[1] - 1.0)) * c.out_gain,
        ];

        // --- Soft saturator ---
        let clip = c.color.clamp(0.0, 1.0);
        let (left, right) = if self.hq {
            (
                self.oversampler[0].process(mixed[0], |x| soft_clip(x, clip)),
                self.oversampler[1].process(mixed[1], |x| soft_clip(x, clip)),
            )
        } else {
            (soft_clip(mixed[0], clip), soft_clip(mixed[1], clip))
        };

        Frame {
            left,
            right,
            gain,
            env: self.env,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL: Controls = Controls {
        amount: 0.25,
        out_gain: 1.0,
        color: 1.0,
    };

    fn dsp_with(sr: f64, s: BlockSettings) -> MagnitudeDsp {
        let mut dsp = MagnitudeDsp::new(sr);
        dsp.set_block(&s);
        dsp.settle();
        dsp
    }

    #[test]
    fn coefficients_match_schematic_cache() {
        // Values cached inside the original schematic's co_eff_calc modules (44.1 kHz).
        assert!((one_percent_coefficient(ATTACK_TIME_SEC, 44_100.0) - 0.593_256_65).abs() < 1e-6);
        assert!((one_percent_coefficient(RELEASE_TIME_SEC, 44_100.0) - 0.999_582_4).abs() < 1e-6);
    }

    #[test]
    fn silence_stays_silent() {
        let mut dsp = dsp_with(44_100.0, BlockSettings::default());
        for _ in 0..1000 {
            let f = dsp.process_frame(0.0, 0.0, &ORIGINAL);
            assert!(f.left.abs() < 1e-9 && f.right.abs() < 1e-9);
            assert_eq!(f.gain, [1.0, 1.0]);
        }
    }

    #[test]
    fn lookahead_delays_audio_by_whole_samples() {
        let s = BlockSettings {
            lookahead_ms: 10.0,
            ..BlockSettings::default()
        };
        let mut dsp = dsp_with(48_000.0, s);
        let dry = Controls {
            amount: 0.0,
            ..ORIGINAL
        };
        let mut out = Vec::new();
        for i in 0..1000 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            out.push(dsp.process_frame(x, x, &dry).left);
        }
        assert_eq!(dsp.latency(), 480);
        for (i, &y) in out.iter().enumerate() {
            if i == 480 {
                assert!((y - 0.5).abs() < 1e-6, "impulse = {y}");
            } else {
                assert_eq!(y, 0.0, "sample {i}");
            }
        }
    }

    #[test]
    fn lookahead_change_glides_without_clicks() {
        let mut dsp = dsp_with(48_000.0, BlockSettings::default());
        let dry = Controls {
            amount: 0.0,
            out_gain: 1.0,
            color: 0.0,
        };
        let sine =
            |i: usize| (i as f32 * 2.0 * std::f32::consts::PI * 100.0 / 48_000.0).sin() * 0.5;
        let mut prev = 0.0;
        let mut max_step = 0.0f32;
        for i in 0..48_000 {
            if i == 10_000 {
                dsp.set_block(&BlockSettings {
                    lookahead_ms: 0.0,
                    ..BlockSettings::default()
                });
            }
            let y = dsp.process_frame(sine(i), sine(i), &dry).left;
            if i > 5000 {
                max_step = max_step.max((y - prev).abs());
            }
            prev = y;
        }
        // A 100 Hz sine at 0.5 moves at most ~0.0066 per sample; a hard jump
        // in delay time would produce a step close to 1.0.
        assert!(max_step < 0.05, "max step {max_step}");
    }

    #[test]
    fn soft_saturator_bounds_output() {
        let mut dsp = dsp_with(
            44_100.0,
            BlockSettings {
                lookahead_ms: 0.0,
                ..BlockSettings::default()
            },
        );
        let dry = Controls {
            amount: 0.0,
            ..ORIGINAL
        };
        let f = dsp.process_frame(100.0, -100.0, &dry);
        assert!(f.left > 0.0 && f.left < 1.0);
        assert!(f.right < 0.0 && f.right > -1.0);
    }

    #[test]
    fn transfer_curve_matches_original() {
        let mut dsp = dsp_with(
            44_100.0,
            BlockSettings {
                lookahead_ms: 0.0,
                ..BlockSettings::default()
            },
        );
        for _ in 0..20_000 {
            dsp.process_frame(0.001, 0.001, &ORIGINAL);
        }
        let quiet = dsp.process_frame(0.001, 0.001, &ORIGINAL);
        assert_eq!(quiet.gain, [1.0, 1.0], "-60 dBFS is below threshold");

        for _ in 0..20_000 {
            dsp.process_frame(1.0, 1.0, &ORIGINAL);
        }
        let loud = dsp.process_frame(1.0, 1.0, &ORIGINAL);
        let gr_db = -20.0 * loud.gain[0].log10();
        // 50 dB over threshold * (1 - 0.5994) = 20.03 dB
        assert!((gr_db - 20.03).abs() < 0.05, "GR = {gr_db}");
    }

    #[test]
    fn release_reaches_one_percent_in_250ms() {
        // The defining property of the original co_eff_calc: after the input stops,
        // the envelope falls to 1 % of its level in RELEASE_TIME_SEC.
        let sr = 48_000.0;
        let mut dsp = dsp_with(
            sr,
            BlockSettings {
                lookahead_ms: 0.0,
                ..BlockSettings::default()
            },
        );
        for _ in 0..4800 {
            dsp.process_frame(0.5, 0.5, &ORIGINAL);
        }
        let start = dsp.process_frame(0.0, 0.0, &ORIGINAL).env[0];
        for _ in 1..(RELEASE_TIME_SEC as f64 * sr) as usize {
            dsp.process_frame(0.0, 0.0, &ORIGINAL);
        }
        let end = dsp.process_frame(0.0, 0.0, &ORIGINAL).env[0];
        let ratio = end / start;
        assert!(
            (ratio - 0.01).abs() < 0.001,
            "ratio after release time = {ratio}"
        );
    }

    #[test]
    fn channels_are_independent_unless_linked() {
        let quiet_left = |link: f32| {
            let mut dsp = dsp_with(
                48_000.0,
                BlockSettings {
                    lookahead_ms: 0.0,
                    link,
                    ..BlockSettings::default()
                },
            );
            let mut f = dsp.process_frame(0.0, 0.0, &ORIGINAL);
            for _ in 0..4800 {
                f = dsp.process_frame(0.001, 0.9, &ORIGINAL);
            }
            f.gain[0]
        };
        assert_eq!(quiet_left(0.0), 1.0, "unlinked: quiet channel untouched");
        assert!(quiet_left(1.0) < 0.2, "linked: loud channel drives both");
    }

    #[test]
    fn sidechain_hpf_ignores_sub_bass() {
        let gr_for = |hpf: f32| {
            let mut dsp = dsp_with(
                48_000.0,
                BlockSettings {
                    lookahead_ms: 0.0,
                    sc_hpf_hz: hpf,
                    ..BlockSettings::default()
                },
            );
            let mut f = dsp.process_frame(0.0, 0.0, &ORIGINAL);
            for i in 0..48_000 {
                let x = (i as f32 * 2.0 * std::f32::consts::PI * 30.0 / 48_000.0).sin() * 0.1;
                f = dsp.process_frame(x, x, &ORIGINAL);
            }
            -20.0 * f.gain[0].log10()
        };
        assert!(gr_for(20.0) > 6.0);
        assert!(gr_for(300.0) < gr_for(20.0) - 6.0);
    }

    #[test]
    fn hq_reports_extra_latency_and_stays_bounded() {
        let mut dsp = dsp_with(
            48_000.0,
            BlockSettings {
                lookahead_ms: 0.0,
                hq: true,
                ..BlockSettings::default()
            },
        );
        assert_eq!(dsp.latency(), OVERSAMPLER_LATENCY);
        let dry = Controls {
            amount: 0.0,
            ..ORIGINAL
        };
        for i in 0..4800 {
            let x = (i as f32 * 0.37).sin() * 4.0;
            let f = dsp.process_frame(x, x, &dry);
            assert!(f.left.is_finite() && f.left.abs() < 1.05);
        }
    }

    #[test]
    fn static_curve_lifts_quiet_material_relative_to_loud() {
        let lift_quiet = static_curve_db(-60.0, 0.5) - -60.0;
        let lift_loud = static_curve_db(-6.0, 0.5) - -6.0;
        assert!(lift_quiet.abs() < 1e-4);
        assert!(lift_loud < -4.0);
    }
}
