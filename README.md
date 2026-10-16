# 🎛️ Gersic Pressure: Born Again

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Framework: Truce](https://img.shields.io/badge/Framework-Truce%20v6.3-purple.svg)](https://github.com/truce-audio/truce)
[![Rust: 2024](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](https://www.rust-lang.org/)
[![Formats: CLAP | VST3 | AU](https://img.shields.io/badge/Formats-CLAP%20%7C%20VST3%20%7C%20AU-brightgreen.svg)]()
[![GitHub Release](https://img.shields.io/github/v/release/nagualcode/gersicpressurebornagain?color=blue)](https://github.com/nagualcode/gersicpressurebornagain/releases)

> *"Pressure Mastering Compressor uses a technique called upward compression; it increases the level of quiet portions in a mix without reducing the level of attack transients like a traditional compressor."*  
> — **Jeremy Gersic, 2006**

**Gersic Pressure: Born Again** is a bit-exact, modern recreation of the legendary 2006 VST dynamics processor **PRESSURE Upward Compressor** originally designed by Tom & Jeremy Gersic.

Engineered with **Rupert Neve-caliber sound engineering precision** and built on the modern [Truce](https://github.com/truce-audio/truce) audio plugin framework in Rust, this version brings the iconic algorithm to 64-bit modern DAWs (macOS, Windows, Linux) in **CLAP, VST3, AU v2/v3, and Standalone** formats with **zero-allocation real-time safety**.

In your DAW (Logic Pro, Ableton Live, Reaper, Bitwig, Studio One, Cubase, FL Studio), the plugin is published under the vendor folder **Jardim Sonoro**.

---

## ⚡ Quick Download (macOS Pre-compiled Binaries)

Ready-to-use binaries are available under [Releases](https://github.com/nagualcode/gersicpressurebornagain/releases):
- **Universal macOS Installer (`.pkg`)** and **Bundle Archive (`.zip`)** supporting both Apple Silicon (ARM64) and Intel (x86_64).
- Includes **CLAP**, **VST3**, and **Audio Unit (AU v2/v3)** formats + factory presets.

---

## 🎵 Why Upward Compression? (The Rupert Neve Perspective)

In traditional **downward compression**, signals exceeding a threshold are attenuated. While effective for controlling peak excursions, aggressive downward compression frequently robs a mix of its initial transient punch (*crest factor*), dulling snare hits, acoustic guitar picks, and vocal plosives.

**Upward compression** flips this paradigm:
1. High-level signals exceeding $-50\text{ dBFS}$ are attenuated along the parallel (*wet*) path.
2. Low-level dynamic content (below $-50\text{ dBFS}$) passes through with zero attenuation ($0\text{ dB}$ gain reduction).
3. Summing the delayed original signal (*dry*) with the compressed signal (*wet*) lifts the noise floor, subtle room reverberations, cymbal decays, and vocal breath nuances while attack transients remain completely uncompromised through the direct path.

---

## 🔬 Reverse Engineering & DSP Architecture

The original 2006 binary (`Gersic_Pressure.dll`) was unpacked from UPX and its proprietary OutSim SynthMaker 0.9.5 container decrypted (4-byte circular ROR rotation `[2, 4, 5, 3]`), extracting the original circuit blueprint (`schematic.osm`).

### Exact Signal Flow and DSP Equations:

```
Audio In [L, R] ──┬─────────────────────────────► [Lookahead Delay Buffer] ───► Dry [L, R] ──┬──► (1 - Amount) ──┐
                  │                                                                          │                    │
                  ▼                                                                          ▼                    ▼
          [Sidechain Detector]                                                           [Gain Mult]            [+]
          normIn = max(|L|, |R|) + 1e-8                                                  G_lin = 10^(GR/20)       │
                  │                                                                          ▲                    │
                  ▼                                                                          │                    │
          [Envelope Follower]                                                           Wet [L, R] ──► (Amount) ──┘
          c_atk = exp(-1 / (0.0002 * fs))                                                            │
          c_rel = exp(-1 / (0.25 * fs))                                                              ▼
                  │                                                                             [Parallel Mix]
                  ▼                                                                                  │
          [Log Transfer Curve]                                                                       ▼
          dB_in = 20 * log10(max(env, 1e-10))                                                   [Makeup Gain]
          over  = max(dB_in - (-50.0 dB), 0)                                                    gain * 10^(dB/20)
          GR_dB = -over * (1 - 0.5994)                                                               │
                                                                                                     ▼
                                                                                              [Rational Clipper]
                                                                                              y = x / (1 + |x|)
                                                                                                     │
                                                                                                     ▼
                                                                                               Audio Out [L, R]
```

1. **Lookahead Audio Delay Buffer:**
   - Circular ring buffer with zero audio-thread memory allocation.
   - Delay range: `0.0 ms` to `100.0 ms` (default: `50.0 ms`).
   - The peak detector monitors the un-delayed incoming audio, enabling true overshoot-free transient tracking without phase distortion or envelope discontinuity.

2. **Sidechain Peak Detector:**
   - Magnitude detection: $\text{normIn} = \max(|L|, |R|) + 10^{-8}$.
   - Attack ballistics: $\tau_{\text{attack}} = 0.2\text{ ms}$ ($0.0002\text{ s}$).
   - Release ballistics: $\tau_{\text{release}} = 250\text{ ms}$ ($0.25\text{ s}$).

3. **Logarithmic Transfer Curve:**
   - Threshold: $-50.0\text{ dBFS}$.
   - Compression Ratio: $0.5994$ ($\approx 0.6$).
   - Gain reduction: $\text{GR}_{\text{dB}} = -\text{over} \times (1.0 - 0.5994) = -\text{over} \times 0.4006$.

4. **Parallel Upward Mix:**
   - $\text{mixed} = \text{dry} \cdot (1.0 - \text{Amount}) + \text{wet} \cdot \text{Amount}$.

5. **Makeup Gain (*Out Gain*):**
   - $-50.0\text{ dB}$ to $+50.0\text{ dB}$ ($10^{\text{gain}_{\text{dB}} / 20}$).

6. **Neve-Grade Rational Soft Saturator (*Soft Clipper*):**
   - Smooth algebraic saturation curve: $y = \frac{x}{1.0 + |x|}$.
   - Harmonically limits peaks near $0\text{ dBFS}$ with vintage transformer warmth, avoiding harsh digital clipping.

---

## 🎛️ Original Factory Presets

All 10 original 2006 Gersic Pressure factory presets have been restored, converted to Truce `.trucepreset` format, and validated:

| Preset | LookAhead | Amount | Out Gain | Sonic Profile |
| :--- | :--- | :--- | :--- | :--- |
| **Default** | `50.0 ms` | `25.0%` | `0.0 dB` | Balanced default mastering setup |
| **Very Light Compression** | `0.0 ms` | `12.5%` | `0.0 dB` | Subtle dynamic lift for acoustic sources |
| **Light Compression** | `0.0 ms` | `26.25%` | `0.0 dB` | Smooth upward compression with zero latency |
| **Medium Compression** | `0.0 ms` | `50.0%` | `0.0 dB` | Optimal density for master bus & subgroup stems |
| **Heavy Compression** | `0.0 ms` | `73.75%` | `0.0 dB` | Dense compression exposing background textures |
| **Squashed** | `0.0 ms` | `100.0%` | `0.0 dB` | 100% wet upward path for extreme presence |
| **Look at that!** | `68.75 ms` | `50.0%` | `0.0 dB` | Extended lookahead with organic dynamic punch |
| **Buk a Chuck a** | `70.0 ms` | `72.5%` | `0.0 dB` | Punchy room ambience for drum kits & rhythm stems |
| **Number Nine Number Nine** | `50.0 ms` | `33.0%` | `0.0 dB` | Classical mastering balance |
| **All the way up** | `0.0 ms` | `8.75%` | `0.0 dB` | Transparent mastering touch |

---

## 🚀 Building & Installing from Source

### Prerequisites
- [Rust toolchain](https://rustup.rs/) (version 1.85+)
- Truce CLI:
  ```bash
  cargo install cargo-truce
  ```

### Build & Install to Your DAW (macOS / Linux / Windows)
```bash
# Clone repository
git clone https://github.com/nagualcode/gersicpressurebornagain.git
cd gersicpressurebornagain

# Build and install CLAP, VST3, and AU directly into your user plugin paths
cargo truce install --clap --vst3 --au2 --user
```

The plugin will appear in your DAW under the **Jardim Sonoro** manufacturer category.

### Run as Standalone App (No DAW required)
```bash
cargo run --bin gersic-pressure-standalone --features standalone
```

### Run Real-Time Safety & DSP Test Suite
```bash
cargo test
```
*Enforces real-time memory-safety verification (`enable_rt_paranoid!`) and bit-exact preset round-trips.*

---

## 📂 Project Structure

```
gersicpressurebornagain/
├── Cargo.toml               # Package configuration & Truce audio dependencies
├── truce.toml               # Plugin metadata (Vendor: Jardim Sonoro, CLAP, VST3, AU)
├── LICENSE                  # MIT License
├── README.md                # Comprehensive documentation
├── src/
│   ├── lib.rs               # Parameter declarations, UI layout & Truce PluginLogic
│   ├── dsp.rs               # Pure DSP core (delay buffer, detector, saturator)
│   └── main.rs              # Standalone executable runner
├── presets/                 # 10 original factory presets (.trucepreset)
├── tests/
│   └── presets.rs           # Preset integrity and boundary unit tests
├── bundles/                 # Pre-compiled bundles (CLAP, VST3, AU)
└── legacy/                  # 2006 original artifacts (x86 DLLs, schematic.osm, manual)
```

---

## 📜 Credits & License

- **Original Algorithm & Concept:** Tom & Jeremy Gersic (2006).
- **Reverse Engineering & Rust Recreation:** [nagualcode](https://github.com/nagualcode) (2026).
- **Vendor / Publisher:** Jardim Sonoro.
- **License:** Open source under the [MIT License](LICENSE).
