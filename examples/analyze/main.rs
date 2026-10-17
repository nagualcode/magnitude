//! Objective comparison of the v1 DSP (verbatim copy in `v1_dsp.rs`) and the v2 DSP.
//!
//! `cargo run --release --example analyze`

#[allow(dead_code)]
mod v1_dsp;

use std::f64::consts::PI;
use std::time::Instant;

use gersic_pressure::dsp::{BlockSettings, Controls, GersicPressureDsp};

const SR: f64 = 44_100.0;

fn v2(hq: bool) -> GersicPressureDsp {
    let mut d = GersicPressureDsp::new(SR);
    d.set_block(&BlockSettings {
        lookahead_ms: 0.0,
        hq,
        ..BlockSettings::default()
    });
    d.settle();
    d
}

/// Time for gain reduction to recover from 20 dB to below 3 dB after a 0 dBFS burst.
fn release_ms(mut process: impl FnMut(f32) -> f32) -> f64 {
    for _ in 0..(SR as usize / 2) {
        process(1.0);
    }
    let mut n = 0usize;
    while process(0.0) > 3.0 {
        n += 1;
    }
    n as f64 / SR * 1000.0
}

/// Power of everything that is neither the fundamental's harmonics nor DC,
/// relative to the fundamental (dB). Uses a bin-centred sine so no window is needed.
fn alias_db(output: &[f32], k: usize) -> f64 {
    let n = output.len();
    let power = |bin: usize| {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &x) in output.iter().enumerate() {
            let ph = 2.0 * PI * (bin * i) as f64 / n as f64;
            re += f64::from(x) * ph.cos();
            im -= f64::from(x) * ph.sin();
        }
        re * re + im * im
    };
    let fundamental = power(k);
    let mut alias = 0.0;
    for bin in 1..n / 2 {
        if bin % k != 0 {
            alias += power(bin);
        }
    }
    10.0 * (alias / fundamental).log10()
}

fn sine(k: usize, n: usize, amp: f32) -> impl Fn(usize) -> f32 {
    move |i| amp * (2.0 * PI * (k * i) as f64 / n as f64).sin() as f32
}

fn main() {
    let orig = Controls {
        amount: 0.5,
        out_gain: 1.0,
        color: 1.0,
    };

    // --- Release ballistics ---
    let mut d1 = v1_dsp::GersicPressureDsp::new(SR);
    let r1 = release_ms(|x| d1.process_sample(x, x, 0.0, 0.5, 0.0).2);
    let mut d2 = v2(false);
    let r2 = release_ms(|x| -20.0 * d2.process_frame(x, x, &orig).gain[0].log10());
    println!("GR recovery 20 dB -> 3 dB:  v1 {r1:7.1} ms   v2 {r2:7.1} ms");

    // --- Aliasing of the soft clipper (Amount 0 isolates it), 0 dBFS sine near 7 kHz ---
    let n = 4096;
    let k = 651; // 651 * 44100 / 4096 ≈ 7009 Hz
    let s = sine(k, n, 1.0);
    let warm = 8192;
    let dry = Controls {
        amount: 0.0,
        ..orig
    };
    let mut d1 = v1_dsp::GersicPressureDsp::new(SR);
    let out1: Vec<f32> = (0..warm + n)
        .map(|i| d1.process_sample(s(i), s(i), 0.0, 0.0, 0.0).0)
        .skip(warm)
        .collect();
    let mut d2 = v2(true);
    let out2: Vec<f32> = (0..warm + n)
        .map(|i| d2.process_frame(s(i), s(i), &dry).left)
        .skip(warm)
        .collect();
    println!(
        "Aliasing, 7 kHz @ 0 dBFS:   v1 {:7.1} dB   v2 HQ {:7.1} dB",
        alias_db(&out1, k),
        alias_db(&out2, k)
    );

    // --- CPU: 60 s of stereo noise-ish material ---
    let len = 60 * SR as usize;
    let input: Vec<f32> = (0..len)
        .map(|i| ((i as f32 * 0.618).sin() * (i as f32 * 0.0013).sin()) * 0.7)
        .collect();
    let mut sink = 0.0f32;

    let mut d1 = v1_dsp::GersicPressureDsp::new(SR);
    let t = Instant::now();
    for &x in &input {
        sink += d1.process_sample(x, -x, 50.0, 0.25, 0.0).0;
    }
    let c1 = t.elapsed().as_secs_f64();

    let bench = |hq: bool, sink: &mut f32| {
        let mut d = GersicPressureDsp::new(SR);
        d.set_block(&BlockSettings {
            hq,
            ..BlockSettings::default()
        });
        d.settle();
        let c = Controls {
            amount: 0.25,
            out_gain: 1.0,
            color: 1.0,
        };
        let t = Instant::now();
        for &x in &input {
            *sink += d.process_frame(x, -x, &c).left;
        }
        t.elapsed().as_secs_f64()
    };
    let c2 = bench(false, &mut sink);
    let c3 = bench(true, &mut sink);
    println!(
        "CPU, 60 s stereo @ 44.1k:   v1 {:.1} ms ({:.0}x RT)   v2 {:.1} ms ({:.0}x RT)   v2 HQ {:.1} ms ({:.0}x RT)",
        c1 * 1e3,
        60.0 / c1,
        c2 * 1e3,
        60.0 / c2,
        c3 * 1e3,
        60.0 / c3
    );
    std::hint::black_box(sink);
}
