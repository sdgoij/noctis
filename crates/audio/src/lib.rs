//! # audio — playback and 3D spatialization for the Noctis engine
//!
//! This crate wraps [rodio](https://docs.rs/rodio) 0.22 in an [`AudioEngine`]
//! that is driven by the ECS. Playback is requested either explicitly
//! ([`AudioEngine::play`]) or by setting the `playing` field of an
//! [`AudioSource`](ecs::components::AudioSource) component and calling
//! [`AudioEngine::update`], which reconciles the world's audio components with
//! the live sounds.
//!
//! On `wasm32` playback goes through the browser's WebAudio through the same
//! [`AudioEngine`] API; the `AudioContext` is resumed on the first user gesture,
//! as the browser autoplay policy requires.
//!
//! ## Live and silent engines
//!
//! [`AudioEngine::new`] opens the default output device and returns an
//! [`AudioError`] (never a panic) if none is available. [`AudioEngine::silent`]
//! returns a device-free engine with identical behaviour except that no samples
//! are produced — ideal for tests and headless servers.
//!
//! ```
//! use audio::{AudioEngine, PlaybackConfig};
//! use ecs::assets::AudioClip;
//! use ecs::world::Entity;
//!
//! let mut engine = AudioEngine::silent();
//! let clip = AudioClip {
//!     name: "beep".into(),
//!     sample_rate: 48_000,
//!     channels: 1,
//!     samples: vec![0.0; 480],
//! };
//! let handle = engine.play(Entity::from_raw(0, 0), &clip, PlaybackConfig::default());
//! assert!(engine.is_playing(handle));
//! engine.stop(handle);
//! assert!(!engine.is_playing(handle));
//! ```
//!
//! ## Spatialization
//!
//! The [`spatial`] module contains the pure math used to place sounds relative
//! to a listener: [`spatial::attenuation`], [`spatial::pan_gains`], and
//! [`spatial::to_mono_or_stereo`]. The engine sets the listener with
//! [`AudioEngine::set_listener`] and derives each spatial emitter's position
//! from its `GlobalTransform`.

#![forbid(unsafe_code)]

pub mod clip_source;
pub mod decode;
pub mod engine;
pub mod spatial;

/// The WebAudio output backend (`wasm32` only).
#[cfg(target_arch = "wasm32")]
mod web;

pub use clip_source::ClipSource;
pub use decode::{load_clip, load_clip_bytes};
pub use engine::{
    AudioEngine, AudioError, DEFAULT_MAX_DISTANCE, DEFAULT_ROLLOFF, Listener, PlaybackConfig,
    SoundHandle,
};
pub use spatial::{MonoOrStereo, attenuation, pan_from_gains, pan_gains, to_mono_or_stereo};
