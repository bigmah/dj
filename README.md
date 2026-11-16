# DJ

A keyboard-first, two-deck DJ app written in Rust with [Dioxus](https://dioxuslabs.com) (desktop) and `cpal`.

```sh
cargo run --release
```

On first launch it scans `~/Music`; add more folders with **+ Folder**. BPM, beat grid, cue points and
hot cues are saved per track in `~/Library/Application Support/dj-rs/library.json`.

## Features

- 2 decks: play/cue (CDJ-style cue preview), 8 hot cues, pitch ±8/16/25/50 %, pitch nudge, beat jump
- Automatic BPM + beat grid detection, with ÷2 / ×2 / "beat here" fixes
- Sync (tempo + phase lock, handles half/double time), quantize
- Auto loops (1/32 to 64 beats), halve/double, manual loop in/out, reloop
- Mixer: trim + auto-gain, 3-band isolator EQ with kills, one-knob LPF/HPF filter, faders, crossfader curves
- Scrolling 3-band waveforms with beat grid, track overviews (click/drag to seek)
- **Set recording**: master output → 24-bit WAV in `~/Music/DJ Sets`, plus a `.txt` tracklist and `.cue`
  sheet, built automatically from the tracks that were audible

## Keyboard

The left hand plays Deck A and the right hand plays Deck B. Hold **⌥ Option** to switch the same keys to the mixer.
Press `` ` `` in the app for the full sheet.

| Deck A | Deck B | Action |
|---|---|---|
| `S` | `L` | Play / pause |
| `A` | `;` | Cue (hold at cue point to preview) · ⇧ = jump to start |
| `D` | `K` | Sync · ⇧ = reset pitch |
| `F` `G` | `H` `J` | Nudge slower / faster (hold) · ⇧ = beat jump |
| `Z` `X` | `.` `/` | Pitch −/+ 0.1 % · ⇧ = 1 % |
| `1 2 3 4 Q W E R` | `7 8 9 0 U I O P` | Hot cues 1–8 · ⇧ = delete |
| `C` | `,` | Auto loop on/off · ⇧ = reloop |
| `V` / `B` | `M` / `N` | Loop halve / double · ⇧ = loop in / out |
| `T` | `Y` | Load selected track |
| ⌥`Q`/`A` ⌥`W`/`S` ⌥`E`/`D` | ⌥`U`/`J` ⌥`I`/`K` ⌥`O`/`L` | High / mid / low EQ up/down (hold) |
| ⌥`1` ⌥`2` ⌥`3` | ⌥`7` ⌥`8` ⌥`9` | Kill high / mid / low |
| ⌥`R` / ⌥`F` | ⌥`P` / ⌥`;` | Filter toward HPF / LPF (stops at centre) |
| ⌥`T` / ⌥`G` | ⌥`Y` / ⌥`H` | Channel volume up / down |

Global: `←` `→` crossfader (⇧ slams), `Space` centres it, `↑` `↓` browse, `Enter` loads into the stopped deck,
`Tab` search, `-` `=` zoom, ⌥`-`/⌥`=` master, `⌃R` record, `⌃Q` quantize.

## Layout

| Path | What |
|---|---|
| `src/audio/` | Real-time engine: decks, mixer, DSP, sync, recording tap (runs in the cpal callback) |
| `src/analysis.rs` | BPM/beat grid detection, waveform, auto-gain |
| `src/decode.rs` | symphonia decoding + tags |
| `src/library.rs` | Folder scan and persistent track data |
| `src/recorder.rs` | WAV writer thread, tracklist / CUE |
| `src/keys.rs` | Keyboard map (also drives the help sheet and on-screen key hints) |
| `src/ui/` | Dioxus components; `assets/bridge.js` handles keys and canvas waveforms |

Tests: `cargo test`. To check BPM detection against a real folder, run
`DJ_MUSIC=~/Music cargo test --release real_library -- --ignored --nocapture`.

## Not yet supported

Headphone cueing (needs a multi-output interface), key lock (time-stretch), MIDI controllers, and more than 2 decks.
