//! # Gersic Pressure - Upward Mastering Compressor
//!
//! Recreation of Tom & Jeremy Gersic's 2006 PRESSURE VST plugin, reverse engineered
//! from the original binary (`Gersic_Pressure.dll` / `schematic.osm`) and rebuilt on
//! the [Truce](https://github.com/truce-audio/truce) plugin framework.
//!
//! Upward compression is realised the way the original does it: a parallel blend of
//! the (lookahead-delayed) dry signal with a copy that is compressed above -50 dBFS.
//! Loud passages are pulled down in the wet path while quiet ones pass at unity, so
//! after the blend the quiet material sits relatively higher.
//!
//! The three original controls (LookAhead, Amount, Out Gain) behave as in 2006. The
//! v2 additions (Color, Link, SC HPF, HQ) default to the original sound.

pub mod dsp;
mod editor;
pub mod oversampling;

use std::sync::Arc;
use truce::prelude::*;

use GersicPressureParamsParamId as P;
pub use dsp::GersicPressureDsp;
use dsp::{BlockSettings, Controls};

/// Sub-block length for per-sample parameter smoothing.
const CHUNK: usize = 64;

// --- Parameters ---

#[derive(Params)]
pub struct GersicPressureParams {
    /// Audio-path delay relative to the detector, in milliseconds.
    /// Changes glide click-free; the host is told the new latency.
    #[param(
        name = "LookAhead",
        short_name = "Ahead",
        range = "linear(0.0, 100.0)",
        default = 50.0,
        unit = "ms"
    )]
    pub lookahead: FloatParam,

    /// Parallel blend of the compressed path (original 0..50 scale shown as 0..100 %).
    #[param(
        name = "Amount",
        short_name = "Amount",
        range = "linear(0.0, 100.0)",
        default = 25.0,
        unit = "%",
        format = "format_percent",
        smooth = "exp(10)"
    )]
    pub amount: FloatParam,

    /// Output gain in dB, applied before the soft saturator.
    #[param(
        name = "Out Gain",
        short_name = "Gain",
        range = "linear(-50.0, 50.0)",
        default = 0.0,
        unit = "dB",
        smooth = "exp(10)"
    )]
    pub gain: FloatParam,

    /// Soft saturator drive: 100 % is the original `x / (1 + |x|)`, 0 % is clean.
    #[param(
        name = "Color",
        short_name = "Color",
        range = "linear(0.0, 100.0)",
        default = 100.0,
        unit = "%",
        format = "format_percent",
        smooth = "exp(10)"
    )]
    pub color: FloatParam,

    /// Stereo detector link: 0 % detects each channel on its own (as the original).
    #[param(
        name = "Link",
        short_name = "Link",
        range = "linear(0.0, 100.0)",
        default = 0.0,
        unit = "%",
        format = "format_percent"
    )]
    pub link: FloatParam,

    /// Detector high-pass; the lowest setting switches it off.
    #[param(
        name = "SC HPF",
        short_name = "HPF",
        range = "log(20.0, 500.0)",
        default = 20.0,
        unit = "Hz"
    )]
    pub sc_hpf: FloatParam,

    /// Run the saturator 4x oversampled (adds 71 samples of compensated latency).
    #[param(name = "HQ", default = true)]
    pub hq: BoolParam,

    /// Left output peak (linear).
    #[meter]
    pub meter_left: MeterSlot,

    /// Right output peak (linear).
    #[meter]
    pub meter_right: MeterSlot,

    /// Lowest wet-path gain of the block, left (linear, <= 1).
    #[meter]
    pub meter_gr: MeterSlot,

    /// Lowest wet-path gain of the block, right (linear, <= 1).
    #[meter]
    pub meter_gr_right: MeterSlot,

    /// Highest detector envelope of the block (linear).
    #[meter]
    pub meter_env: MeterSlot,
}

impl GersicPressureParams {
    /// Ranges are authored as 0..100, so print the plain value (the stock `%`
    /// formatter expects 0..1 and would show "2500%").
    fn format_percent(&self, value: f64) -> String {
        format!("{value:.1} %")
    }

    fn block_settings(&self) -> BlockSettings {
        BlockSettings {
            lookahead_ms: self.lookahead.value(),
            link: self.link.value() * 0.01,
            sc_hpf_hz: self.sc_hpf.value(),
            hq: self.hq.value(),
        }
    }
}

// --- Plugin Implementation ---

pub struct GersicPressure;

impl PluginLogic for GersicPressure {
    type Params = GersicPressureParams;
    type DspState = GersicPressureDsp;

    fn init(params: &GersicPressureParams, _cx: &InitContext) -> GersicPressureDsp {
        let mut dsp = GersicPressureDsp::new(44_100.0);
        dsp.set_block(&params.block_settings());
        dsp.settle();
        dsp
    }

    fn reset(state: &mut GersicPressureDsp, params: &GersicPressureParams, config: &AudioConfig) {
        state.reset(config.sample_rate);
        state.set_block(&params.block_settings());
        state.settle();
    }

    fn latency(state: &GersicPressureDsp) -> u32 {
        state.latency()
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

        state.set_block(&params.block_settings());

        let mut amount = [0.0f32; CHUNK];
        let mut gain_db = [0.0f32; CHUNK];
        let mut color = [0.0f32; CHUNK];
        let mut min_gain = [1.0f32; 2];
        let mut max_env = 0.0f32;
        let mut cached_db = f32::NAN;
        let mut cached_lin = 1.0f32;

        let mut start = 0;
        while start < num_samples {
            let len = CHUNK.min(num_samples - start);
            params.amount.read_into(&mut amount[..len]);
            params.gain.read_into(&mut gain_db[..len]);
            params.color.read_into(&mut color[..len]);

            let mut i = 0;
            buffer.slice(start, len).for_each_stereo_frame(|inp, out| {
                // dB -> linear only when the smoothed value actually moves.
                if gain_db[i] != cached_db {
                    cached_db = gain_db[i];
                    cached_lin = db_to_linear(cached_db);
                }
                let controls = Controls {
                    amount: amount[i] * 0.01,
                    out_gain: cached_lin,
                    color: color[i] * 0.01,
                };
                let f = state.process_frame(inp[0], inp[1], &controls);
                out[0] = f.left;
                out[1] = f.right;
                min_gain[0] = min_gain[0].min(f.gain[0]);
                min_gain[1] = min_gain[1].min(f.gain[1]);
                max_env = max_env.max(f.env[0].max(f.env[1]));
                i += 1;
            });
            start += len;
        }

        let outs = buffer.num_output_channels();
        if outs >= 1 {
            context.set_meter(P::MeterLeft, buffer.output_peak(0));
        }
        context.set_meter(
            P::MeterRight,
            buffer.output_peak(if outs >= 2 { 1 } else { 0 }),
        );
        context.set_meter(P::MeterGr, min_gain[0]);
        context.set_meter(
            P::MeterGrRight,
            if outs >= 2 { min_gain[1] } else { min_gain[0] },
        );
        context.set_meter(P::MeterEnv, max_env);

        ProcessStatus::Normal
    }

    fn editor(params: Arc<GersicPressureParams>) -> Box<dyn Editor> {
        editor::create(params)
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
    fn defaults_reproduce_the_original() {
        let params = GersicPressureParams::new();
        assert_eq!(params.lookahead.value(), 50.0);
        assert_eq!(params.amount.value(), 25.0);
        assert_eq!(params.gain.value(), 0.0);
        assert_eq!(params.color.value(), 100.0);
        assert_eq!(params.link.value(), 0.0);
        assert!(params.sc_hpf.value() <= dsp::SC_HPF_OFF_HZ);
    }

    #[test]
    fn has_editor() {
        truce_test::assert_has_editor::<Plugin>();
    }

    #[test]
    fn state_round_trips() {
        truce_test::assert_state_round_trip::<Plugin>();
    }

    #[test]
    fn process_is_realtime_clean() {
        use std::time::Duration;
        use truce_test::{InputSource, assert_realtime_clean, driver};
        assert_realtime_clean(|| {
            driver!(Plugin)
                .duration(Duration::from_millis(60))
                .input(InputSource::Constant(0.25))
                .script(|s| {
                    s.set_param(P::Amount, 50.0);
                    s.wait_ms(10);
                    s.set_param(P::Gain, 6.0);
                    s.wait_ms(10);
                    s.set_param(P::Lookahead, 20.0);
                    s.wait_ms(10);
                    s.set_param(P::ScHpf, 150.0);
                    s.wait_ms(10);
                    s.set_param(P::Hq, 0.0);
                    s.wait_ms(10);
                })
                .run()
        });
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn gui_screenshot_macos() {
        truce_test::screenshot!(Plugin, "screenshots/gersic_pressure_default_macos.png").run();
    }
}
