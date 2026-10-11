//! The WebAudio output backend for `wasm32`.
//!
//! rodio reaches the browser through cpal's WebAudio backend, but the
//! `AudioContext` it opens starts *suspended* (the browser autoplay policy) and
//! cpal never hands the context back to resume it, so a rodio sink is always
//! silent on the web. This backend owns the context itself: it resumes on the
//! first user gesture and plays each clip from an `AudioBuffer` through a gain
//! (volume) and, for a spatial sound, a stereo panner that reproduces the gains
//! [`pan_gains`](crate::spatial::pan_gains) computes.
//!
//! The backend mirrors the rodio sink's behaviour so [`AudioEngine`] is
//! target-agnostic: the same volume, pitch, pan, pause/resume, stop, and
//! "has finished" bookkeeping, only realised with browser nodes.

use std::cell::RefCell;
use std::rc::Rc;

use ecs::assets::AudioClip;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use web_sys::{
    AudioBuffer, AudioBufferSourceNode, AudioContext, AudioScheduledSourceNode, GainNode,
    StereoPannerNode,
};

use crate::engine::{AudioError, PlaybackConfig};
use crate::spatial::{pan_from_gains, to_mono_or_stereo};

/// The shared WebAudio output: one `AudioContext` for the engine.
pub struct WebBackend {
    context: AudioContext,
}

impl WebBackend {
    /// Open an `AudioContext` and arrange to resume it on the first gesture.
    ///
    /// Creating a context before a user gesture is allowed; it simply starts
    /// suspended. [`resume_on_gesture`] resumes it when the user first interacts,
    /// which is what the autoplay policy requires before any sound is heard.
    pub fn new() -> Result<Self, AudioError> {
        let context =
            AudioContext::new().map_err(|error| AudioError::DeviceUnavailable(describe(&error)))?;
        resume_on_gesture(&context);
        Ok(Self { context })
    }

    /// Start `clip`, returning its node graph (or `None` for an empty clip).
    pub fn play(
        &self,
        clip: &AudioClip,
        config: &PlaybackConfig,
        initial_gains: (f32, f32),
    ) -> Option<WebSound> {
        let buffer = self.build_buffer(clip)?;
        WebSound::start(self.context.clone(), buffer, config, initial_gains)
    }

    /// Build an `AudioBuffer` from a clip's interleaved samples.
    ///
    /// The clip is folded to mono or stereo first (WebAudio's panner and gain
    /// graph assume at most two channels). Degenerate clips — no channels, a zero
    /// sample rate, or no frames — yield `None`, matching the rodio path.
    fn build_buffer(&self, clip: &AudioClip) -> Option<AudioBuffer> {
        if clip.sample_rate == 0 {
            return None;
        }
        let pcm = to_mono_or_stereo(clip);
        let channels = pcm.channels() as usize;
        if channels == 0 {
            return None;
        }
        let samples = pcm.samples();
        let frames = samples.len() / channels;
        if frames == 0 {
            return None;
        }
        let buffer = self
            .context
            .create_buffer(channels as u32, frames as u32, clip.sample_rate as f32)
            .ok()?;
        for channel in 0..channels {
            let plane: Vec<f32> = (0..frames)
                .map(|frame| samples[frame * channels + channel])
                .collect();
            buffer.copy_to_channel(&plane, channel as i32).ok()?;
        }
        Some(buffer)
    }
}

/// A live WebAudio sound: a buffer source through an optional panner and a gain.
pub struct WebSound {
    state: Rc<RefCell<State>>,
}

/// The mutable node graph and playback clock of a sound.
struct State {
    context: AudioContext,
    buffer: AudioBuffer,
    gain: GainNode,
    panner: Option<StereoPannerNode>,
    source: Option<AudioBufferSourceNode>,
    looped: bool,
    pitch: f32,
    duration: f64,
    /// The playhead when the sound was last started, in seconds.
    offset: f64,
    /// The context time the current source started at.
    started_at: f64,
    paused: bool,
    stopped: bool,
}

impl WebSound {
    fn start(
        context: AudioContext,
        buffer: AudioBuffer,
        config: &PlaybackConfig,
        gains: (f32, f32),
    ) -> Option<Self> {
        let gain = context.create_gain().ok()?;
        // The engine sets the real volume immediately after `play`; start silent
        // so nothing blips at an unset level.
        gain.gain().set_value(0.0);

        let panner = if config.spatial {
            let panner = context.create_stereo_panner().ok()?;
            panner.pan().set_value(pan_from_gains(gains.0, gains.1));
            panner.connect_with_audio_node(&gain).ok()?;
            Some(panner)
        } else {
            None
        };
        gain.connect_with_audio_node(&context.destination()).ok()?;

        let duration = buffer.duration();
        let state = Rc::new(RefCell::new(State {
            context,
            buffer,
            gain,
            panner,
            source: None,
            looped: config.looped,
            pitch: config.pitch,
            duration,
            offset: 0.0,
            started_at: 0.0,
            paused: false,
            stopped: false,
        }));
        {
            let mut state = state.borrow_mut();
            let source = start_source(&mut state, 0.0)?;
            state.source = Some(source);
        }
        Some(Self { state })
    }

    pub fn set_volume(&self, volume: f32) {
        self.state.borrow().gain.gain().set_value(volume.max(0.0));
    }

    pub fn set_speed(&self, speed: f32) {
        let mut state = self.state.borrow_mut();
        state.pitch = speed;
        if let Some(source) = &state.source {
            source.playback_rate().set_value(speed);
        }
    }

    pub fn set_gains(&self, left: f32, right: f32) {
        if let Some(panner) = &self.state.borrow().panner {
            panner.pan().set_value(pan_from_gains(left, right));
        }
    }

    pub fn pause(&self) {
        let mut state = self.state.borrow_mut();
        if state.paused || state.stopped {
            return;
        }
        state.offset = playhead(&state);
        if let Some(source) = state.source.take() {
            stop_source(&source);
        }
        state.paused = true;
    }

    pub fn resume(&self) {
        let mut state = self.state.borrow_mut();
        if !state.paused || state.stopped {
            return;
        }
        let offset = state.offset;
        if let Some(source) = start_source(&mut state, offset) {
            state.source = Some(source);
            state.paused = false;
        }
    }

    pub fn stop(&self) {
        let mut state = self.state.borrow_mut();
        if let Some(source) = state.source.take() {
            stop_source(&source);
        }
        state.stopped = true;
    }

    /// `true` once a one-shot has played out; a looping sound is never empty
    /// until it is stopped.
    pub fn is_empty(&self) -> bool {
        let state = self.state.borrow();
        if state.stopped {
            return true;
        }
        if state.looped {
            return false;
        }
        playhead(&state) >= state.duration
    }
}

/// Create a source node for `buffer`, wire it into the graph, and start it at
/// `offset`; records the start time on `state`.
fn start_source(state: &mut State, offset: f64) -> Option<AudioBufferSourceNode> {
    let source = state.context.create_buffer_source().ok()?;
    source.set_buffer(Some(&state.buffer));
    source.set_loop(state.looped);
    source.playback_rate().set_value(state.pitch);
    let connected = match &state.panner {
        Some(panner) => source.connect_with_audio_node(panner).is_ok(),
        None => source.connect_with_audio_node(&state.gain).is_ok(),
    };
    if !connected {
        return None;
    }
    source.start_with_when_and_grain_offset(0.0, offset).ok()?;
    state.started_at = state.context.current_time();
    Some(source)
}

/// Stop a source through the non-deprecated parent method.
fn stop_source(source: &AudioBufferSourceNode) {
    let _ = source.unchecked_ref::<AudioScheduledSourceNode>().stop();
}

/// The playhead position in seconds (wrapped within one loop for a looping
/// sound); frozen while paused.
fn playhead(state: &State) -> f64 {
    if state.paused {
        return state.offset;
    }
    let elapsed = state.offset + (state.context.current_time() - state.started_at);
    if state.looped && state.duration > 0.0 {
        elapsed % state.duration
    } else {
        elapsed
    }
}

/// Resume `context` on the first user gesture, as the autoplay policy requires.
///
/// The listeners are `forget`ten: there is a fixed handful, and they must outlive
/// the page. Resuming an already-running context is a no-op, so leaving them in
/// place is harmless.
fn resume_on_gesture(context: &AudioContext) {
    let Some(window) = web_sys::window() else {
        return;
    };
    for kind in ["pointerdown", "keydown", "touchstart"] {
        let context = context.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            let _ = context.resume();
        });
        let _ = window.add_event_listener_with_callback(kind, closure.as_ref().unchecked_ref());
        closure.forget();
    }
}

/// A human-readable description of a JS error value.
fn describe(error: &JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}
