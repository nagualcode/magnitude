# 🎛️ Gersic Pressure: Born Again

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Framework: Truce](https://img.shields.io/badge/Framework-Truce%20v6.3-purple.svg)](https://github.com/truce-audio/truce)
[![Rust: 2024](https://img.shields.io/badge/Rust-2024%20Edition-orange.svg)](https://www.rust-lang.org/)
[![Formats: CLAP | VST3 | AU](https://img.shields.io/badge/Formats-CLAP%20%7C%20VST3%20%7C%20AU-brightgreen.svg)]()

> *"Pressure Mastering Compressor uses a technique called upward compression; it increases the level of quiet portions in a mix without reducing the level of attack transients like a traditional compressor."*  
> — **Jeremy Gersic, 2006**

**Gersic Pressure: Born Again** is a bit-exact, modern recreation of the legendary 2006 VST mastering dynamics processor **PRESSURE Upward Compressor** by Tom & Jeremy Gersic.

Engineered with **Rupert Neve-caliber sound engineering precision** and built on the modern [Truce](https://github.com/truce-audio/truce) audio plugin framework in Rust, this version brings the iconic algorithm to 64-bit modern DAWs (macOS, Windows, Linux) in **CLAP, VST3, AU v2, and Standalone** formats with **zero-allocation real-time safety**.

---

## 🎵 Why Upward Compression? (The Rupert Neve Perspective)

In traditional **downward compression**, signals exceeding a threshold are attenuated. While effective for controlling peak excursions, aggressive downward compression frequently robs a mix of its initial transient punch (*crest factor*), dulling snare hits, acoustic guitar picks, and vocal plosives.

**Upward compression** flips this paradigm:
1. Sinais que excedem o limiar de $-50\text{ dBFS}$ são atenuados no caminho paralelo (*wet*).
2. Sinais de baixo nível dinâmico (abaixo de $-50\text{ dBFS}$) passam sem atenuação ($0\text{ dB}$ de redução).
3. Ao somar o sinal original atrasado (*dry*) com o sinal comprimido (*wet*), o piso dinâmico e os detalhes de baixo nível (reverbs de sala, caudas de pratos, respiração vocal) são elevados, enquanto os transientes de ataque são integralmente preservados através do sinal original.

---

## 🔬 Engenharia Reversa e Arquitetura DSP

O binário original de 2006 (`Gersic_Pressure.dll`) foi descompactado (UPX) e sua estrutura de container proprietária OutSim SynthMaker 0.9.5 foi descriptografada (rotação circular de 4 bytes ROR `[2, 4, 5, 3]`), revelando o projeto de circuito original (`schematic.osm`).

### Algoritmos e Equações DSP Exatas:

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
   - Ring buffer circular sem alocação em thread de áudio.
   - Faixa de atraso: `0.0 ms` a `100.0 ms` (padrão: `50.0 ms`).
   - O detector de pico opera no sinal não atrasado, permitindo antecipação de transientes sem atraso de fase ou descontinuidade de envelope.

2. **Sidechain Peak Detector:**
   - Detecção de magnitude: $\text{normIn} = \max(|L|, |R|) + 10^{-8}$.
   - Balística de Ataque: $\tau_{\text{attack}} = 0.2\text{ ms}$ ($0.0002\text{ s}$).
   - Balística de Release: $\tau_{\text{release}} = 250\text{ ms}$ ($0.25\text{ s}$).

3. **Curva de Transferência Logarítmica:**
   - Limiar (*Threshold*): $-50.0\text{ dBFS}$.
   - Razão (*Ratio*): $0.5994$ ($\approx 0.6$).
   - $\text{GR}_{\text{dB}} = -\text{over} \times (1.0 - 0.5994) = -\text{over} \times 0.4006$.

4. **Mix Upward Paralelo:**
   - $\text{mixed} = \text{dry} \cdot (1.0 - \text{Amount}) + \text{wet} \cdot \text{Amount}$.

5. **Ganho de Saída (*Out Gain*):**
   - $-50.0\text{ dB}$ a $+50.0\text{ dB}$ ($10^{\text{gain}_{\text{dB}} / 20}$).

6. **Saturador Racional Suave (*Soft Clipper*):**
   - Transferência analógica: $y = \frac{x}{1.0 + |x|}$.
   - Limita harmonicamente picos em torno de $0\text{ dBFS}$ com distorção analógica suave, prevenindo distorção digital estridente (*harsh digital clipping*).

---

## 🎛️ Presets de Fábrica Originais

Todos os 10 presets de fábrica originais do Gersic Pressure de 2006 foram convertidos e validados:

| Preset | LookAhead | Amount | Out Gain | Descrição Sonora |
| :--- | :--- | :--- | :--- | :--- |
| **Default** | `50.0 ms` | `25.0%` | `0.0 dB` | Configuração padrão equilibrada |
| **Very Light Compression** | `0.0 ms` | `12.5%` | `0.0 dB` | Elevação dinâmica sutil para instrumentos acústicos |
| **Light Compression** | `0.0 ms` | `26.25%` | `0.0 dB` | Compressão upward suave com zero latência |
| **Medium Compression** | `0.0 ms` | `50.0%` | `0.0 dB` | Densidade ideal para mix buss |
| **Heavy Compression** | `0.0 ms` | `73.75%` | `0.0 dB` | Compressão densa revelando detalhes de fundo |
| **Squashed** | `0.0 ms` | `100.0%` | `0.0 dB` | Sinal 100% processado para máxima densidade |
| **Look at that!** | `68.75 ms` | `50.0%` | `0.0 dB` | Lookahead longo com pegada dinâmica |
| **Buk a Chuck a** | `70.0 ms` | `72.5%` | `0.0 dB` | Punch e ambiência para baterias e ritmos |
| **Number Nine Number Nine** | `50.0 ms` | `33.0%` | `0.0 dB` | Equilíbrio clássico para masterização geral |
| **All the way up** | `0.0 ms` | `8.75%` | `0.0 dB` | Toque sutil para masterização transparente |

---

## 🚀 Instalação e Compilação

### Pré-requisitos
- [Rust toolchain](https://rustup.rs/) (versão 1.85+)
- CLI do Truce:
  ```bash
  cargo install cargo-truce
  ```

### Compilar e Instalar na sua DAW (macOS / Windows / Linux)
```bash
# Clone o repositório
git clone https://github.com/nagualcode/gersicpressurebornagain.git
cd gersicpressurebornagain

# Compilar e instalar CLAP, VST3 e AU diretamente na sua máquina
cargo truce install --clap --vst3 --au2 --user
```

### Rodar como Aplicativo Standalone (sem DAW)
```bash
cargo run --bin gersic-pressure-standalone --features standalone
```

### Executar a Suíte de Testes
```bash
cargo test
```
*Garante zero alocações na thread de áudio (`enable_rt_paranoid!`) e validação de todos os presets.*

---

## 📂 Estrutura do Projeto

```
gersicpressurebornagain/
├── Cargo.toml               # Configuração do pacote e dependências Truce
├── truce.toml               # Metadados de bundles (CLAP, VST3, AU, AAX)
├── LICENSE                  # Licença aberta MIT
├── README.md                # Documentação técnica e sonora
├── src/
│   ├── lib.rs               # Interface de parâmetros, GUI e PluginLogic Truce
│   ├── dsp.rs               # Núcleo DSP puro de compressão e saturação
│   └── main.rs              # Runner standalone
├── presets/                 # 10 factory presets originais (.preset)
├── tests/
│   └── presets.rs           # Testes de integridade de presets
├── bundles/                 # Binários pré-compilados (CLAP, VST3, AU)
└── legacy/                  # Arquivos históricos originais de 2006 (DLLs, .osm, manual)
```

---

## 📜 Licença e Créditos

- **Algoritmo Original e Concepção:** Tom & Jeremy Gersic (2006).
- **Engenharia Reversa e Recriação em Rust:** nagualcode (2026).
- **Licença:** Código aberto sob a licença [MIT](LICENSE).
