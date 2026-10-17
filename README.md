# Magnitude

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Framework: Truce](https://img.shields.io/badge/Framework-Truce%20v6.3-purple.svg)](https://github.com/truce-audio/truce)
[![Rust: 2024](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](https://www.rust-lang.org/)
[![Formats: CLAP | VST3 | AU](https://img.shields.io/badge/Formats-CLAP%20%7C%20VST3%20%7C%20AU-brightgreen.svg)]()
[![GitHub Release](https://img.shields.io/github/v/release/nagualcode/magnitude?color=blue)](https://github.com/nagualcode/magnitude/releases)

> *"Pressure Mastering Compressor uses a technique called upward compression; it increases the level of quiet portions in a mix without reducing the level of attack transients like a traditional compressor."*
> — **Jeremy Gersic, 2006**

**Magnitude** is an upward mastering compressor for macOS (CLAP, VST3, AU). It is descended from the 2006 freeware VST **PRESSURE Upward Compressor** by Tom & Jeremy Gersic: the DSP was reverse engineered from the original SynthMaker schematic and rebuilt in Rust on the [Truce](https://github.com/truce-audio/truce) plugin framework. It reproduces the original sound by default, with fidelity fixes (see [What changed in v2](#what-changed-in-v2)) and optional modern features.

Until v2.0.1 the project was called **Gersic Pressure: Born Again**. See [Upgrading from Gersic Pressure](#upgrading-from-gersic-pressure). The repository was renamed from `gersicpressurebornagain` to `magnitude`; old links redirect.

![Magnitude editor](screenshots/magnitude_default_macos.png)

Published under the vendor **Jardim Sonoro** in your DAW.

---

## Download (macOS, Apple Silicon)

Get the latest build from [Releases](https://github.com/nagualcode/magnitude/releases):

- `magnitude-<ver>-macos-arm64.pkg`: installer (CLAP, VST3, AU v2 + factory presets, user scope, no admin prompt).
- `magnitude-<ver>-macos-arm64-bundles.zip`: the raw `.clap`, `.vst3` and `.component` bundles.

The binaries are **arm64 only** (Apple Silicon). Intel Macs need to build from source.

The builds are ad-hoc signed and **not notarized**. If macOS blocks the installer, right-click → *Open*. If you use the zip, clear the quarantine flag before copying the bundles:

```bash
xattr -dr com.apple.quarantine "Magnitude.clap" "Magnitude.vst3" "Magnitude.component"
```

### Upgrading from Gersic Pressure

Magnitude keeps the same plugin identity as Gersic Pressure (CLAP/VST3 ids, the AU code `aufx PR3S Jrdm` and all parameter ids). Projects and presets saved with Gersic Pressure v1/v2 open in Magnitude unchanged, and the DAW simply shows the new name.

Because the identity is shared, **remove the old bundles** so your DAW doesn't find two plugins with the same id, then rescan:

```bash
rm -rf ~/Library/Audio/Plug-Ins/CLAP/"Gersic Pressure.clap" \
       ~/Library/Audio/Plug-Ins/VST3/"Gersic Pressure.vst3" \
       ~/Library/Audio/Plug-Ins/Components/"Gersic Pressure.component" \
       ~/Library/Audio/Presets/"Jardim Sonoro"/"Gersic Pressure"
```

(If you installed system-wide, the same names live under `/Library/Audio/Plug-Ins/`.)

---

## Controls

| Control | Range | Default | Origin | What it does |
| :--- | :--- | :--- | :--- | :--- |
| **LookAhead** | 0–100 ms | 50 ms | 2006 | Delays the audio relative to the detector. Changes glide without clicks, and the latency is reported to the host for delay compensation. |
| **Amount** | 0–100 % | 25 % | 2006 | Blend of the compressed path into the dry path. |
| **Out Gain** | ±50 dB | 0 dB | 2006 | Gain before the soft saturator. |
| **Color** | 0–100 % | 100 % | v2 | Saturator drive. 100 % is the original `x / (1 + |x|)`, 0 % is fully clean. |
| **Link** | 0–100 % | 0 % | v2 | Stereo detector link. 0 % detects each channel on its own, like the original. |
| **SC HPF** | Off, 20–500 Hz | Off | v2 | High-pass on the detector only, so bass stops driving the compression. |
| **HQ** | on/off | on | v2 | Runs the saturator 4× oversampled with linear-phase filters. Adds 71 samples of latency, which the host compensates. |

The editor follows the layout of an Ableton Live device. Drag a dial up or down to change it, hold Shift or Cmd for fine steps, and double-click to reset. The display has two views: **Transfer** shows the live static curve with the detector level as a dot, and **Activity** shows about 4 s of detector level and gain-reduction history. The meters show the gain reduction you actually hear after the blend, per channel, plus the output peak.

---

## The signal chain

This section walks through what happens to your audio inside Magnitude, stage by stage, in the order the samples flow. Every number here comes from the source (`src/dsp.rs`, `src/oversampling.rs`) or from measurements in its tests.

### Overview

```
                 in L/R
                    │
         ┌──────────┴──────────────────┐
         │ DETECTOR (undelayed)        │ AUDIO PATH
         ▼                             ▼
  ② SC HPF (detector only)      ⑥ LookAhead delay 0–100 ms
         ▼                             │ dry
  ③ |x| + stereo Link                  ▼
         ▼                      ⑦ Parallel blend
  ④ Envelope (peak, 0.2/250 ms)   dry · (1 − Amount + Amount · g)
         ▼                             ▲         │
  ⑤ Gain computer ─────── g ───────────┘         ▼
    (−50 dBFS, 1.67 : 1)                  ⑧ Out Gain ±50 dB
                                                 ▼
                                          ⑨ Saturator x / (1 + Color·|x|)
                                             [4× oversampled when HQ]
                                                 ▼
                                              out L/R
```

The design has two branches:

- The **detector** listens to the *undelayed* input and works out, per channel, a gain `g` (at most 1) for the compressed path.
- The **audio path** delays the input by the lookahead, blends the uncompressed (dry) and compressed versions, applies output gain and finally soft-saturates.

The "upward" part is in the blend (stage ⑦). Nothing in Magnitude ever turns quiet material *up*. Instead, loud material is turned *down* in one copy of the signal, and mixing that copy back in narrows the distance between loud and quiet. After makeup gain this is heard as quiet detail coming up. The dry copy is never compressed, which is how attack transients survive.

### ① Input and channel handling

Magnitude processes in 32-bit float, one stereo frame at a time. A mono input feeds both channels of the detector and the audio path, and the output takes the left channel. The two channels have independent detectors, envelopes and gains, so each stage below runs once per channel unless stated otherwise.

### ② Sidechain high-pass filter (SC HPF, v2)

| | |
|---|---|
| Type | 2nd-order Butterworth high-pass (RBJ biquad, Q = 0.707), 64-bit state |
| Range | 20–500 Hz, logarithmic. At the lowest setting the filter is **off** (bypassed) |
| Applies to | The **detector only**. The audio you hear is never filtered |

A peak detector with a fast attack follows the biggest waveform in the signal, and in a mix that is almost always the kick and the bass. Without the filter, every kick pulls the compressed path down. That can be the sound you want (the original has no filter), but if you hear the whole mix "pumping" with the low end, an SC HPF around 80–150 Hz makes the compressor react to the midrange instead. Because only the detector is filtered, the bass in the output stays exactly as loud and as full as before.

### ③ Rectifier and stereo link (Link, v2)

Each channel's detector signal is rectified to its magnitude, `|x|`. The **Link** control then blends each channel's magnitude toward the louder of the two:

```
det_L = |L| + Link · (max(|L|, |R|) − |L|)
det_R = |R| + Link · (max(|L|, |R|) − |R|)
```

- **Link 0 %** (default, as the original): each channel is compressed by its own level. A hi-hat panned right compresses only the right side. This sounds wider and livelier, but a loud off-centre event can briefly pull the stereo image toward the other side.
- **Link 100 %**: both channels get the same gain, driven by whichever side is louder. The stereo image stays exactly where it is, which is the safer choice on a full stereo master.
- In between, the image shift is partly reduced.

### ④ Envelope follower

This is the original 2006 equation, recovered from the SynthMaker schematic:

```
in  = det + 1e-8
c   = (in >= env) ? c_attack : c_release
env = c · (env − in) + in − 1e-8
```

It is a **peak** follower (not RMS). When the signal is above the envelope it uses the attack coefficient, and when it is below it uses the release coefficient. The tiny `1e-8` offsets keep the follower out of the denormal range on silence.

The coefficients use SynthMaker's convention, where a time means *time to fall to 1 % (−40 dB)*:

```
c = 0.01 ^ (1 / (t · fs))
```

| | Nominal time (to 1 %) | Equivalent RC time constant τ |
|---|---|---|
| Attack | 0.2 ms | 0.043 ms |
| Release | 250 ms | 54.3 ms |

These times are fixed and independent of the sample rate. What they mean in practice:

- **Attack is effectively instant.** The envelope reaches a new peak within a few samples, so the detector follows individual waveform peaks. There is no "slow attack lets the transient through" behaviour in the detector itself; transients are kept by the dry path and shaped by the lookahead (stage ⑥).
- **Release recovers at a constant rate in dB.** The envelope falls about 160 dB per second. The gain reduction is 40 % of that (see stage ⑤), so it recovers at **about 64 dB per second**: 20 dB of reduction takes ≈ 0.27 s to fall back to 3 dB. This fairly quick release is what makes the effect breathe with the music instead of sitting as a static level change.
- **Low frequencies ripple.** Between two peaks of a 40 Hz wave (12.5 ms apart after rectification) the envelope sags about 2 dB, which becomes ≈ 0.8 dB of gain ripple. That is a mild waveform-rate modulation on heavy bass, part of the original's character. The SC HPF (stage ②) removes it by keeping bass out of the detector.

### ⑤ Gain computer

The envelope is compared against a fixed threshold of **−50 dBFS**. Above it, the compressed path is reduced by **0.4006 dB for every dB of overshoot**:

```
over   = max(env_dB − (−50), 0)
GR_dB  = −over · (1 − 0.5994)          →   g = (env / threshold) ^ −0.4006
```

- **Output slope 0.5994 = ratio ≈ 1.67 : 1, hard knee.** The constant `0.5994` from the schematic is the output slope of the compressed path. It rises 0.6 dB for every 1 dB of input, a gentle ratio of about 1.67 : 1.
- **The threshold is extremely low.** At −50 dBFS, practically everything except room tone, fades and reverb tails is above it, so the compressed path behaves as a constant-ratio compressor across the whole musical range.
- **Maximum reduction.** A full-scale 0 dBFS peak gets 20.03 dB of reduction in the compressed path.
- **Below −50 dBFS** the gain is exactly 1 (unity), so the quietest material is never touched.

Magnitude computes this every sample with one `log2` and one `exp2`, and skips both whenever the envelope is below the threshold. The 2006 original recalculated only every 64 samples to save CPU.

### ⑥ LookAhead delay

| | |
|---|---|
| Range | 0–100 ms (default 50 ms), on the **audio path only** |
| Resolution | Whole samples when settled (e.g. 50 ms = 2400 samples at 48 kHz) |
| Changes | Glide over ≈ 30 ms with 4-point Hermite interpolation, then snap to the exact sample, so there are no clicks |

The detector hears the input immediately while the audio is held back, so the gain `g` is applied *LookAhead milliseconds earlier* relative to the music. Combined with the near-instant attack:

- **0 ms**: the gain clamps down on the transient itself. The first fraction of a millisecond of each hit passes the compressed path before the envelope catches up.
- **A few ms**: the compressed path is already down when the hit arrives, with no overshoot.
- **Tens of ms (the 50–70 ms of the Default, "Look at that!" and "Buk a Chuck a" presets)**: the compressed path starts dipping well *before* each hit. The quiet material just ahead of a drum is ducked and then swells back after it with the release. The dry path is untouched, so the hit itself stays sharp while the surroundings move. This audible "breathing into the hit" is a big part of the original's sound, and why the lookahead range goes far beyond what transient control alone would need.

The delay always counts as latency, and Magnitude reports it to your DAW (see stage ⑩).

### ⑦ Parallel blend: where upward compression happens

```
out = dry · (1 − Amount) + dry · g · Amount   =   dry · (1 − Amount + Amount · g)
```

**Amount** sets how much of the compressed copy replaces the dry signal (0 % = dry only, 100 % = fully compressed). Because the two copies are the same signal at different gains, the blend collapses into a single gain factor. That means no comb filtering or phase interaction, whatever the setting.

The resulting level change, for a steady signal at a given peak level (Out Gain 0 dB, before the saturator):

| Input level | Amount 25 % | Amount 50 % | Amount 75 % | Amount 100 % |
|---:|---:|---:|---:|---:|
| −60 dBFS | 0.00 dB | 0.00 dB | 0.00 dB | 0.00 dB |
| −50 dBFS | 0.00 dB | 0.00 dB | 0.00 dB | 0.00 dB |
| −40 dBFS | −0.84 dB | −1.77 dB | −2.82 dB | −4.01 dB |
| −30 dBFS | −1.42 dB | −3.11 dB | −5.22 dB | −8.01 dB |
| −20 dBFS | −1.80 dB | −4.08 dB | −7.17 dB | −12.02 dB |
| −10 dBFS | −2.05 dB | −4.75 dB | −8.67 dB | −16.02 dB |
| 0 dBFS | −2.21 dB | −5.20 dB | −9.77 dB | −20.03 dB |

Read the table column by column:

- Material below −50 dBFS is never changed, and the louder the material, the more it comes down.
- At partial Amounts the reduction levels off for loud signals instead of growing forever, because the dry share always passes. At 50 %, loud signals can lose at most about 6 dB. The curve therefore bends smoothly, like a soft knee, even though the compressed path itself has a hard knee.
- Raise **Out Gain** by roughly the reduction your loudest parts receive and the peaks return to where they were, while everything quieter ends up louder. That is the upward compression the original manual describes: *"it increases the level of quiet portions in a mix without reducing the level of attack transients."*

The **Transfer** view in the editor draws exactly this curve, including Out Gain and the saturator, with a dot at the current detector level.

### ⑧ Out Gain

A plain gain of ±50 dB, smoothed over 10 ms. It sits **before** the saturator, so it doubles as the saturator's drive. Turning it up both restores level after the blend and pushes the signal harder into stage ⑨.

### ⑨ Saturator (Color) and HQ oversampling

The original ends with a fixed soft clipper. Magnitude exposes its internal drive input (`clip`, fixed at 1.0 in 2006) as **Color**:

```
y = x / (1 + Color · |x|)
```

| Input peak | Output at Color 100 % (original) |
|---:|---:|
| −40 dBFS | −40.09 dBFS |
| −20 dBFS | −20.83 dBFS |
| −12 dBFS | −13.95 dBFS |
| −6 dBFS | −9.53 dBFS |
| 0 dBFS | −6.02 dBFS |
| +6 dBFS | −3.53 dBFS |
| +12 dBFS | −1.95 dBFS |

What this means for the sound:

- **It is always on, and it is not subtle at Color 100 %.** It already shapes material at −20 dBFS, and the output can never reach full scale; it approaches ±1/Color asymptotically. This gentle, ever-present rounding is part of the 2006 character, and it also acts as a safety limiter after Out Gain.
- **Odd harmonics only.** The curve is symmetric, so it adds odd harmonics, more of them the harder it is driven.
- **Lower Color for a cleaner output.** At 0 % the stage is perfectly linear, so you get only the compression with no saturation, and you then need to watch your output peaks.

**HQ** (default on) runs this stage at 4× the sample rate:

- **Why it matters:** a waveshaper creates harmonics above Nyquist, and at the base rate they fold back into the audible band as inharmonic aliasing. With HQ, a 7 kHz sine at 0 dBFS goes from −29.7 dB of aliasing to −65.4 dB.
- **How it works:** two cascaded linear-phase half-band FIR stages (127 and 31 taps, Kaiser-windowed, ≈ 100 dB stopband). The passband is flat up to 19.8 kHz at 44.1 kHz (21.6 kHz at 48 kHz). Only the saturator is oversampled; compression is unaffected.
- **Cost:** 71 samples of latency (1.6 ms at 44.1 kHz), reported to the host, and about 0.6 % of one Apple Silicon core.

### ⑩ Latency and delay compensation

Total latency = LookAhead in samples + 71 samples when HQ is on. For example, the default 50 ms with HQ at 48 kHz is 2400 + 71 = 2471 samples (≈ 51.5 ms). Magnitude reports this to the DAW, so the track stays in time with the rest of your session.

Changing LookAhead or HQ changes the latency. Most DAWs briefly restart audio processing to re-align, which is normal. For live monitoring or tracking through Magnitude, set LookAhead to 0 ms and, if needed, turn HQ off for zero latency.

### ⑪ Metering

- **GR** (one bar per channel) shows the reduction you actually hear after the blend, `−20·log10(1 − Amount + Amount·g)`, not the raw reduction of the compressed path. It therefore matches the table in stage ⑦ and moves with Amount.
- **Out** shows output sample peaks, −60 to 0 dBFS.
- The **Activity** view scrolls about 4 seconds of detector level (grey) and gain reduction (orange), which makes the release behaviour and any bass-driven pumping easy to see.

### ⑫ Parameter smoothing

Amount, Out Gain and Color are smoothed per sample over 10 ms, so automation never zippers. LookAhead uses its own glide (stage ⑥). Link and SC HPF are applied once per audio block, since they only affect the detector.

### Starting points

| Goal | Settings |
|---|---|
| Original default | LookAhead 50 ms, Amount 25 %, Out Gain 0 dB, Color 100 %, Link 0 %, SC HPF off |
| Transparent density on a master | LookAhead 0–5 ms, Amount 20–35 %, Out Gain +1.5 to +2.5 dB, Color 30–60 %, Link 100 %, SC HPF 100 Hz |
| Room and ambience lift on drums | LookAhead 50–70 ms, Amount 60–75 %, Out Gain to taste, Color 100 %, Link 0 % |
| Parallel "squash" without bass pumping | Amount 75–100 %, SC HPF 120–150 Hz, lower Color if peaks get crunchy |

---

## Reverse engineering

The original `Gersic_Pressure.dll` was unpacked from UPX, and its OutSim SynthMaker 0.9.5 payload was decrypted (4-byte rotation `[2, 4, 5, 3]`) to recover `schematic.osm`.

- **Code components:** the envelope follower, transfer function, delay and soft clipper are stored in the schematic as plain text.
- **Parameter values:** they come from the binary parameter block: `Threshold = -50.0`, `Attack = 0.0002`, `Release = 0.25`, `Ratio = 0.5994`, `softclip = 1.0`.
- **Ballistics convention:** the coefficients cached in the two `co_eff_calc` modules (0.593257 and 0.999582, computed at 44.1 kHz) confirm the 1 % time convention used in stage ④.

---

## What changed in v2

### Fidelity fixes (v1 did not match the original)

1. **Envelope ballistics.** SynthMaker's `co_eff_calc` defines a time as *time to decay to 1 %*. The schematic even caches the coefficients it computed at 44.1 kHz (0.593257 and 0.999582), and those match `0.01^(1/(t·fs))` exactly. v1 used `exp(−1/(t·fs))`, which made the release about 4.6× slower than the original. Measured recovery from 20 dB to 3 dB of GR: **v1 1220 ms → v2 266 ms**.
2. **Stereo detection.** The original runs its compressor on SSE lanes and has separate `reduction1` / `reduction3` outputs, which points to independent per-channel detection. v1 always linked the channels with `max(|L|, |R|)`. v2 defaults to unlinked, and **Link** brings back the v1 behaviour.
3. **Parameter smoothing.** v1 called the smoother once per *block*, but Truce's `read()` advances it by one *sample*. Amount and gain moved in block-sized steps, and a "10 ms" smoother took about 10 blocks. v2 smooths every sample.
4. **Latency.** v1 delayed the audio by the lookahead (50 ms by default) without telling the host, so the track played late against the rest of the session. v2 reports its latency, and it changes with LookAhead and HQ.
5. **Lookahead changes.** v1 jumped between whole-sample delays and clicked when you moved the knob. v2 glides with Hermite interpolation and snaps to the exact sample when it settles.

### Sound quality

- **Alias-free saturator.** At its original setting `x/(1+|x|)` is a strong nonlinearity, and at 1× it folds harmonics back into the audio band. With HQ on, a 7 kHz sine at 0 dBFS goes from **−29.7 dB of aliasing (v1) to −65.4 dB (v2)**. The oversampler is two Kaiser-windowed half-band FIR stages (127 + 31 taps, ~100 dB stopband) with integer latency.
- Gain computation runs every sample in the log2 domain. The original updated it every 64 samples (`hop(64)`), a 2006 CPU saving that stepped the gain.

### Performance

- The delay buffer is a power-of-two ring sized to the sample rate (64 KB at 48 kHz instead of a fixed 1.5 MB), so it fits in cache and wraps with a bit mask.
- One `log2` + `exp2` per channel, skipped completely below the threshold, plus one multiply for the blend.
- dB-to-linear for Out Gain only runs while the value moves. The FIR dot products are written so they vectorise with NEON.
- Measured on Apple Silicon, 60 s of stereo at 44.1 kHz: v1 2568× real time, v2 1928×, v2 with HQ 175× (about 0.6 % of one core).

Run the comparison yourself (the v1 DSP is kept verbatim in `examples/analyze/v1_dsp.rs`):

```bash
cargo run --release --example analyze
```

---

## Validation

Every release is checked with `cargo truce validate`: **auval** (AU v2 and v3), **pluginval** (VST3) and **clap-validator 0.4.1** (CLAP: 37 passed, 7 skipped for extensions the plugin doesn't implement).

clap-validator found two spec violations in Truce 6.3.0's CLAP wrapper: `clap_host_latency::changed` was called while the plugin was active, and state/preset loads didn't request a parameter rescan. They are fixed in a vendored copy (`vendor/truce-clap`, see `PATCHED.md`) that the build uses through `[patch.crates-io]`. The fixes are proposed upstream in [truce-audio/truce#238](https://github.com/truce-audio/truce/pull/238); the vendored copy goes away once they ship.

---

## Factory presets

All 10 original presets are included. They contain only the three original controls, so they load with the v2 extras at their defaults (Color 100 %, Link 0 %, SC HPF off), which is the original sound.

| Preset | LookAhead | Amount | Out Gain |
| :--- | :--- | :--- | :--- |
| Default | 50.0 ms | 25.0 % | 0.0 dB |
| Very Light Compression | 0.0 ms | 12.5 % | 0.0 dB |
| Light Compression | 0.0 ms | 26.25 % | 0.0 dB |
| Medium Compression | 0.0 ms | 50.0 % | 0.0 dB |
| Heavy Compression | 0.0 ms | 73.75 % | 0.0 dB |
| Squashed | 0.0 ms | 100.0 % | 0.0 dB |
| Look at that! | 68.75 ms | 50.0 % | 0.0 dB |
| Buk a Chuck a | 70.0 ms | 72.5 % | 0.0 dB |
| Number Nine Number Nine | 50.0 ms | 33.0 % | 0.0 dB |
| All the way up | 0.0 ms | 8.75 % | 0.0 dB |

Parameter IDs are hashes of the field names, and the three original fields kept their names, so v1 projects and presets load unchanged in v2.

---

## Building from source

Requirements: [Rust](https://rustup.rs/) 1.92+ and the Truce CLI (`cargo install cargo-truce`).

```bash
git clone https://github.com/nagualcode/magnitude.git
cd magnitude

cargo truce install --clap --vst3 --au2 --user    # build + install into your user plugin folders
cargo truce run                                   # standalone, no DAW needed
cargo test --release                              # DSP, real-time-safety, preset and GUI snapshot tests
cargo truce package --formats clap,vst3,au2 --user --host-only   # .pkg installer in target/dist
```

## Project structure

```
├── src/
│   ├── lib.rs            # parameters, block processing, meters, latency
│   ├── dsp.rs            # detector, gain computer, lookahead, saturator
│   ├── oversampling.rs   # 4x linear-phase half-band oversampler
│   ├── editor.rs         # egui editor (Live-style device)
│   └── main.rs           # standalone runner
├── examples/analyze/     # v1 vs v2 measurements (release, aliasing, CPU)
├── presets/              # 10 original factory presets
├── tests/presets.rs      # preset integrity tests
├── screenshots/          # GUI snapshot baseline
├── assets/               # plugin icons
├── vendor/truce-clap/    # Truce CLAP wrapper with two spec fixes (see PATCHED.md)
└── legacy/               # original 2006 artifacts (DLL, schematic, manual)
```

## Credits & license

- **Original algorithm & concept (PRESSURE, 2006):** Tom & Jeremy Gersic. Magnitude is an independent project and is not affiliated with or endorsed by them.
- **Magnitude (reverse engineering, DSP, Rust implementation):** [nagualcode](https://github.com/nagualcode) (2026).
- **Vendor:** Jardim Sonoro.
- Magnitude's source code is released under the [MIT License](LICENSE). The files in `legacy/` are the original authors' work and are included for research and reference only.
