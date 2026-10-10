//! Decoding audio files into [`AudioClip`]s.
//!
//! [`load_clip`] reads an audio file (WAV, Ogg/Vorbis, or MP3 — whatever the
//! `symphonia` backends behind rodio support) into an [`AudioClip`] of
//! interleaved `f32` samples, so a project's `assets/audio/*` become playable.
//! [`load_clip_bytes`] is the byte-based counterpart, for a caller that reads
//! the file through a store rather than a path.

use std::path::Path;

use ecs::assets::AudioClip;
use rodio::Source;

use crate::AudioError;

/// Decode an audio file into an [`AudioClip`] (interleaved `f32` samples in
/// `[-1, 1]`, with its sample rate and channel count).
pub fn load_clip(path: impl AsRef<Path>) -> Result<AudioClip, AudioError> {
    let path = path.as_ref();
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("clip")
        .to_string();
    let file = std::fs::File::open(path).map_err(|error| AudioError::Io(error.to_string()))?;
    let decoder = rodio::Decoder::new(std::io::BufReader::new(file))
        .map_err(|error| AudioError::Decode(error.to_string()))?;
    Ok(clip_from_decoder(name, decoder))
}

/// Decode in-memory audio `bytes` into an [`AudioClip`] named `name`.
///
/// The byte-based counterpart of [`load_clip`], for a caller that reads the file
/// through a store (a project bundle, browser storage) rather than a path.
pub fn load_clip_bytes(name: &str, bytes: &[u8]) -> Result<AudioClip, AudioError> {
    let decoder = rodio::Decoder::new(std::io::Cursor::new(bytes.to_vec()))
        .map_err(|error| AudioError::Decode(error.to_string()))?;
    Ok(clip_from_decoder(name.to_string(), decoder))
}

/// Collect a rodio decoder into an [`AudioClip`].
fn clip_from_decoder<R>(name: String, decoder: rodio::Decoder<R>) -> AudioClip
where
    R: std::io::Read + std::io::Seek,
{
    let sample_rate = decoder.sample_rate().get();
    let channels = decoder.channels().get();
    let samples: Vec<f32> = decoder.collect();
    AudioClip {
        name,
        sample_rate,
        channels,
        samples,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal 16-bit PCM WAV file: one channel at `sample_rate`.
    fn wav_bytes(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes()); // PCM chunk size
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // mono
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
        out.extend_from_slice(&2u16.to_le_bytes()); // block align
        out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            out.extend_from_slice(&sample.to_le_bytes());
        }
        out
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "noctis-audio-{tag}-{}-{:?}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn decodes_a_wav_into_a_clip() {
        let samples = [0i16, 16_384, -16_384, 32_767];
        let path = temp_path("wav");
        std::fs::write(&path, wav_bytes(8_000, &samples)).expect("write wav");

        let clip = load_clip(&path).expect("decode");
        assert_eq!(clip.sample_rate, 8_000);
        assert_eq!(clip.channels, 1);
        assert_eq!(clip.samples.len(), samples.len());
        assert!(clip.name.starts_with("noctis-audio-wav"));

        // The 16-bit samples decode to `[-1, 1]` with a tolerance for rounding.
        let expected = [0.0f32, 0.5, -0.5, 1.0];
        for (got, want) in clip.samples.iter().zip(expected) {
            assert!((got - want).abs() < 1e-4, "got {got}, want {want}");
        }

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn decodes_wav_from_bytes() {
        let samples = [0i16, 16_384, -16_384, 32_767];
        let clip = load_clip_bytes("beep", &wav_bytes(8_000, &samples)).expect("decode bytes");
        assert_eq!(clip.sample_rate, 8_000);
        assert_eq!(clip.channels, 1);
        assert_eq!(clip.name, "beep", "the caller names the clip");
        assert_eq!(clip.samples.len(), samples.len());
    }

    #[test]
    fn a_non_audio_file_is_a_decode_error() {
        let path = temp_path("junk");
        std::fs::write(&path, b"this is not audio").expect("write junk");
        let error = load_clip(&path).unwrap_err();
        assert!(matches!(error, AudioError::Decode(_)), "{error:?}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let error = load_clip(temp_path("missing")).unwrap_err();
        assert!(matches!(error, AudioError::Io(_)), "{error:?}");
    }
}
