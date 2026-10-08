//! Pure spatial-audio math.
//!
//! Everything in this module is deterministic and free of any audio-device
//! dependency, which makes it the primary thing the crate tests. The engine in
//! [`crate::AudioEngine`] feeds [`GlobalTransform`](ecs::components::GlobalTransform)
//! positions and a listener pose through these functions to produce per-sound
//! gains.
//!
//! The listener convention matches a right-handed camera: `forward` points into
//! the scene and `up` is the world up vector, so `right = normalize(forward × up)`.
//! An emitter on the `+right` side pans to the right channel.

use ecs::assets::AudioClip;
use math::{Vec3, cross, dot, length};

/// One half of the equal-power center gain (`1/√2 ≈ 0.7071`).
const CENTER: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Distance-based attenuation using an inverse-distance rolloff.
///
/// The gain is `1 / (1 + rolloff * distance)` clamped to `[0, 1]`, with the
/// following boundary behaviour:
///
/// * `distance <= 0` (at the listener) yields `1.0`.
/// * `distance >= max_distance` yields `0.0` (the source is inaudible).
/// * `max_distance <= 0` yields `0.0` (nothing can be heard).
/// * a negative `rolloff` is treated as `0.0` (no falloff until the cutoff).
///
/// `rolloff` is expressed per world unit; larger values fall off faster.
pub fn attenuation(distance: f32, max_distance: f32, rolloff: f32) -> f32 {
    if max_distance <= 0.0 {
        return 0.0;
    }
    if distance <= 0.0 {
        return 1.0;
    }
    if distance >= max_distance {
        return 0.0;
    }
    let rolloff = if rolloff < 0.0 { 0.0 } else { rolloff };
    (1.0 / (1.0 + rolloff * distance)).clamp(0.0, 1.0)
}

/// Equal-power stereo pan gains for an emitter relative to a listener pose.
///
/// `pan` is the normalized projection of the emitter direction onto the
/// listener's right vector (`right = normalize(forward × up)`), in `[-1, 1]`.
/// The gains satisfy `left² + right² == 1`, so a sound stays at a constant
/// perceived loudness as it moves across the field.
///
/// Degenerate inputs (an emitter exactly at the listener, or a zero-length
/// `forward`/`up` basis) fall back to the centered equal-power gain
/// `(1/√2, 1/√2)`.
pub fn pan_gains(
    emitter: Vec3,
    listener_pos: Vec3,
    listener_forward: Vec3,
    listener_up: Vec3,
) -> (f32, f32) {
    let to_emitter = emitter - listener_pos;
    let distance = length(to_emitter);
    if distance <= f32::EPSILON {
        return (CENTER, CENTER);
    }

    let forward_len = length(listener_forward);
    let up_len = length(listener_up);
    if forward_len <= f32::EPSILON || up_len <= f32::EPSILON {
        return (CENTER, CENTER);
    }

    let right = cross(listener_forward / forward_len, listener_up / up_len);
    let right_len = length(right);
    if right_len <= f32::EPSILON {
        // `forward` and `up` are parallel: no well-defined left/right axis.
        return (CENTER, CENTER);
    }

    let pan = dot(to_emitter / distance, right / right_len).clamp(-1.0, 1.0);
    let left = ((1.0 - pan) * 0.5).sqrt();
    let right_gain = ((1.0 + pan) * 0.5).sqrt();
    (left, right_gain)
}

/// The result of normalizing a clip's channel layout to mono or stereo.
#[derive(Debug, Clone, PartialEq)]
pub struct MonoOrStereo {
    channels: u16,
    samples: Vec<f32>,
}

impl MonoOrStereo {
    /// The resulting channel count (`0`, `1`, or `2`).
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// The interleaved samples.
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// `true` if the result is mono.
    pub fn is_mono(&self) -> bool {
        self.channels == 1
    }

    /// `true` if the result is stereo.
    pub fn is_stereo(&self) -> bool {
        self.channels == 2
    }

    /// The number of samples (not frames).
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// `true` if there are no samples.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Borrow the samples as an `f32` iterator.
    pub fn iter(&self) -> std::slice::Iter<'_, f32> {
        self.samples.iter()
    }

    /// Consume the wrapper and return the raw sample vector.
    pub fn into_samples(self) -> Vec<f32> {
        self.samples
    }
}

impl IntoIterator for MonoOrStereo {
    type Item = f32;
    type IntoIter = std::vec::IntoIter<f32>;

    fn into_iter(self) -> Self::IntoIter {
        self.samples.into_iter()
    }
}

/// Normalize a clip's samples to mono or stereo.
///
/// * A mono (`1`) or stereo (`2`) clip is passed through unchanged.
/// * Any other channel count (`0`, or `> 2`) is folded down to mono by
///   averaging each frame's channels.
///
/// Trailing samples that do not form a complete frame are ignored.
pub fn to_mono_or_stereo(clip: &AudioClip) -> MonoOrStereo {
    let channels = clip.channels as usize;
    match clip.channels {
        1 | 2 => MonoOrStereo {
            channels: clip.channels,
            samples: clip.samples.clone(),
        },
        0 => MonoOrStereo {
            channels: 0,
            samples: Vec::new(),
        },
        _ => {
            let frames = clip.samples.len() / channels;
            let mut samples = Vec::with_capacity(frames);
            for frame in 0..frames {
                let base = frame * channels;
                let sum: f32 = clip.samples[base..base + channels].iter().sum();
                samples.push(sum / channels as f32);
            }
            MonoOrStereo {
                channels: 1,
                samples,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use math::vec3;

    fn clip(channels: u16, samples: Vec<f32>) -> AudioClip {
        AudioClip {
            name: "test".into(),
            sample_rate: 48_000,
            channels,
            samples,
        }
    }

    #[test]
    fn attenuation_at_listener_is_full() {
        assert_eq!(attenuation(0.0, 50.0, 1.0), 1.0);
        assert_eq!(attenuation(-3.0, 50.0, 1.0), 1.0);
    }

    #[test]
    fn attenuation_is_zero_at_and_beyond_max() {
        assert_eq!(attenuation(50.0, 50.0, 1.0), 0.0);
        assert_eq!(attenuation(75.0, 50.0, 1.0), 0.0);
        assert_eq!(attenuation(10.0, 0.0, 1.0), 0.0);
        assert_eq!(attenuation(10.0, -5.0, 1.0), 0.0);
    }

    #[test]
    fn attenuation_follows_inverse_distance() {
        // 1 / (1 + 1 * 10) == 1/11
        assert!((attenuation(10.0, 100.0, 1.0) - 1.0 / 11.0).abs() < 1e-6);
    }

    #[test]
    fn attenuation_is_monotonic_decreasing() {
        let mut previous = attenuation(0.0, 100.0, 2.0);
        for step in 1..=100 {
            let current = attenuation(step as f32, 100.0, 2.0);
            assert!(current <= previous, "gain rose at distance {step}");
            previous = current;
        }
    }

    #[test]
    fn zero_rolloff_holds_full_gain_until_cutoff() {
        assert_eq!(attenuation(25.0, 50.0, 0.0), 1.0);
        assert_eq!(attenuation(49.9, 50.0, 0.0), 1.0);
        assert_eq!(attenuation(50.0, 50.0, 0.0), 0.0);
    }

    #[test]
    fn negative_rolloff_is_clamped() {
        assert_eq!(attenuation(10.0, 50.0, -3.0), 1.0);
    }

    #[test]
    fn pan_center_when_emitter_at_listener() {
        let (l, r) = pan_gains(
            vec3(0.0, 0.0, 0.0),
            vec3(0.0, 0.0, 0.0),
            vec3(0.0, 0.0, -1.0),
            vec3(0.0, 1.0, 0.0),
        );
        assert_eq!((l, r), (CENTER, CENTER));
    }

    #[test]
    fn pan_right_when_emitter_to_the_right() {
        // forward = -Z, up = +Y  =>  right = +X
        let (l, r) = pan_gains(
            vec3(10.0, 0.0, 0.0),
            vec3(0.0, 0.0, 0.0),
            vec3(0.0, 0.0, -1.0),
            vec3(0.0, 1.0, 0.0),
        );
        assert!(l < 1e-6, "left gain should be ~0, got {l}");
        assert!((r - 1.0).abs() < 1e-6, "right gain should be ~1, got {r}");
    }

    #[test]
    fn pan_left_when_emitter_to_the_left() {
        let (l, r) = pan_gains(
            vec3(-10.0, 0.0, 0.0),
            vec3(0.0, 0.0, 0.0),
            vec3(0.0, 0.0, -1.0),
            vec3(0.0, 1.0, 0.0),
        );
        assert!((l - 1.0).abs() < 1e-6, "left gain should be ~1, got {l}");
        assert!(r < 1e-6, "right gain should be ~0, got {r}");
    }

    #[test]
    fn pan_is_centered_directly_ahead_and_behind() {
        let ahead = pan_gains(
            vec3(0.0, 0.0, -10.0),
            vec3(0.0, 0.0, 0.0),
            vec3(0.0, 0.0, -1.0),
            vec3(0.0, 1.0, 0.0),
        );
        let behind = pan_gains(
            vec3(0.0, 0.0, 10.0),
            vec3(0.0, 0.0, 0.0),
            vec3(0.0, 0.0, -1.0),
            vec3(0.0, 1.0, 0.0),
        );
        assert_eq!(ahead, (CENTER, CENTER));
        assert_eq!(behind, (CENTER, CENTER));
    }

    #[test]
    fn pan_preserves_energy() {
        for x in [-5.0f32, -2.0, -0.5, 0.0, 0.5, 2.0, 5.0] {
            let (l, r) = pan_gains(
                vec3(x, 0.0, 0.0),
                vec3(0.0, 0.0, 0.0),
                vec3(0.0, 0.0, -1.0),
                vec3(0.0, 1.0, 0.0),
            );
            assert!((l * l + r * r - 1.0).abs() < 1e-5, "energy drift at x={x}");
        }
    }

    #[test]
    fn pan_handles_degenerate_basis() {
        let zero = vec3(0.0, 0.0, 0.0);
        let emitter = vec3(5.0, 0.0, 0.0);
        assert_eq!(
            pan_gains(emitter, zero, zero, vec3(0.0, 1.0, 0.0)),
            (CENTER, CENTER)
        );
        // forward parallel to up => no right axis
        assert_eq!(
            pan_gains(emitter, zero, vec3(0.0, 1.0, 0.0), vec3(0.0, 1.0, 0.0)),
            (CENTER, CENTER)
        );
    }

    #[test]
    fn encode_mono_passthrough() {
        let result = to_mono_or_stereo(&clip(1, vec![0.1, 0.2, 0.3]));
        assert_eq!(result.channels(), 1);
        assert!(result.is_mono());
        assert_eq!(result.samples(), &[0.1, 0.2, 0.3]);
    }

    #[test]
    fn encode_stereo_passthrough() {
        let result = to_mono_or_stereo(&clip(2, vec![0.1, 0.9, 0.2, 0.8]));
        assert_eq!(result.channels(), 2);
        assert!(result.is_stereo());
        assert_eq!(result.samples(), &[0.1, 0.9, 0.2, 0.8]);
    }

    #[test]
    fn encode_multichannel_folds_to_mono() {
        let result = to_mono_or_stereo(&clip(4, vec![1.0, 0.0, 0.0, 0.0, 0.0, 2.0, 2.0, 0.0]));
        assert_eq!(result.channels(), 1);
        assert_eq!(result.samples(), &[0.25, 1.0]);
    }

    #[test]
    fn encode_zero_channels_is_empty() {
        let result = to_mono_or_stereo(&clip(0, vec![1.0, 2.0]));
        assert_eq!(result.channels(), 0);
        assert!(result.is_empty());
        assert_eq!(result.into_iter().count(), 0);
    }

    #[test]
    fn mono_or_stereo_iterates_as_f32() {
        let result = to_mono_or_stereo(&clip(1, vec![0.5, -0.5]));
        let collected: Vec<f32> = result.into_iter().collect();
        assert_eq!(collected, vec![0.5, -0.5]);
    }
}
