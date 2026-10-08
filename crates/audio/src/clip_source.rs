//! A minimal `f32` PCM [`Source`] built from an [`AudioClip`].
//!
//! The engine feeds clips to rodio through this source instead of the decoder
//! stack, so playback works with the engine's in-memory `f32` sample buffers and
//! needs no file I/O. Looping is handled here (rather than with rodio's
//! `repeat_infinite`) so a single concrete source type covers both looped and
//! one-shot playback.

use std::time::Duration;

use ecs::assets::AudioClip;
use rodio::{ChannelCount, SampleRate, Source};

/// A one-shot or looping source over an interleaved `f32` sample buffer.
///
/// `ClipSource` is `Send` and cheaply constructed from a borrowed [`AudioClip`].
/// When `looped` is set the iterator never reports exhaustion and
/// [`Source::current_span_len`] returns `None` (an infinite span).
#[derive(Debug, Clone)]
pub struct ClipSource {
    samples: Vec<f32>,
    channels: ChannelCount,
    sample_rate: SampleRate,
    looped: bool,
    pos: usize,
}

impl ClipSource {
    /// Build a source from a clip.
    ///
    /// Returns `None` if the clip has zero channels or a zero sample rate, which
    /// cannot be represented as a valid rodio source.
    pub fn new(clip: &AudioClip, looped: bool) -> Option<Self> {
        let channels = ChannelCount::new(clip.channels)?;
        let sample_rate = SampleRate::new(clip.sample_rate)?;
        Some(Self {
            samples: clip.samples.clone(),
            channels,
            sample_rate,
            looped,
            pos: 0,
        })
    }

    /// The number of sample frames (samples divided by channel count).
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.get() as usize
    }

    /// `true` if the source repeats forever.
    pub fn is_looped(&self) -> bool {
        self.looped
    }
}

impl Iterator for ClipSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.samples.is_empty() {
            return None;
        }
        if self.pos >= self.samples.len() {
            if self.looped {
                self.pos = 0;
            } else {
                return None;
            }
        }
        let sample = self.samples[self.pos];
        self.pos += 1;
        Some(sample)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.looped {
            (0, None)
        } else {
            let remaining = self.samples.len().saturating_sub(self.pos);
            (remaining, Some(remaining))
        }
    }
}

impl Source for ClipSource {
    fn current_span_len(&self) -> Option<usize> {
        if self.samples.is_empty() || (!self.looped && self.pos >= self.samples.len()) {
            Some(0)
        } else if self.looped {
            None
        } else {
            Some(self.samples.len())
        }
    }

    fn channels(&self) -> ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        if self.looped {
            return None;
        }
        let frames = self.frames();
        if frames == 0 {
            return Some(Duration::ZERO);
        }
        let seconds = frames as f32 / self.sample_rate.get() as f32;
        Some(Duration::from_secs_f32(seconds))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(channels: u16, sample_rate: u32, samples: Vec<f32>) -> AudioClip {
        AudioClip {
            name: "test".into(),
            sample_rate,
            channels,
            samples,
        }
    }

    #[test]
    fn rejects_degenerate_clips() {
        assert!(ClipSource::new(&clip(0, 48_000, vec![1.0]), false).is_none());
        assert!(ClipSource::new(&clip(1, 0, vec![1.0]), false).is_none());
    }

    #[test]
    fn iterates_mono_samples() {
        let source = ClipSource::new(&clip(1, 48_000, vec![1.0, 2.0, 3.0]), false).unwrap();
        let collected: Vec<f32> = source.collect();
        assert_eq!(collected, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn reports_channels_rate_and_span() {
        let source = ClipSource::new(&clip(2, 44_100, vec![0.0; 8]), false).unwrap();
        assert_eq!(source.channels().get(), 2);
        assert_eq!(source.sample_rate().get(), 44_100);
        assert_eq!(source.current_span_len(), Some(8));
        assert_eq!(source.frames(), 4);
    }

    #[test]
    fn total_duration_matches_frames() {
        let source = ClipSource::new(&clip(2, 48_000, vec![0.0; 96_000]), false).unwrap();
        let duration = source.total_duration().unwrap();
        assert!((duration.as_secs_f32() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn looped_source_wraps_and_never_ends() {
        let mut source = ClipSource::new(&clip(1, 48_000, vec![1.0, 2.0]), true).unwrap();
        assert_eq!(source.current_span_len(), None);
        assert_eq!(source.total_duration(), None);
        let produced: Vec<f32> = (&mut source).take(5).collect();
        assert_eq!(produced, vec![1.0, 2.0, 1.0, 2.0, 1.0]);
    }

    #[test]
    fn exhausted_source_reports_span_zero() {
        let mut source = ClipSource::new(&clip(1, 48_000, vec![1.0]), false).unwrap();
        assert_eq!(source.next(), Some(1.0));
        assert_eq!(source.next(), None);
        assert_eq!(source.current_span_len(), Some(0));
        assert!(source.is_exhausted());
    }
}
