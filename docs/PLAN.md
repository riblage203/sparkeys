# sparkeys — architecture plan

Native piano MIDI visualizer (Rust, winit + wgpu + egui). Two modes:
**Playback** (falling notes from a `.mid`, SoundFont audio) and **Live** (USB MIDI keyboard,
notes rise from the keys).

## Threads

| Thread | Owns | Talks via |
|---|---|---|
| Main / render | winit loop, wgpu, egui, `Song`, visual state | sends `AudioCmd` over an `rtrb` ring; reads the audio clock atomics |
| Audio (cpal callback) | `rustysynth::Synthesizer`, playback cursor | receives `AudioCmd`, publishes `frames_rendered` + position atomics, returns retired objects on a "garbage" ring so the callback never frees memory |
| MIDI input (midir callback) | nothing heavy | pushes `LiveEvent` into two rings: one straight to audio (lowest latency), one to render |

The callback never allocates, locks or frees: songs/SoundFonts arrive as pre-built `Box`/`Arc`
and leave through the garbage ring to be dropped on the main thread.

## Module layout

```
src/
  main.rs          entry point: logging, event loop
  app.rs           winit ApplicationHandler; wires modules together, input shortcuts
  config.rs        Config (serde/toml), load/save next to exe or in OS config dir
  midi/
    mod.rs         Note, Song, TrackInfo, PedalEvent
    tempo.rs       TempoMap: ticks -> seconds for any PPQ / SMPTE, many tempo changes
    parse.rs       midly -> Song (note pairing, vel-0 note-off, overlapping same pitch)
  timeline/
    mod.rs         Transport (play/pause/seek/speed/loop A-B)
    clock.rs       ClockSource (wall clock in phase 1, audio clock in phase 2) + VisualClock smoothing
  audio/
    mod.rs         AudioEngine (cpal stream setup, device errors -> UI)
    commands.rs    AudioCmd enum, ring buffers
    sequencer.rs   audio-thread cursor over Song events, sustain/CC handling, speed
  live/
    mod.rs         midir device listing, connect, hot-plug polling, LiveEvent
  render/
    mod.rs         Renderer: frame graph, owns all passes
    gpu.rs         instance/surface/device/queue, resize
    layout.rs      88-key geometry: pitch -> (x, width, is_black), shared by notes & keyboard
    background.rs  gradient + vignette/noise
    notes.rs       instanced note quads (playback falling / live rising)
    keyboard.rs    instanced keys, press depth + glow
    particles.rs   GPU particle pool (compute sim, 100k cap)
    bloom.rs       HDR target, threshold + dual-filter down/up chain, composite + tonemap
    shaders/*.wgsl one file per pass
  ui/
    mod.rs         egui integration (context, winit state, egui-wgpu renderer)
    panels.rs      transport bar, settings, track list, device/SoundFont pickers, errors
```

## Key data structures

```rust
// midi
pub struct Note { start_s: f64, end_s: f64, pitch: u8, velocity: u8, channel: u8, track: u16 }
pub struct Song {
    notes: Vec<Note>,            // sorted by start_s -> binary-search culling
    max_note_len_s: f64,         // widen the culling window so long notes aren't dropped
    events: Vec<TimedEvent>,     // all channel events in seconds, sorted; what the audio sequencer plays
    pedal: Vec<PedalSpan>,       // CC64 down/up spans per channel, for the visual indicator only
    tracks: Vec<TrackInfo>,      // name, note count, color index, muted/hidden
    duration_s: f64,
}
pub struct TempoMap { ppq: u16, segments: Vec<TempoSegment> } // {tick, start_s, us_per_quarter}

// timeline
// Audio thread publishes song position as f64 bits in an AtomicU64 + the wall Instant it
// corresponds to; VisualClock extrapolates with speed and slews toward it (no jitter, no drift).
pub struct VisualClock { last_audio_pos: f64, smoothed: f64, speed: f64, .. }

// audio
pub enum AudioCmd { Play, Pause, Seek(f64), SetSpeed(f32), SetVolume(f32), LoadSong(Arc<Song>),
                    LoadSoundFont(Box<Synthesizer>), Live(LiveEvent), AllNotesOff }

// render: GPU instance layouts (bytemuck Pod)
struct NoteInstance { rect: [f32; 4], color: [f32; 4], params: [f32; 4] } // velocity, radius, ...
struct KeyInstance  { rect: [f32; 4], color: [f32; 4], press: f32, glow: f32, is_black: u32, _pad: u32 }
struct Particle     { pos: [f32; 2], vel: [f32; 2], color: [f32; 4], life: f32, max_life: f32, size: f32, seed: f32 }
```

## Render frame graph

1. Background (fullscreen triangle) → HDR `Rgba16Float` target
2. Notes (instanced rounded rects, gradient, edge) → HDR
3. Hit line + keyboard (instanced) → HDR
4. Particles: compute update pass, then additive instanced draw → HDR
5. Bloom: threshold/downsample chain → upsample chain (dual-filter)
6. Composite + ACES-ish tonemap → swapchain (sRGB)
7. egui on top → swapchain

Per frame CPU work is bounded by what is visible: `partition_point` on `notes` for
`[t - max_note_len, t + fall_window]`, never a full scan.

## Phases

0. Scaffold: window, wgpu, background shader, egui overlay *(this phase)*
1. MIDI + tempo map (tests) + flat notes + keyboard + wall clock transport
2. Audio: rustysynth + cpal, audio master clock, sustain, volume, A/V sync check
3. Effects: HDR/bloom, note styling, hit line, particles, key glow, profiling
4. Live mode: midir, low latency path, rising notes, hot-plug
5. Polish: settings persistence, drag-and-drop, fullscreen, errors, README
