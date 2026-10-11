//! The [`AudioEngine`]: rodio playback plus ECS reconciliation.
//!
//! The engine has two interchangeable backends:
//!
//! * **Live** — opened with [`AudioEngine::new`], which opens the default output
//!   device via rodio. Every sound is a real [`rodio::Player`] attached to the
//!   device mixer.
//! * **Silent** — created with [`AudioEngine::silent`], which never touches a
//!   device. It performs identical bookkeeping (volume, pitch, pan, playback
//!   state) but produces no sound, so it is safe on headless servers and in
//!   tests.
//!
//! Both expose the exact same API. [`AudioEngine::update`] reconciles live
//! sounds with the [`AudioSource`] components in a [`World`].

use std::collections::HashMap;
use std::fmt;

#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use ecs::assets::{AssetId, AssetRegistry, AudioClip};
use ecs::components::{AudioSource, GlobalTransform};
use ecs::world::{Entity, World};
use math::{Vec3, length, vec3};

#[cfg(not(target_arch = "wasm32"))]
use rodio::mixer::Mixer;
#[cfg(not(target_arch = "wasm32"))]
use rodio::source::ChannelVolume;
#[cfg(not(target_arch = "wasm32"))]
use rodio::{MixerDeviceSink, Player, Source};

#[cfg(not(target_arch = "wasm32"))]
use crate::clip_source::ClipSource;
use crate::spatial::{attenuation, pan_gains};
#[cfg(target_arch = "wasm32")]
use crate::web::{WebBackend, WebSound as LiveSink};

/// Default `max_distance` for sounds started with [`AudioEngine::play`].
pub const DEFAULT_MAX_DISTANCE: f32 = 50.0;
/// Default `rolloff` for sounds started with [`AudioEngine::play`].
pub const DEFAULT_ROLLOFF: f32 = 1.0;

/// An error opening or configuring the audio output, or decoding an audio file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    /// No output device is available.
    NoOutputDevice,
    /// The device exists but the stream could not be created.
    DeviceUnavailable(String),
    /// An audio file could not be read from disk.
    Io(String),
    /// An audio file could not be decoded (an unknown or corrupt format).
    Decode(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::NoOutputDevice => write!(f, "no audio output device available"),
            AudioError::DeviceUnavailable(message) => {
                write!(f, "audio device unavailable: {message}")
            }
            AudioError::Io(message) => write!(f, "audio io error: {message}"),
            AudioError::Decode(message) => write!(f, "audio decode error: {message}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// A handle to a sound started by the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SoundHandle(u64);

impl SoundHandle {
    /// The raw id, unique within an engine.
    pub fn id(self) -> u64 {
        self.0
    }
}

/// Parameters for a single playback request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaybackConfig {
    /// Volume multiplier (typically `1.0`).
    pub volume: f32,
    /// Playback rate multiplier; changes pitch and duration.
    pub pitch: f32,
    /// Repeat forever.
    pub looped: bool,
    /// Spatialize relative to the listener.
    pub spatial: bool,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            volume: 1.0,
            pitch: 1.0,
            looped: false,
            spatial: false,
        }
    }
}

impl From<&AudioSource> for PlaybackConfig {
    fn from(source: &AudioSource) -> Self {
        Self {
            volume: source.volume,
            pitch: source.pitch,
            looped: source.looped,
            spatial: source.spatial,
        }
    }
}

/// The listener pose used for spatialization.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    /// World-space position.
    pub position: Vec3,
    /// Normalized forward direction (into the scene).
    pub forward: Vec3,
    /// World up vector.
    pub up: Vec3,
}

impl Default for Listener {
    fn default() -> Self {
        Self {
            position: vec3(0.0, 0.0, 0.0),
            forward: vec3(0.0, 0.0, -1.0),
            up: vec3(0.0, 1.0, 0.0),
        }
    }
}

/// The active output backend.
enum Backend {
    /// No device; bookkeeping only.
    Silent,
    /// A live output. On native this is the rodio sink (which must outlive every
    /// player it created); on the web it is the shared `AudioContext`.
    #[cfg(not(target_arch = "wasm32"))]
    Live {
        _sink: MixerDeviceSink,
        mixer: Mixer,
    },
    #[cfg(target_arch = "wasm32")]
    Live { sound: WebBackend },
}

/// A live rodio player plus, for spatial sounds, the shared pan gains the
/// player's stereo source reads on a timer.
#[cfg(not(target_arch = "wasm32"))]
enum LiveSink {
    Plain(Player),
    Spatial {
        player: Player,
        gains: Arc<Mutex<(f32, f32)>>,
    },
}

#[cfg(not(target_arch = "wasm32"))]
impl LiveSink {
    fn set_volume(&self, volume: f32) {
        match self {
            LiveSink::Plain(player) => player.set_volume(volume),
            LiveSink::Spatial { player, .. } => player.set_volume(volume),
        }
    }

    fn set_speed(&self, speed: f32) {
        match self {
            LiveSink::Plain(player) => player.set_speed(speed),
            LiveSink::Spatial { player, .. } => player.set_speed(speed),
        }
    }

    fn set_gains(&self, left: f32, right: f32) {
        if let LiveSink::Spatial { gains, .. } = self
            && let Ok(mut gains) = gains.lock()
        {
            *gains = (left, right);
        }
    }

    fn pause(&self) {
        match self {
            LiveSink::Plain(player) => player.pause(),
            LiveSink::Spatial { player, .. } => player.pause(),
        }
    }

    fn resume(&self) {
        match self {
            LiveSink::Plain(player) => player.play(),
            LiveSink::Spatial { player, .. } => player.play(),
        }
    }

    fn stop(&self) {
        match self {
            LiveSink::Plain(player) => player.stop(),
            LiveSink::Spatial { player, .. } => player.stop(),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            LiveSink::Plain(player) => player.empty(),
            LiveSink::Spatial { player, .. } => player.empty(),
        }
    }
}

/// One tracked sound.
struct SoundEntry {
    entity: Entity,
    config: PlaybackConfig,
    clip: AssetId,
    max_distance: f32,
    rolloff: f32,
    /// Desired playback state (authoritative in silent mode).
    playing: bool,
    paused: bool,
    /// `true` if this sound is driven by an ECS `AudioSource`.
    managed: bool,
    sink: Option<LiveSink>,
    position: Vec3,
    attenuation: f32,
    left_gain: f32,
    right_gain: f32,
    effective_volume: f32,
}

/// Everything needed to start a sound, beyond the owning entity.
struct SoundSpec<'a> {
    clip: &'a AudioClip,
    config: PlaybackConfig,
    clip_id: AssetId,
    max_distance: f32,
    rolloff: f32,
    managed: bool,
}

impl<'a> SoundSpec<'a> {
    /// A manual (non-ECS) sound spec with the crate defaults.
    fn manual(clip: &'a AudioClip, config: PlaybackConfig) -> Self {
        Self {
            clip,
            config,
            clip_id: AssetId::NONE,
            max_distance: DEFAULT_MAX_DISTANCE,
            rolloff: DEFAULT_ROLLOFF,
            managed: false,
        }
    }
}

/// The audio engine.
pub struct AudioEngine {
    backend: Backend,
    sounds: HashMap<SoundHandle, SoundEntry>,
    entity_sounds: HashMap<Entity, SoundHandle>,
    next_id: u64,
    master_volume: f32,
    listener: Listener,
}

impl AudioEngine {
    /// Open the default output device.
    ///
    /// Returns [`AudioError`] instead of panicking when no device is available.
    /// Use [`AudioEngine::silent`] for a device-free engine.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new() -> Result<AudioEngine, AudioError> {
        let sink = rodio::DeviceSinkBuilder::open_default_sink().map_err(map_device_error)?;
        let mixer = sink.mixer().clone();
        Ok(AudioEngine::with_backend(Backend::Live {
            _sink: sink,
            mixer,
        }))
    }

    /// Open the browser `AudioContext`.
    ///
    /// The context starts suspended (the browser autoplay policy) and is resumed
    /// on the first user gesture; playback requested before then is queued. A
    /// context that cannot be created is an [`AudioError::DeviceUnavailable`], so
    /// the caller falls back to [`AudioEngine::silent`].
    #[cfg(target_arch = "wasm32")]
    pub fn new() -> Result<AudioEngine, AudioError> {
        let sound = WebBackend::new()?;
        Ok(AudioEngine::with_backend(Backend::Live { sound }))
    }

    /// Create a device-free engine that performs all bookkeeping but emits no
    /// sound. Intended for tests and headless servers.
    pub fn silent() -> AudioEngine {
        AudioEngine::with_backend(Backend::Silent)
    }

    fn with_backend(backend: Backend) -> AudioEngine {
        AudioEngine {
            backend,
            sounds: HashMap::new(),
            entity_sounds: HashMap::new(),
            next_id: 1,
            master_volume: 1.0,
            listener: Listener::default(),
        }
    }

    /// `true` if this engine is connected to a real device.
    pub fn is_live(&self) -> bool {
        matches!(self.backend, Backend::Live { .. })
    }

    /// The master volume multiplier (applied on top of per-sound volume).
    pub fn master_volume(&self) -> f32 {
        self.master_volume
    }

    /// Set the master volume multiplier, re-applying it to every live sound.
    pub fn set_master_volume(&mut self, volume: f32) {
        self.master_volume = volume;
        let master = self.master_volume;
        for sound in self.sounds.values_mut() {
            sound.effective_volume = master * sound.config.volume * sound.attenuation;
            if let Some(sink) = &sound.sink {
                sink.set_volume(sound.effective_volume);
            }
        }
    }

    /// Set the listener pose used for spatialization.
    pub fn set_listener(&mut self, position: Vec3, forward: Vec3, up: Vec3) {
        self.listener = Listener {
            position,
            forward,
            up,
        };
    }

    /// The current listener pose.
    pub fn listener(&self) -> Listener {
        self.listener
    }

    /// Start a clip on `entity` and return a handle.
    ///
    /// The entity is remembered for spatial position updates during
    /// [`AudioEngine::update`]. Sounds started this way are *manual*: `update`
    /// will spatialize them from the entity's `GlobalTransform` but will not
    /// start or stop them from ECS state.
    pub fn play(
        &mut self,
        entity: Entity,
        clip: &AudioClip,
        config: PlaybackConfig,
    ) -> SoundHandle {
        self.spawn(entity, SoundSpec::manual(clip, config))
    }

    fn spawn(&mut self, entity: Entity, spec: SoundSpec<'_>) -> SoundHandle {
        let SoundSpec {
            clip,
            config,
            clip_id,
            max_distance,
            rolloff,
            managed,
        } = spec;
        let handle = SoundHandle(self.next_id);
        self.next_id += 1;

        let sink = self.make_sink(clip, &config, (1.0, 1.0));
        let effective_volume = self.master_volume * config.volume;
        if let Some(sink) = &sink {
            sink.set_speed(config.pitch);
            sink.set_volume(effective_volume);
        }

        self.sounds.insert(
            handle,
            SoundEntry {
                entity,
                config,
                clip: clip_id,
                max_distance,
                rolloff,
                playing: true,
                paused: false,
                managed,
                sink,
                position: math::zero3(),
                attenuation: 1.0,
                left_gain: 1.0,
                right_gain: 1.0,
                effective_volume,
            },
        );
        handle
    }

    fn make_sink(
        &self,
        clip: &AudioClip,
        config: &PlaybackConfig,
        initial_gains: (f32, f32),
    ) -> Option<LiveSink> {
        match &self.backend {
            Backend::Silent => None,
            #[cfg(not(target_arch = "wasm32"))]
            Backend::Live { mixer, .. } => spawn_sink(mixer, clip, config, initial_gains),
            #[cfg(target_arch = "wasm32")]
            Backend::Live { sound } => sound.play(clip, config, initial_gains),
        }
    }

    /// Stop and forget a sound.
    pub fn stop(&mut self, handle: SoundHandle) {
        if let Some(sound) = self.sounds.remove(&handle)
            && let Some(sink) = &sound.sink
        {
            sink.stop();
        }
        self.entity_sounds.retain(|_, h| *h != handle);
    }

    /// Stop and forget every sound.
    pub fn stop_all(&mut self) {
        for (_, sound) in self.sounds.drain() {
            if let Some(sink) = &sound.sink {
                sink.stop();
            }
        }
        self.entity_sounds.clear();
    }

    /// Set a sound's volume (clamped to `>= 0`).
    pub fn set_volume(&mut self, handle: SoundHandle, volume: f32) {
        let master = self.master_volume;
        if let Some(sound) = self.sounds.get_mut(&handle) {
            sound.config.volume = volume.max(0.0);
            sound.effective_volume = master * sound.config.volume * sound.attenuation;
            if let Some(sink) = &sound.sink {
                sink.set_volume(sound.effective_volume);
            }
        }
    }

    /// Set a sound's playback rate/pitch.
    pub fn set_pitch(&mut self, handle: SoundHandle, pitch: f32) {
        if let Some(sound) = self.sounds.get_mut(&handle) {
            sound.config.pitch = pitch;
            if let Some(sink) = &sound.sink {
                sink.set_speed(pitch);
            }
        }
    }

    /// Pause a sound.
    pub fn pause(&mut self, handle: SoundHandle) {
        if let Some(sound) = self.sounds.get_mut(&handle) {
            sound.paused = true;
            if let Some(sink) = &sound.sink {
                sink.pause();
            }
        }
    }

    /// Resume a paused sound.
    pub fn resume(&mut self, handle: SoundHandle) {
        if let Some(sound) = self.sounds.get_mut(&handle) {
            sound.paused = false;
            if let Some(sink) = &sound.sink {
                sink.resume();
            }
        }
    }

    /// `true` if the sound is still active.
    ///
    /// For live sounds this reports whether the underlying player still has
    /// queued audio (so a finished one-shot reports `false`); for silent sounds
    /// it reports the tracked playback state.
    pub fn is_playing(&self, handle: SoundHandle) -> bool {
        match self.sounds.get(&handle) {
            None => false,
            Some(sound) => match &sound.sink {
                Some(sink) => !sink.is_empty(),
                None => sound.playing,
            },
        }
    }

    /// `true` if the sound is currently paused.
    pub fn is_paused(&self, handle: SoundHandle) -> bool {
        self.sounds.get(&handle).is_some_and(|s| s.paused)
    }

    /// The number of tracked sounds.
    pub fn sound_count(&self) -> usize {
        self.sounds.len()
    }

    /// The handle currently associated with an entity, if any.
    pub fn handle_for_entity(&self, entity: Entity) -> Option<SoundHandle> {
        self.entity_sounds.get(&entity).copied()
    }

    /// The most recently computed equal-power pan gains for a sound.
    pub fn channel_gains(&self, handle: SoundHandle) -> Option<(f32, f32)> {
        self.sounds
            .get(&handle)
            .map(|s| (s.left_gain, s.right_gain))
    }

    /// The most recently computed distance attenuation for a sound.
    pub fn sound_attenuation(&self, handle: SoundHandle) -> Option<f32> {
        self.sounds.get(&handle).map(|s| s.attenuation)
    }

    /// The effective per-sound volume (`master * volume * attenuation`).
    pub fn effective_volume(&self, handle: SoundHandle) -> Option<f32> {
        self.sounds.get(&handle).map(|s| s.effective_volume)
    }

    /// Reconcile sounds with the [`AudioSource`] components in `world`.
    ///
    /// For every entity with an `AudioSource`:
    ///
    /// * when `playing` is set and a clip is assigned, playback starts (or the
    ///   existing sound is updated);
    /// * when `playing` is cleared, or the component is removed, the sound stops;
    /// * `volume`, `pitch`, `looped`, and `spatial` are pushed to the live sink;
    /// * spatial sounds are attenuated and panned from the entity's
    ///   [`GlobalTransform`] relative to the listener using `max_distance` and
    ///   `rolloff`.
    ///
    /// Sounds started with [`AudioEngine::play`] are only spatialized, never
    /// started or stopped by this method.
    pub fn update(&mut self, world: &World, assets: &AssetRegistry) {
        let entities = world.entities_with::<AudioSource>();
        let mut present: Vec<Entity> = Vec::with_capacity(entities.len());

        for entity in entities {
            let Some(source) = world.get::<AudioSource>(entity).copied() else {
                continue;
            };
            present.push(entity);
            let existing = self.entity_sounds.get(&entity).copied();

            if source.playing && !source.clip.is_none() {
                match existing {
                    Some(handle) if self.sounds.contains_key(&handle) => {
                        self.apply_source(handle, &source, entity, assets);
                    }
                    _ => {
                        if let Some(clip) = assets.audio.get(source.clip) {
                            let spec = SoundSpec {
                                clip,
                                config: PlaybackConfig::from(&source),
                                clip_id: source.clip.id(),
                                max_distance: source.max_distance,
                                rolloff: source.rolloff,
                                managed: true,
                            };
                            let handle = self.spawn(entity, spec);
                            self.entity_sounds.insert(entity, handle);
                            self.apply_source(handle, &source, entity, assets);
                        }
                    }
                }
            } else if let Some(handle) = existing {
                self.stop(handle);
            }
        }

        // Sounds whose entity lost its `AudioSource` (or despawned) stop.
        let stale: Vec<Entity> = self
            .entity_sounds
            .keys()
            .copied()
            .filter(|entity| !present.contains(entity))
            .collect();
        for entity in stale {
            if let Some(handle) = self.entity_sounds.remove(&entity) {
                self.stop(handle);
            }
        }

        self.refresh_spatialization(world);
    }

    fn apply_source(
        &mut self,
        handle: SoundHandle,
        source: &AudioSource,
        entity: Entity,
        assets: &AssetRegistry,
    ) {
        let clip_changed = self
            .sounds
            .get(&handle)
            .is_some_and(|s| s.managed && s.clip != source.clip.id());
        let structural_change = self.sounds.get(&handle).is_some_and(|s| {
            s.config.spatial != source.spatial || s.config.looped != source.looped
        });

        if (clip_changed || structural_change)
            && let Some(clip) = assets.audio.get(source.clip)
        {
            let config = PlaybackConfig::from(source);
            let gains = self
                .sounds
                .get(&handle)
                .map(|s| (s.left_gain, s.right_gain))
                .unwrap_or((1.0, 1.0));
            let sink = self.make_sink(clip, &config, gains);
            if let Some(sound) = self.sounds.get_mut(&handle) {
                sound.clip = source.clip.id();
                sound.sink = sink;
            }
        }

        if let Some(sound) = self.sounds.get_mut(&handle) {
            sound.entity = entity;
            sound.config = PlaybackConfig::from(source);
            sound.clip = source.clip.id();
            sound.max_distance = source.max_distance;
            sound.rolloff = source.rolloff;
            sound.playing = source.playing;
            sound.effective_volume = self.master_volume * sound.config.volume * sound.attenuation;
            if let Some(sink) = &sound.sink {
                sink.set_speed(sound.config.pitch);
                if !sound.config.spatial {
                    sink.set_volume(sound.effective_volume);
                }
            }
        }
    }

    /// Recompute attenuation and pan for every spatial sound from the world.
    fn refresh_spatialization(&mut self, world: &World) {
        let listener = self.listener;
        let master = self.master_volume;
        for sound in self.sounds.values_mut() {
            if !sound.config.spatial {
                continue;
            }
            let emitter = world
                .get::<GlobalTransform>(sound.entity)
                .map(|transform| transform.position())
                .unwrap_or_else(math::zero3);

            let distance = length(emitter - listener.position);
            let attenuation = attenuation(distance, sound.max_distance, sound.rolloff);
            let (left, right) =
                pan_gains(emitter, listener.position, listener.forward, listener.up);

            sound.position = emitter;
            sound.attenuation = attenuation;
            sound.left_gain = left;
            sound.right_gain = right;
            sound.effective_volume = master * sound.config.volume * attenuation;

            if let Some(sink) = &sound.sink {
                sink.set_volume(sound.effective_volume);
                sink.set_gains(left, right);
            }
        }
    }
}

/// Build a live sink for a clip, wrapping it in a panned stereo chain when
/// `spatial` is set.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_sink(
    mixer: &Mixer,
    clip: &AudioClip,
    config: &PlaybackConfig,
    initial_gains: (f32, f32),
) -> Option<LiveSink> {
    let source = ClipSource::new(clip, config.looped)?;
    if config.spatial {
        let gains = Arc::new(Mutex::new(initial_gains));
        let shared = Arc::clone(&gains);
        let panned = ChannelVolume::new(source, vec![initial_gains.0, initial_gains.1])
            .periodic_access(Duration::from_millis(10), move |channel_volume| {
                if let Ok(g) = shared.lock() {
                    channel_volume.set_volume(0, g.0);
                    channel_volume.set_volume(1, g.1);
                }
            });
        let player = Player::connect_new(mixer);
        player.set_speed(config.pitch);
        player.append(panned);
        Some(LiveSink::Spatial { player, gains })
    } else {
        let player = Player::connect_new(mixer);
        player.set_speed(config.pitch);
        player.append(source);
        Some(LiveSink::Plain(player))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn map_device_error(error: rodio::DeviceSinkError) -> AudioError {
    use rodio::DeviceSinkError;
    match error {
        DeviceSinkError::NoDevice => AudioError::NoOutputDevice,
        other => AudioError::DeviceUnavailable(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ecs::assets::Assets;
    use ecs::components::GlobalTransform;
    use math::Transform;

    fn test_clip() -> AudioClip {
        AudioClip {
            name: "beep".into(),
            sample_rate: 48_000,
            channels: 1,
            samples: vec![0.5; 480],
        }
    }

    fn registry_with(clip: &AudioClip) -> (AssetRegistry, ecs::assets::AssetHandle<AudioClip>) {
        let mut registry = AssetRegistry::new();
        let handle = registry.audio.insert(clip.clone());
        (registry, handle)
    }

    fn sound_on(world: &mut World, clip: ecs::assets::AssetHandle<AudioClip>) -> Entity {
        let entity = world.spawn();
        world.insert(
            entity,
            AudioSource {
                clip,
                playing: true,
                ..AudioSource::default()
            },
        );
        entity
    }

    #[test]
    fn silent_engine_reports_not_live() {
        let engine = AudioEngine::silent();
        assert!(!engine.is_live());
        assert_eq!(engine.sound_count(), 0);
    }

    #[test]
    fn play_and_stop_tracks_state() {
        let mut engine = AudioEngine::silent();
        let clip = test_clip();
        let entity = Entity::from_raw(0, 0);
        let handle = engine.play(entity, &clip, PlaybackConfig::default());

        assert!(engine.is_playing(handle));
        assert_eq!(engine.sound_count(), 1);

        engine.stop(handle);
        assert!(!engine.is_playing(handle));
        assert_eq!(engine.sound_count(), 0);
    }

    #[test]
    fn stop_all_clears_everything() {
        let mut engine = AudioEngine::silent();
        let clip = test_clip();
        let entity = Entity::from_raw(0, 0);
        let a = engine.play(entity, &clip, PlaybackConfig::default());
        let b = engine.play(entity, &clip, PlaybackConfig::default());
        assert!(engine.is_playing(a) && engine.is_playing(b));
        engine.stop_all();
        assert_eq!(engine.sound_count(), 0);
        assert!(!engine.is_playing(a));
    }

    #[test]
    fn volume_and_master_volume_compose() {
        let mut engine = AudioEngine::silent();
        let clip = test_clip();
        let entity = Entity::from_raw(0, 0);
        let handle = engine.play(entity, &clip, PlaybackConfig::default());

        engine.set_volume(handle, 0.5);
        assert!((engine.effective_volume(handle).unwrap() - 0.5).abs() < 1e-6);

        engine.set_master_volume(0.5);
        assert!((engine.effective_volume(handle).unwrap() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn pause_and_resume_toggle_state() {
        let mut engine = AudioEngine::silent();
        let clip = test_clip();
        let entity = Entity::from_raw(0, 0);
        let handle = engine.play(entity, &clip, PlaybackConfig::default());

        assert!(!engine.is_paused(handle));
        engine.pause(handle);
        assert!(engine.is_paused(handle));
        // Paused sounds are still "active".
        assert!(engine.is_playing(handle));
        engine.resume(handle);
        assert!(!engine.is_paused(handle));
    }

    #[test]
    fn update_starts_when_playing_turns_on() {
        let mut engine = AudioEngine::silent();
        let (assets, clip) = registry_with(&test_clip());
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            AudioSource {
                clip,
                playing: false,
                ..AudioSource::default()
            },
        );

        engine.update(&world, &assets);
        assert_eq!(engine.sound_count(), 0, "not playing yet");

        world.get_mut::<AudioSource>(entity).unwrap().playing = true;
        engine.update(&world, &assets);

        let handle = engine.handle_for_entity(entity).expect("sound started");
        assert!(engine.is_playing(handle));
        assert_eq!(engine.sound_count(), 1);
    }

    #[test]
    fn update_stops_when_playing_turns_off() {
        let mut engine = AudioEngine::silent();
        let (assets, clip) = registry_with(&test_clip());
        let mut world = World::new();
        let entity = sound_on(&mut world, clip);

        engine.update(&world, &assets);
        assert_eq!(engine.sound_count(), 1);

        world.get_mut::<AudioSource>(entity).unwrap().playing = false;
        engine.update(&world, &assets);
        assert_eq!(engine.sound_count(), 0);
        assert!(engine.handle_for_entity(entity).is_none());
    }

    #[test]
    fn update_stops_when_component_removed() {
        let mut engine = AudioEngine::silent();
        let (assets, clip) = registry_with(&test_clip());
        let mut world = World::new();
        let entity = sound_on(&mut world, clip);

        engine.update(&world, &assets);
        assert_eq!(engine.sound_count(), 1);

        world.remove::<AudioSource>(entity);
        engine.update(&world, &assets);
        assert_eq!(engine.sound_count(), 0);
    }

    #[test]
    fn update_pushes_volume_and_pitch() {
        let mut engine = AudioEngine::silent();
        let (assets, clip) = registry_with(&test_clip());
        let mut world = World::new();
        let entity = sound_on(&mut world, clip);

        engine.update(&world, &assets);
        let handle = engine.handle_for_entity(entity).unwrap();

        {
            let source = world.get_mut::<AudioSource>(entity).unwrap();
            source.volume = 0.25;
            source.pitch = 2.0;
        }
        engine.update(&world, &assets);
        assert!((engine.effective_volume(handle).unwrap() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn update_spatializes_from_global_transform() {
        let mut engine = AudioEngine::silent();
        let (assets, clip) = registry_with(&test_clip());
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            AudioSource {
                clip,
                playing: true,
                spatial: true,
                max_distance: 100.0,
                rolloff: 1.0,
                ..AudioSource::default()
            },
        );
        // 10 units to the right of the listener at the origin.
        world.insert(
            entity,
            GlobalTransform::from_local(&Transform::from_translation(vec3(10.0, 0.0, 0.0))),
        );

        engine.update(&world, &assets);
        let handle = engine.handle_for_entity(entity).unwrap();

        // attenuation = 1 / (1 + 1*10) = 0.0909...
        assert!((engine.sound_attenuation(handle).unwrap() - 1.0 / 11.0).abs() < 1e-5);
        let (left, right) = engine.channel_gains(handle).unwrap();
        assert!(left < 1e-5, "expected full right pan, got left={left}");
        assert!((right - 1.0).abs() < 1e-5);

        // Move behind the max distance: inaudible.
        world.get_mut::<GlobalTransform>(entity).unwrap().matrix =
            Transform::from_translation(vec3(0.0, 0.0, -200.0)).to_matrix();
        engine.update(&world, &assets);
        assert_eq!(engine.sound_attenuation(handle).unwrap(), 0.0);
    }

    #[test]
    fn update_non_spatial_sound_ignores_distance() {
        let mut engine = AudioEngine::silent();
        let (assets, clip) = registry_with(&test_clip());
        let mut world = World::new();
        let entity = sound_on(&mut world, clip);
        world.insert(
            entity,
            GlobalTransform::from_local(&Transform::from_translation(vec3(500.0, 0.0, 0.0))),
        );

        engine.update(&world, &assets);
        let handle = engine.handle_for_entity(entity).unwrap();
        assert_eq!(engine.sound_attenuation(handle).unwrap(), 1.0);
        assert_eq!(engine.channel_gains(handle).unwrap(), (1.0, 1.0));
    }

    #[test]
    fn update_uses_listener_pose() {
        let mut engine = AudioEngine::silent();
        engine.set_listener(
            vec3(100.0, 0.0, 0.0),
            vec3(0.0, 0.0, -1.0),
            vec3(0.0, 1.0, 0.0),
        );
        let (assets, clip) = registry_with(&test_clip());
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            AudioSource {
                clip,
                playing: true,
                spatial: true,
                ..AudioSource::default()
            },
        );
        // The emitter is 5 units right of the listener.
        world.insert(
            entity,
            GlobalTransform::from_local(&Transform::from_translation(vec3(105.0, 0.0, 0.0))),
        );

        engine.update(&world, &assets);
        let handle = engine.handle_for_entity(entity).unwrap();
        let (left, right) = engine.channel_gains(handle).unwrap();
        assert!(left < 1e-5);
        assert!((right - 1.0).abs() < 1e-5);
    }

    #[test]
    fn update_with_missing_clip_does_not_start() {
        let mut engine = AudioEngine::silent();
        let assets = AssetRegistry::new();
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            AudioSource {
                clip: ecs::assets::AssetHandle::NONE,
                playing: true,
                ..AudioSource::default()
            },
        );

        engine.update(&world, &assets);
        assert_eq!(engine.sound_count(), 0);
    }

    #[test]
    fn update_ignores_unknown_clip_handle() {
        let mut engine = AudioEngine::silent();
        let assets = AssetRegistry::new();
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            AudioSource {
                clip: ecs::assets::AssetHandle::from_raw(99),
                playing: true,
                ..AudioSource::default()
            },
        );

        engine.update(&world, &assets);
        assert_eq!(engine.sound_count(), 0);
    }

    #[test]
    fn manual_sounds_are_spatialized_but_not_managed() {
        let mut engine = AudioEngine::silent();
        let clip = test_clip();
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            GlobalTransform::from_local(&Transform::from_translation(vec3(0.0, 0.0, -5.0))),
        );
        let handle = engine.play(
            entity,
            &clip,
            PlaybackConfig {
                spatial: true,
                ..PlaybackConfig::default()
            },
        );

        // No `AudioSource` on the entity, yet the manual sound survives update.
        engine.update(&world, &AssetRegistry::new());
        assert!(engine.is_playing(handle));
        assert!(engine.channel_gains(handle).is_some());
    }

    #[test]
    fn play_spatial_in_silent_engine_tracks_gains() {
        let mut engine = AudioEngine::silent();
        let clip = test_clip();
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            GlobalTransform::from_local(&Transform::from_translation(vec3(-8.0, 0.0, 0.0))),
        );
        let handle = engine.play(
            entity,
            &clip,
            PlaybackConfig {
                spatial: true,
                ..PlaybackConfig::default()
            },
        );
        engine.update(&world, &AssetRegistry::new());
        let (left, right) = engine.channel_gains(handle).unwrap();
        assert!((left - 1.0).abs() < 1e-5, "expected full left, got {left}");
        assert!(right < 1e-5);
    }

    #[test]
    fn assets_store_round_trip() {
        let mut audio: Assets<AudioClip> = Assets::new();
        let handle = audio.insert(test_clip());
        assert_eq!(audio.get(handle).unwrap().name, "beep");
    }
}
