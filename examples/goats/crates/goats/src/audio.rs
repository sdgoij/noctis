//! The game's audio, on the engine's [`AudioEngine`] (rodio on native, the browser's
//! WebAudio on `wasm32`) — ported from the original's `audio.js`. The clips are the
//! original's own `sfx/` samples, embedded.
//!
//! The simulation only *asks* for a sound ([`crate::Sound`]); this module is the
//! plugin's, so a silent device or a headless host never pays for one.

use noctis::prelude::*;

const MUSIC_VOLUME: f32 = 0.20;
const SFX_VOLUME: f32 = 0.65;
const RAIN_VOLUME: f32 = 0.9;
const WIND_VOLUME: f32 = 0.8;
const BLAST_VOLUME: f32 = 1.0;
/// Metres over which a bang loses half of what is left past the blast's radius.
const BLAST_FALLOFF: f32 = 24.0;
const DEBRIS_VOLUME: f32 = 0.55;
const DEBRIS_DELAY: f32 = 0.42;
const DEBRIS_SPREAD: f32 = 0.34;
/// The blast's own reach, in metres (`hazards::BLAST_RADIUS`): inside it a bang is
/// heard at full volume.
const BLAST_RADIUS: f32 = 3.2;

/// The tracks, embedded (`.notes/goats.md`): the background music, the two weather
/// beds, and the one-shot slots.
const MUSIC: &[u8] = include_bytes!("../content/sfx/jkstudios-rage-2-187959.mp3");
const RAIN: &[u8] = include_bytes!("../content/sfx/WE Heavy Outside Rain 1.ogg");
const WIND: &[u8] = include_bytes!("../content/sfx/WE Light Wind Whistle 1.ogg");
const BLEATS: &[&[u8]] = &[
    include_bytes!("../content/sfx/dragon-studio-goat-baa-390303.mp3"),
    include_bytes!("../content/sfx/freesound_community-happy-goat-6463.mp3"),
];
const BLASTS: &[&[u8]] = &[
    include_bytes!("../content/sfx/soundreality-explosion-fx-343683.mp3"),
    include_bytes!("../content/sfx/freesound_community-medium-explosion-40472.mp3"),
];
const DEBRIS: &[&[u8]] = &[include_bytes!(
    "../content/sfx/freesound_community-stones-falling-6375.mp3"
)];

/// A bang's dirt coming down a beat later.
struct Pending {
    left: f32,
    clip: AudioClip,
    gain: f32,
}

/// The game's audio: a looping track, two weather beds, and the one-shot effects.
pub struct Audio {
    engine: AudioEngine,
    rain: Option<SoundHandle>,
    wind: Option<SoundHandle>,
    bleats: Vec<AudioClip>,
    blasts: Vec<AudioClip>,
    debris: Vec<AudioClip>,
    pending: Vec<Pending>,
    rng: u32,
    muted: bool,
    /// The sfx volume, `0..=100` (`SETTINGS.sfx`).
    volume: u8,
}

impl Audio {
    /// Open the default output (or fall back to a silent engine), load the clips, and
    /// start the track and the two beds at silence (`makeAudio`).
    pub fn new() -> Audio {
        let engine = AudioEngine::new().unwrap_or_else(|_| AudioEngine::silent());
        let mut audio = Audio {
            engine,
            rain: None,
            wind: None,
            bleats: Vec::new(),
            blasts: Vec::new(),
            debris: Vec::new(),
            pending: Vec::new(),
            rng: 0x02f6_e2b1,
            muted: false,
            volume: 100,
        };
        // The beds start at silence and swell with the weather; keeping them playing
        // avoids a start/stop click at every transition.
        audio.rain = audio.bed(RAIN);
        audio.wind = audio.bed(WIND);
        audio.bleats = load_slot(BLEATS);
        audio.blasts = load_slot(BLASTS);
        audio.debris = load_slot(DEBRIS);
        if let Ok(clip) = load_clip_bytes("music", MUSIC) {
            let handle = audio.engine.play(
                music_entity(),
                &clip,
                PlaybackConfig {
                    volume: MUSIC_VOLUME,
                    pitch: 1.0,
                    looped: true,
                    spatial: false,
                },
            );
            audio.engine.set_volume(handle, MUSIC_VOLUME);
        }
        audio.apply_volume();
        audio
    }

    /// A looping bed, started at silence.
    fn bed(&mut self, bytes: &[u8]) -> Option<SoundHandle> {
        let clip = load_clip_bytes("bed", bytes).ok()?;
        let handle = self.engine.play(
            music_entity(),
            &clip,
            PlaybackConfig {
                volume: 0.0,
                pitch: 1.0,
                looped: true,
                spatial: false,
            },
        );
        self.engine.set_volume(handle, 0.0);
        Some(handle)
    }

    /// The sfx gain, `0..=1` (`sfxGain`), zero when muted.
    fn gain(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            SFX_VOLUME * self.volume as f32 / 100.0
        }
    }

    /// A little pitch variation, so repeats do not sound identical.
    fn jitter(&mut self) -> f32 {
        let mut s = self.rng;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.rng = s;
        0.9 + (s as f32 / 4_294_967_296.0) * 0.25
    }

    /// Play the sound the simulation asked for (`playBleat` / `playBlast`).
    pub fn play_sound(&mut self, sound: crate::Sound, goat: (f32, f32)) {
        match sound {
            crate::Sound::Bleat { gain } => {
                let Some(clip) = pick(&mut self.rng, &self.bleats) else {
                    return;
                };
                let volume = self.gain() * gain;
                let pitch = self.jitter();
                self.engine.play(
                    effect_entity(),
                    &clip,
                    PlaybackConfig {
                        volume,
                        pitch,
                        looped: false,
                        spatial: false,
                    },
                );
            }
            crate::Sound::Blast { x, z } => {
                let att = blast_attenuation(x, z, goat);
                let Some(clip) = pick(&mut self.rng, &self.blasts) else {
                    return;
                };
                let volume = self.gain() * BLAST_VOLUME * att;
                let pitch = self.jitter();
                self.engine.play(
                    effect_entity(),
                    &clip,
                    PlaybackConfig {
                        volume,
                        pitch,
                        looped: false,
                        spatial: false,
                    },
                );
                // ...and its grit, later (`updateAudio` plays it).
                if let Some(clip) = pick(&mut self.rng, &self.debris) {
                    self.pending.push(Pending {
                        left: DEBRIS_DELAY + att * DEBRIS_SPREAD,
                        clip,
                        gain: att,
                    });
                }
            }
        }
    }

    /// Tie the beds to the weather and play whatever grit is due (`updateAudio`).
    pub fn update(&mut self, dt: f32, rain: f32, wind_sway: f32) {
        let music = if self.muted {
            0.0
        } else {
            MUSIC_VOLUME * self.volume as f32 / 100.0
        };
        if let Some(handle) = self.rain {
            self.engine.set_volume(handle, music * RAIN_VOLUME * rain);
        }
        if let Some(handle) = self.wind {
            self.engine
                .set_volume(handle, music * WIND_VOLUME * wind_sway);
        }

        let mut i = self.pending.len();
        while i > 0 {
            i -= 1;
            self.pending[i].left -= dt;
            if self.pending[i].left > 0.0 {
                continue;
            }
            let fall = self.pending.remove(i);
            if self.muted {
                continue;
            }
            let volume = self.gain() * DEBRIS_VOLUME * fall.gain;
            let pitch = self.jitter();
            self.engine.play(
                effect_entity(),
                &fall.clip,
                PlaybackConfig {
                    volume,
                    pitch,
                    looped: false,
                    spatial: false,
                },
            );
        }
    }

    /// Toggle the master mute, returning whether it is now muted.
    pub fn toggle_mute(&mut self) -> bool {
        self.muted = !self.muted;
        self.apply_volume();
        self.muted
    }

    /// Adjust the sfx volume, returning its new `0..=100` value.
    pub fn adjust_volume(&mut self, amount: i8) -> u8 {
        self.volume = (self.volume as i16 + amount as i16).clamp(0, 100) as u8;
        self.apply_volume();
        self.volume
    }

    fn apply_volume(&mut self) {
        // The engine's own master mute, so the track and the beds (set by
        // `set_volume`) follow too.
        self.engine.set_master_volume(self.gain());
    }
}

impl Default for Audio {
    fn default() -> Self {
        Self::new()
    }
}

/// A bang's loudness from where the goat is (`blastAttenuation`): full inside the
/// blast, half at twenty-odd metres, tending to silence.
fn blast_attenuation(x: f32, z: f32, goat: (f32, f32)) -> f32 {
    let dx = x - goat.0;
    let dz = z - goat.1;
    let d = ((dx * dx + dz * dz).sqrt() - BLAST_RADIUS).max(0.0);
    1.0 / (1.0 + d / BLAST_FALLOFF)
}

/// A named clip from a byte slot, dropping any that fail to decode.
fn load_slot(slot: &[&[u8]]) -> Vec<AudioClip> {
    slot.iter()
        .enumerate()
        .filter_map(|(i, bytes)| load_clip_bytes(&format!("sfx{i}"), bytes).ok())
        .collect()
}

/// A random clip from `slot`, advancing the audio RNG (`sfxPick`). A free function so
/// it can borrow the RNG and the slot — separate fields of `Audio` — at once.
fn pick(rng: &mut u32, slot: &[AudioClip]) -> Option<AudioClip> {
    if slot.is_empty() {
        return None;
    }
    let mut s = *rng;
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    *rng = s;
    Some(slot[(s as usize) % slot.len()].clone())
}

/// The engine binds each sound to an entity for spatialization; these are inert
/// sentinels, since the game's sounds are non-spatial and it never calls
/// `AudioEngine::update`.
fn music_entity() -> Entity {
    Entity::from_raw(0, 0)
}
fn effect_entity() -> Entity {
    Entity::from_raw(1, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bang_falls_off_with_distance() {
        let goat = (0.0, 0.0);
        // Inside the blast, full volume.
        assert!((blast_attenuation(0.0, 0.0, goat) - 1.0).abs() < 1e-6);
        // Far away, quiet but never silent.
        let far = blast_attenuation(100.0, 0.0, goat);
        assert!(far > 0.0 && far < 0.3, "a distant bang is faint: {far}");
        // Monotone in distance.
        assert!(blast_attenuation(10.0, 0.0, goat) > blast_attenuation(30.0, 0.0, goat));
    }
}
