/*!
 * Audio, on the engine's [`AudioEngine`] — rodio on native, the browser's
 * WebAudio on `wasm32`. Using one engine for both targets is what lets the web
 * demo have sound at all: the engine resumes the `AudioContext` on the first
 * user gesture, as the autoplay policy requires.
 */

use std::collections::HashMap;

use noctis::prelude::{
    AudioClip, AudioEngine, Entity, PlaybackConfig, SoundHandle, load_clip_bytes,
};

/// A one-shot sound effect.
pub enum SoundEffect {
    Jump,
    Coin,
    Dead,
}

const MUSIC_VOLUME: f32 = 1.0;
const SFX_VOLUME: f32 = 0.55;

/// The songs, by the name a level's metadata uses.
const SONGS: &[(&str, &[u8])] = &[
    (
        "reverie-amidst-ruins",
        include_bytes!("../../assets/songs/reverie-amidst-ruins.ogg"),
    ),
    (
        "echoes-of-solace",
        include_bytes!("../../assets/songs/echoes-of-solace.ogg"),
    ),
    ("colors", include_bytes!("../../assets/songs/colors.ogg")),
    (
        "the-final-storm",
        include_bytes!("../../assets/songs/the-final-storm.ogg"),
    ),
];

// The engine binds each sound to an entity for spatialization; these are inert
// sentinels, since the game's sounds are non-spatial and it never calls
// `AudioEngine::update` (which reconciles `AudioSource` components).
fn music_entity() -> Entity {
    Entity::from_raw(0, 0)
}
fn effect_entity() -> Entity {
    Entity::from_raw(1, 0)
}

/// The game's audio: a looping music track plus one-shot effects.
pub struct Audio {
    engine: AudioEngine,
    songs: HashMap<String, AudioClip>,
    jump: AudioClip,
    coin: AudioClip,
    dead: AudioClip,
    music: Option<SoundHandle>,
    effects: Vec<SoundHandle>,
    volume: u8,
    muted: bool,
}

impl Audio {
    /// Open the default output (or fall back to a silent engine) and start
    /// `initial_music`.
    pub fn new(initial_music: &str, volume: u8) -> anyhow::Result<Self> {
        let engine = AudioEngine::new().unwrap_or_else(|_| AudioEngine::silent());

        let mut songs = HashMap::new();
        for (name, bytes) in SONGS {
            songs.insert((*name).to_string(), load_clip_bytes(name, bytes)?);
        }

        let mut audio = Self {
            engine,
            songs,
            jump: load_clip_bytes("jump", include_bytes!("../../assets/sfx/jump.wav"))?,
            coin: load_clip_bytes("coin", include_bytes!("../../assets/sfx/coin.wav"))?,
            dead: load_clip_bytes("dead", include_bytes!("../../assets/sfx/dead.wav"))?,
            music: None,
            effects: Vec::new(),
            volume: volume.min(100),
            muted: false,
        };

        audio.apply_volume();
        audio.play_music(initial_music)?;
        Ok(audio)
    }

    /// Switch the looping music track, stopping the previous one.
    pub fn play_music(&mut self, name: &str) -> anyhow::Result<()> {
        self.stop_music();
        let Some(clip) = self.songs.get(name) else {
            anyhow::bail!("unknown music track: {name}");
        };
        let config = PlaybackConfig {
            volume: MUSIC_VOLUME,
            pitch: 1.0,
            looped: true,
            spatial: false,
        };
        self.music = Some(self.engine.play(music_entity(), clip, config));
        Ok(())
    }

    /// Play a one-shot sound effect.
    pub fn play_effect(&mut self, effect: SoundEffect) {
        let clip = match effect {
            SoundEffect::Jump => &self.jump,
            SoundEffect::Coin => &self.coin,
            SoundEffect::Dead => &self.dead,
        };
        let config = PlaybackConfig {
            volume: SFX_VOLUME,
            pitch: 1.0,
            looped: false,
            spatial: false,
        };
        let handle = self.engine.play(effect_entity(), clip, config);
        self.effects.push(handle);
    }

    /// Forget the one-shot sounds that have finished playing.
    pub fn poll(&mut self) {
        let mut index = 0;
        while index < self.effects.len() {
            if self.engine.is_playing(self.effects[index]) {
                index += 1;
            } else {
                let handle = self.effects.swap_remove(index);
                self.engine.stop(handle);
            }
        }
    }

    /// Toggle all game audio, returning whether it is now muted.
    pub fn toggle_mute(&mut self) -> bool {
        self.muted = !self.muted;
        self.apply_volume();
        self.muted
    }

    /// Adjust master volume, returning its new 0-100 value.
    pub fn adjust_volume(&mut self, amount: i8) -> u8 {
        self.volume = (self.volume as i16 + amount as i16).clamp(0, 100) as u8;
        self.apply_volume();
        self.volume
    }

    fn apply_volume(&mut self) {
        let scale = if self.muted {
            0.0
        } else {
            self.volume as f32 / 100.0
        };
        self.engine.set_master_volume(scale);
    }

    fn stop_music(&mut self) {
        if let Some(handle) = self.music.take() {
            self.engine.stop(handle);
        }
    }
}
