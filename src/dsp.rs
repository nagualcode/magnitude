//! DSP Core for Gersic Pressure Upward Mastering Compressor.
//!
//! Re-engineered from the original 2006 OutSim SynthMaker schematic by Tom & Jeremy Gersic
//! with Rupert Neve-grade audio engineering precision:
//! - Transient-preserving lookahead delay line with zero audio-thread allocations
//! - Asymmetric dual-ballistics sidechain envelope follower (0.2 ms attack, 250 ms release)
//! - Precise logarithmic upward compression transfer function (threshold: -50 dB, ratio: 0.5994)
//! - Parallel crossfade mix ($1 - x$ uncompressed audio, $x$ compressed path)
//! - Output makeup gain (-50 dB to +50 dB)
//! - Rational analog soft-saturator ($x / (1 + |x|)$) for transparent peak taming

/// Maximum delay buffer capacity in samples (192,000 samples matches the original SynthMaker buffer,
/// supporting up to 1 second of lookahead at 192 kHz or 4 seconds at 48 kHz).
pub const MAX_DELAY_SAMPLES: usize = 192_000;

/// Default compression threshold in dB from original schematic.
pub const DEFAULT_THRESHOLD_DB: f32 = -50.0;

/// Fixed compression ratio from original schematic (approx 0.6 in log space).
pub const FIXED_RATIO: f32 = 0.5994;

/// Attack time constant in seconds (0.2 ms).
pub const ATTACK_TIME_SEC: f32 = 0.0002;

/// Release time constant in seconds (250 ms).
pub const RELEASE_TIME_SEC: f32 = 0.25;

/// DSP state holding pre-allocated buffers and detector ballistics.
pub struct GersicPressureDsp {
    sample_rate: f64,
    /// Delay ring buffer for Left audio channel.
    delay_buffer_l: Vec<f32>,
    /// Delay ring buffer for Right audio channel.
    delay_buffer_r: Vec<f32>,
    /// Write pointer into circular delay ring buffers.
    write_pos: usize,
    /// Sidechain envelope follower level.
    env: f32,
    /// Attack coefficient: $c_{attack} = \exp(-1 / (\tau_{attack} \cdot f_s))$.
    c_attack: f32,
    /// Release coefficient: $c_{release} = \exp(-1 / (\tau_{release} \cdot f_s))$.
    c_release: f32,
}

impl Default for GersicPressureDsp {
    fn default() -> Self {
        let mut dsp = Self {
            sample_rate: 44100.0,
            delay_buffer_l: vec![0.0; MAX_DELAY_SAMPLES],
            delay_buffer_r: vec![0.0; MAX_DELAY_SAMPLES],
            write_pos: 0,
            env: 0.0,
            c_attack: 0.0,
            c_release: 0.0,
        };
        dsp.update_coefficients(44100.0);
        dsp
    }
}

impl GersicPressureDsp {
    /// Create a new DSP instance with pre-allocated memory.
    pub fn new(sample_rate: f64) -> Self {
        let mut dsp = Self::default();
        dsp.reset(sample_rate);
        dsp
    }

    /// Reset DSP state, clear delay buffers and update sample rate coefficients.
    pub fn reset(&mut self, sample_rate: f64) {
        self.sample_rate = if sample_rate > 1000.0 { sample_rate } else { 44100.0 };
        self.delay_buffer_l.fill(0.0);
        self.delay_buffer_r.fill(0.0);
        self.write_pos = 0;
        self.env = 0.0;
        self.update_coefficients(self.sample_rate);
    }

    /// Recalculate envelope ballistics coefficients for the given sample rate.
    fn update_coefficients(&mut self, sr: f64) {
        let fs = sr as f32;
        // c = exp(-1 / (tau * fs))
        self.c_attack = (-1.0 / (ATTACK_TIME_SEC * fs)).exp();
        self.c_release = (-1.0 / (RELEASE_TIME_SEC * fs)).exp();
    }

    /// Process a single stereo sample frame.
    ///
    /// # Arguments
    /// * `in_l` - Left input sample (undelayed)
    /// * `in_r` - Right input sample (undelayed)
    /// * `lookahead_ms` - Lookahead delay in milliseconds (0.0 to 100.0 ms)
    /// * `amount_norm` - Wet/dry mix amount (0.0 = 100% uncompressed dry, 1.0 = 100% compressed wet)
    /// * `gain_db` - Output makeup gain in dB (-50.0 dB to +50.0 dB)
    ///
    /// # Returns
    /// `(out_l, out_r, gain_reduction_db)`
    #[inline]
    pub fn process_sample(
        &mut self,
        in_l: f32,
        in_r: f32,
        lookahead_ms: f32,
        amount_norm: f32,
        gain_db: f32,
    ) -> (f32, f32, f32) {
        // --- 1. Sidechain Peak Detection on Undelayed Audio ---
        // Envelope follower tracks the peak magnitude across both channels with denormal guard.
        let peak_in = in_l.abs().max(in_r.abs());
        let norm_in = peak_in + 1e-8;

        // Asymmetric ballistics: attack when input >= current envelope, release otherwise.
        let c = if norm_in >= self.env {
            self.c_attack
        } else {
            self.c_release
        };

        // Equation extracted from SynthMaker schematic:
        // out = c * (out - normIn) + normIn - 1e-8 = c * env[n-1] + (1 - c) * normIn - 1e-8
        self.env = (c * self.env + (1.0 - c) * norm_in - 1e-8).max(0.0);

        // --- 2. Lookahead Delay Buffer ---
        // Convert lookahead from milliseconds to samples: D = (ms / 1000) * sr
        let delay_samples = ((f64::from(lookahead_ms) * 0.001 * self.sample_rate).round() as usize)
            .min(MAX_DELAY_SAMPLES - 1);

        // Write current undelayed audio into ring buffer
        self.delay_buffer_l[self.write_pos] = in_l;
        self.delay_buffer_r[self.write_pos] = in_r;

        // Read delayed audio
        let read_pos = if self.write_pos >= delay_samples {
            self.write_pos - delay_samples
        } else {
            self.write_pos + MAX_DELAY_SAMPLES - delay_samples
        };

        let dry_l = self.delay_buffer_l[read_pos];
        let dry_r = self.delay_buffer_r[read_pos];

        // Advance write pointer
        self.write_pos = (self.write_pos + 1) % MAX_DELAY_SAMPLES;

        // --- 3. Logarithmic Compression Transfer Function ---
        // Convert envelope level to dB space: dB = 20 * log10(max(env, 1e-10))
        let env_db = 20.0 * (self.env.max(1e-10)).log10();

        // Calculate overshoot above the -50 dB threshold
        let over = (env_db - DEFAULT_THRESHOLD_DB).max(0.0);

        // Gain reduction in dB: -over * (1 - ratio)
        // With ratio = 0.5994, reduction = -over * 0.4006 dB
        let gr_db = -over * (1.0 - FIXED_RATIO);

        // Convert reduction dB to linear multiplier: G_lin = 10^(gr_db / 20)
        let g_lin = 10.0f32.powf(gr_db / 20.0);

        // Compressed path (wet)
        let wet_l = dry_l * g_lin;
        let wet_r = dry_r * g_lin;

        // --- 4. Parallel Upward Mix ---
        // In upward compression, quiet portions below -50 dB experience 0 dB reduction (wet == dry).
        // Loud portions above -50 dB are reduced in the wet path.
        // Mixing dry * (1 - amount) + wet * amount preserves the sharp attack transients of the dry
        // signal while compressing dynamic peaks.
        let amount = amount_norm.clamp(0.0, 1.0);
        let mixed_l = dry_l * (1.0 - amount) + wet_l * amount;
        let mixed_r = dry_r * (1.0 - amount) + wet_r * amount;

        // --- 5. Output Makeup Gain ---
        // G_out = 10^(gain_db / 20)
        let gain_mult = 10.0f32.powf(gain_db / 20.0);
        let gained_l = mixed_l * gain_mult;
        let gained_r = mixed_r * gain_mult;

        // --- 6. Rupert Neve-grade Rational Soft Saturation ---
        // Equation from original schematic: out = x / (1 + |x|)
        // Transparent at low levels, smoothly limiting peaks approaching 0 dBFS without digital clipping.
        let out_l = gained_l / (1.0 + gained_l.abs());
        let out_r = gained_r / (1.0 + gained_r.abs());

        (out_l, out_r, gr_db.abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_silence_handling() {
        let mut dsp = GersicPressureDsp::new(44100.0);
        for _ in 0..1000 {
            let (l, r, gr) = dsp.process_sample(0.0, 0.0, 50.0, 0.25, 0.0);
            assert!(l.abs() < 1e-6);
            assert!(r.abs() < 1e-6);
            assert!(gr < 1e-4);
        }
    }

    #[test]
    fn test_lookahead_delay_timing() {
        let mut dsp = GersicPressureDsp::new(48000.0);
        let delay_ms = 10.0; // 10 ms at 48 kHz = 480 samples
        let expected_samples = 480;

        // Send an impulse at t=0
        let (out_l0, _, _) = dsp.process_sample(1.0, 1.0, delay_ms, 0.0, 0.0);
        assert_eq!(out_l0, 0.0, "Impulse should be delayed");

        // Step forward expected_samples - 1 samples
        for _ in 1..expected_samples {
            let (out, _, _) = dsp.process_sample(0.0, 0.0, delay_ms, 0.0, 0.0);
            assert_eq!(out, 0.0);
        }

        // At exactly expected_samples, delayed impulse should emerge
        let (delayed_impulse, _, _) = dsp.process_sample(0.0, 0.0, delay_ms, 0.0, 0.0);
        // Note: soft clipper output = 1.0 / (1.0 + 1.0) = 0.5
        assert!((delayed_impulse - 0.5).abs() < 1e-4, "Delayed impulse did not emerge on time: {}", delayed_impulse);
    }

    #[test]
    fn test_soft_saturator_limits_amplitude() {
        let mut dsp = GersicPressureDsp::new(44100.0);
        // Feed extremely high amplitude (+30 dB)
        let (out_l, out_r, _) = dsp.process_sample(100.0, -100.0, 0.0, 0.0, 0.0);
        assert!(out_l > 0.0 && out_l < 1.0, "Output exceeded 1.0: {}", out_l);
        assert!(out_r < 0.0 && out_r > -1.0, "Output exceeded -1.0: {}", out_r);
    }

    #[test]
    fn test_upward_compression_transfer_curve() {
        let mut dsp = GersicPressureDsp::new(44100.0);

        // Quiet signal: -60 dBFS (amplitude = 0.001), below -50 dB threshold
        for _ in 0..5000 {
            dsp.process_sample(0.001, 0.001, 0.0, 0.5, 0.0);
        }
        let (_, _, gr_quiet) = dsp.process_sample(0.001, 0.001, 0.0, 0.5, 0.0);
        assert!(gr_quiet < 0.01, "Quiet signal below threshold should have 0 dB reduction");

        // Loud signal: 0 dBFS (amplitude = 1.0), 50 dB above threshold
        for _ in 0..5000 {
            dsp.process_sample(1.0, 1.0, 0.0, 0.5, 0.0);
        }
        let (_, _, gr_loud) = dsp.process_sample(1.0, 1.0, 0.0, 0.5, 0.0);
        // Over = 50 dB, GR = 50 * (1 - 0.5994) = 50 * 0.4006 = ~20.03 dB
        assert!((gr_loud - 20.03).abs() < 1.0, "Expected ~20 dB reduction for 0 dBFS input, got: {}", gr_loud);
    }
}
