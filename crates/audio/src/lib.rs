//! The audio engine of music.nikgob: decoding, gapless playback, crossfade,
//! volume/normalization, visualizer tap, on top of `fastframe-audio`'s cpal
//! output.
//!
//! # Decoding and the Opus decision
//!
//! Everything except Opus is decoded by **Symphonia 0.6** (FLAC, MP3, AAC/M4A,
//! Vorbis, WAV/PCM, ALAC, AIFF, MKV). Symphonia has an Ogg *demuxer* that
//! understands Opus (granule positions, end trimming, seeking) but no Opus
//! *decoder*. For the decoder I evaluated:
//!
//! * `symphonia-adapter-libopus` / `opusic-sys` / `audiopus_sys` /
//!   `opus-static-sys`: all drive libopus through **CMake**, which is not
//!   installed here (and the project wants no system tools), so they do not
//!   build.
//! * `opus-decoder` 0.1: pure Rust but a brand-new decode-only crate.
//! * **`ropus` 0.12 (chosen)**: a pure-Rust port of libopus that is
//!   bit-exact against the C reference (fixed-point build), `decode_float`
//!   and an *encoder* included, builds with plain `rustc`. Decoding a
//!   libopus-encoded Ogg file differs from ffmpeg/libopus' float output by
//!   about -86 dBFS RMS (max 7 LSB of 16 bit), i.e. 16-bit precision, which
//!   is the nominal precision of Opus anyway. It decodes ~200x real time.
//!   The encoder also lets the test-suite generate Opus fixtures in-process.
//!
//! Opus details handled here: pre-skip (from `OpusHead`; Symphonia reports
//! packet times on the granule timeline *including* pre-skip, so the source
//! subtracts it), end trimming (Symphonia derives `trim_end` from the last
//! page's granule position; the Opus wrapper applies it), the `OpusHead`
//! output gain, and seeking by granule with an 80 ms pre-roll (decoding
//! restarts 80 ms before the target with a fresh decoder, the extra audio is
//! discarded, so the target sample is bit-identical to linear decoding).
//! Multichannel (mapping family > 0) Opus is not supported.
//!
//! # Threads
//!
//! * **Audio callback** ([`render`]): lock-free, allocation-free. Drains an
//!   `rtrb` SPSC ring of interleaved `f32` at the device rate/channel count
//!   and applies the smoothed volume, a soft limiter, de-click ramps and the
//!   visualizer tap. Where things happen in the stream (item boundaries,
//!   flushes) travels on a second SPSC queue of marks and is applied at the
//!   exact sample, so `TrackChanged` and position are sample-accurate.
//! * **Decoder thread** (`pipeline`): opens items, decodes, resamples with
//!   rubato's FFT resampler, maps channels, applies the normalization gain,
//!   mixes crossfades, and keeps about 300 ms of audio in the ring. The next
//!   item is opened and its resampler built ahead of time, so the hand-over
//!   at the end of an item is just "keep producing".
//! * **Control thread** ([`Player`]): owns the `fastframe_audio::Output`,
//!   calls `pause()`/`resume()`/`maintain()`, and turns reports into
//!   [`Event`]s.
//!
//! Latency: the ring adds no latency beyond what is buffered (about 300 ms
//! ahead, but seeks, replaces and device changes flush it at once, so they are
//! audible within a callback plus the 8 ms de-click ramp). Volume changes are
//! heard within one callback; normalization changes after the buffered
//! 300 ms. [`Player::position`] is derived from fastframe's played clock
//! (device latency already subtracted) and the position anchor published by
//! the callback, so it is smooth and independent of event timing.
//!
//! # Not covered
//!
//! AAC/M4A gapless (no iTunSMPB parsing), chained Ogg streams, Opus
//! multichannel, live looping. See the crate tests for what is verified.
#![forbid(unsafe_code)]

mod analyzer;
mod backend;
mod channels;
mod decode;
mod offline;
mod pipeline;
mod player;
mod render;
mod resample;
mod shared;
mod tap;
mod types;

pub use analyzer::Analyzer;
pub use fastframe_audio::{Buffer, BufferSize, Device, OutputOptions, output_device_names};
pub use offline::{OfflineDevice, OfflineOptions};
pub use player::{Player, PlayerOptions, WakeFn};
pub use tap::{SNAPSHOT_LEN, VisualizerTap};
pub use types::{
    DecodeError, Event, ItemSource, PcmError, PcmRead, PcmSource, PlayItem, SpawnError,
};
