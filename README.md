# sparkeys

Native piano MIDI visualizer with glowing falling notes, embers and a live MIDI mode — Rust, `wgpu`, `egui`.

- **Playback** — open a `.mid` file: notes fall onto an animated 88-key keyboard, played through a SoundFont of your choice. Audio is the master clock, so picture and sound stay in sync at 0.25×–2×.
- **Live** — play a USB MIDI keyboard: notes grow upward from the keys, sparks scale with how hard you play, low-latency sound.
- HDR bloom, rounded glowing notes, GPU particles (up to 100 000 embers), pedal indicator, loop A–B, per-track mute/hide, palettes, colour by track or by hand.

## Build and run (Windows 10/11)

1. Install Rust (stable) from <https://rustup.rs>. The installer offers the **Visual Studio C++ Build Tools** if they are missing — accept them (the "Desktop development with C++" workload).
2. Build and start:

   ```bash
   cargo run --release
   ```

   You can pass a MIDI file to open directly:

   ```bash
   cargo run --release -- path\to\song.mid
   ```

The executable ends up in `target\release\sparkeys.exe`; it needs no other files.

Linux and macOS should build too (Linux needs the ALSA development package, e.g. `libasound2-dev`), but only Windows is tested.

## Getting a SoundFont

sparkeys ships without sounds: pick any General MIDI or piano `.sf2` with **⚙ Settings → Choose SoundFont…** (or the 🎹 SoundFont button). It is remembered for next time. Without one, everything works silently.

Free options:

- **GeneralUser GS** by S. Christian Collins — a complete, compact General MIDI set with a good piano: <https://schristiancollins.com/generaluser.php>
- **Salamander Grand Piano** (Yamaha C5, CC-BY) — search for an `.sf2` version, e.g. "SalC5Light2.sf2"; the "light" builds load quickly.
- **FluidR3_GM** (MIT licence) — classic GM set, packaged by most Linux distributions (`fluid-soundfont-gm`) and mirrored widely.
- More pianos: <https://musical-artifacts.com> (filter by `sf2`).

Large SoundFonts (hundreds of MB) load in the background; playback starts silent and sound appears when loading finishes.

## Using it

### Playback

Open a file with **📂 Open MIDI**, by dragging it onto the window, or from the command line. Then:

| Key | Action |
|---|---|
| Space | Play / pause |
| ← / → | Seek 5 s |
| Home | Back to the start |
| A / B | Set loop start / loop end (B starts looping) |
| L | Loop on / off |
| H or F1 | Hide / show the interface |
| F3 | FPS, note count and audio/visual sync readout |
| F11 | Fullscreen |

The top bar also has speed, *visible* (how many seconds of music are on screen — lower = faster falling notes), volume, **Tracks** (show/hide and mute each part) and **⚙ Settings**.

### Live mode

Click **🎹 Live** and plug in a MIDI keyboard: the first device found is opened automatically, or pick one from the list. Unplugging and re-plugging is handled; the device reconnects by itself.

No MIDI keyboard? Choose **Computer keyboard** in the device list. Keys are mapped by physical position like a piano (letters below are QWERTY labels; on AZERTY use the keys in the same places):

- bottom letter row (Z X C V B N M) = white keys from C3, the keys just above (S D G H J) = black keys;
- top letter row (Q W E R T Y U I O P) = white keys from C4, the number row above = black keys;
- ← / → shift the octave, hold Shift for full velocity.

### Latency tips (live mode)

The audio status line (in Settings) shows the device, sample rate, buffer and measured output latency.

- In **Settings → Audio**, pick your real sound card as **output**. Virtual mixers (e.g. SteelSeries Sonar, Voicemeeter) and Bluetooth headphones add tens of milliseconds.
- Lower the **buffer** (128 or 64 frames). Windows' shared audio mode often enforces a minimum of ~10 ms per buffer; the status line shows what the device actually accepted.

### Settings

Everything is saved to `config.toml` next to the executable (or, if that folder isn't writable, `%APPDATA%\sparkeys\config.toml`) when the window closes:

- **Look**: palette (Ember, Aurora, Neon, Ice), colour by track or by hand, bloom, particle amount, note corner radius, hit line, animated background.
- **Audio**: output device, buffer size, SoundFont.
- **Live**: the key that splits left/right hand colours (middle C by default).
- **Display**: frame-rate cap. Some graphics drivers ignore vsync and render thousands of frames per second; the cap (default: your monitor's refresh rate) keeps the GPU cool.

## Troubleshooting

- **No sound** — choose a SoundFont; check the audio status line in Settings and the output device.
- **"sparkeys could not start"** — the dialog shows the reason, usually the graphics driver. Update it; you can also force a backend with the environment variable `WGPU_BACKEND=dx12` or `vulkan`.
- **Logs** — run from a terminal with `RUST_LOG=sparkeys=debug` to see details.

## Test files

No MIDI or SoundFont files are bundled. Generate test songs with:

```bash
cargo run --release --example gen_midi -- demo demo.mid
```

```bash
cargo run --release --example gen_midi -- stress stress.mid 60000
```

`demo` is a two-hand piece with tempo changes and pedal; `stress` is 60 000 dense random notes for performance testing.

## Development

```bash
cargo test
```

```bash
cargo clippy --all-targets -- -D warnings
```

Tests cover the tempo map and MIDI parsing, the audio sequencer's timing at 0.5×/1×/2×, loops, the clocks, live input and layout; `tests/shaders.rs` validates every WGSL shader with `naga`, so shader mistakes fail the tests instead of the app.

Architecture notes (threads, modules, render graph) are in [docs/PLAN.md](docs/PLAN.md). In short:

- `midi` — `.mid` → `Song` (tempo map, note pairing, events, pedal)
- `timeline` — transport, audio-clock smoothing and sync
- `audio` — cpal stream, rustysynth, sequencer; real-time thread never allocates or locks
- `live` — midir input, hot-plug, computer keyboard
- `render` — wgpu: notes, keyboard, particles (compute), HDR bloom, tonemap (`src/render/shaders/*.wgsl`)
- `ui` — egui panels; `app` — window, input and glue; `config` — settings

## Licence

MIT (as declared in `Cargo.toml`).
