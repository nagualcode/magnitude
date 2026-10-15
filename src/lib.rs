//! # Gersic Pressure - Upward Mastering Compressor
//!
//! Faithful recreation of Tom & Jeremy Gersic's iconic 2006 VST plugin,
//! reverse engineered from the original binary (`Gersic_Pressure.dll` / `schematic.osm`)
//! and brought into the modern era with the [Truce](https://github.com/truce-audio/truce)
//! audio plugin framework and Rupert Neve-caliber sound engineering.
//!
//! ## What is Upward Compression?
//! Unlike traditional downward compressors that squash peaks exceeding a threshold,
//! an upward compressor raises the energy of quiet program material below the threshold
//! while leaving loud transients dynamic and punchy.
//!
//! In Gersic Pressure, this is achieved by:
//! 1. Undelayed sidechain peak detection with asymmetric attack (0.2 ms) and release (250 ms) ballistics.
//! 2. Ring-buffered audio path delay (up to 100 ms lookahead), enabling zero-overshoot transient tracking.
//! 3. Logarithmic transfer function with fixed ratio (0.5994) above a -50 dB threshold.
//! 4. Parallel crossfade mix between dry audio and gain-reduced path.
//! 5. Output makeup gain (-50 dB to +50 dB).
//! 6. Rupert Neve-grade rational soft-saturator ($x / (1 + |x|)$) to transparently tame peaks.

pub mod dsp;

use std::sync::Arc;
use truce::prelude::*;
use truce_gui::IntoLayoutEditor;
use truce_gui_types::layout::{GridLayout, knob, meter, widgets};

pub use dsp::GersicPressureDsp;
use GersicPressureParamsParamId as P;

// --- Parameters ---

#[derive(Params)]
pub struct GersicPressureParams {
    /// LookAhead delay time in milliseconds (0.0 to 100.0 ms).
    /// Delays audio path relative to the sidechain detector.
    #[param(
        name = "LookAhead",
        short_name = "Ahead",
        range = "linear(0.0, 100.0)",
        default = 50.0,
        unit = "ms",
        smooth = "exp(10)"
    )]
    pub lookahead: FloatParam,

    /// Compression Amount / Parallel Wet Mix percentage (0.0% to 100.0%).
    /// Default 25.0% (corresponds to 12.5 in original 0..50 scale).
    #[param(
        name = "Amount",
        short_name = "Amount",
        range = "linear(0.0, 100.0)",
        default = 25.0,
        unit = "%",
        smooth = "exp(10)"
    )]
    pub amount: FloatParam,

    /// Output makeup gain in dB (-50.0 dB to +50.0 dB).
    #[param(
        name = "Out Gain",
        short_name = "Gain",
        range = "linear(-50.0, 50.0)",
        default = 0.0,
        unit = "dB",
        smooth = "exp(10)"
    )]
    pub gain: FloatParam,

    /// Left output peak meter.
    #[meter]
    pub meter_left: MeterSlot,

    /// Right output peak meter.
    #[meter]
    pub meter_right: MeterSlot,

    /// Gain reduction meter in dB.
    #[meter]
    pub meter_gr: MeterSlot,
}

// --- Plugin Implementation ---

pub struct GersicPressure;

impl PluginLogic for GersicPressure {
    type Params = GersicPressureParams;
    type DspState = GersicPressureDsp;

    fn reset(state: &mut GersicPressureDsp, _params: &GersicPressureParams, config: &AudioConfig) {
        state.reset(config.sample_rate);
    }

    fn process(
        state: &mut GersicPressureDsp,
        params: &GersicPressureParams,
        buffer: &mut AudioBuffer,
        _events: &EventList,
        context: &mut ProcessContext,
    ) -> ProcessStatus {
        let num_samples = buffer.num_samples();
        if num_samples == 0 {
            return ProcessStatus::Normal;
        }

        // Read smoothed parameters
        let lookahead_ms = params.lookahead.read();
        let amount_norm = params.amount.read() * 0.01; // convert 0..100% to 0..1
        let gain_db = params.gain.read();

        let mut peak_gr_db = 0.0f32;

        // Idiomatic zero-allocation frame processing:
        buffer.for_each_stereo_frame(|inp, out| {
            let (y_l, y_r, gr) = state.process_sample(inp[0], inp[1], lookahead_ms, amount_norm, gain_db);
            out[0] = y_l;
            out[1] = y_r;
            if gr > peak_gr_db {
                peak_gr_db = gr;
            }
        });

        // Update real-time meters
        if buffer.num_output_channels() >= 1 {
            context.set_meter(P::MeterLeft, buffer.output_peak(0));
        }
        if buffer.num_output_channels() >= 2 {
            context.set_meter(P::MeterRight, buffer.output_peak(1));
        }
        // Normalize gain reduction to [0, 1] range for GUI display (0 to 24 dB reduction range)
        let gr_meter_val = (peak_gr_db / 24.0).clamp(0.0, 1.0);
        context.set_meter(P::MeterGr, gr_meter_val);

        ProcessStatus::Normal
    }

    fn editor(params: Arc<GersicPressureParams>) -> Box<dyn Editor> {
        // Rupert Neve-grade layout:
        // Top row: LookAhead, Amount, Out Gain knobs
        // Right section: Level Meters + Gain Reduction (GR) Meter
        GridLayout::build(vec![widgets(vec![
            knob(P::Lookahead, "LookAhead").at(0, 0),
            knob(P::Amount, "Amount").at(1, 0),
            knob(P::Gain, "Out Gain").at(2, 0),
            meter(&[P::MeterLeft, P::MeterRight], "Output")
                .at(3, 0)
                .rows(2),
            meter(&[P::MeterGr], "GR")
                .at(4, 0)
                .rows(2),
        ])])
        .with_title("PRESSURE")
        .with_subtitle("Upward Mastering Compressor")
        .with_cols(5)
        .resizable(true)
        .min_size((5, 2))
        .max_size((8, 4))
        .into_editor(&params)
    }
}

truce::plugin! {
    logic: GersicPressure,
    params: GersicPressureParams,
}

truce::enable_rt_paranoid!();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_is_valid() {
        truce_test::assert_valid_info::<Plugin>();
    }

    #[test]
    fn test_initialization_and_params() {
        let params = GersicPressureParams::new();
        assert_eq!(params.lookahead.read(), 50.0);
        assert_eq!(params.amount.read(), 25.0);
        assert_eq!(params.gain.read(), 0.0);
    }

    #[test]
    fn process_is_realtime_clean() {
        use std::time::Duration;
        use truce_test::{InputSource, assert_realtime_clean, driver};
        assert_realtime_clean(|| {
            driver!(Plugin)
                .duration(Duration::from_millis(40))
                .input(InputSource::Constant(0.25))
                .script(|s| {
                    s.set_param(P::Amount, 50.0);
                    s.wait_ms(10);
                    s.set_param(P::Gain, 6.0);
                    s.wait_ms(10);
                    s.set_param(P::Lookahead, 20.0);
                    s.wait_ms(10);
                })
                .run()
        });
    }
}
