//! Errors produced by the renderer.
//!
//! Every fallible entry point in this crate returns [`RenderError`]. The GPU
//! paths are deliberately tolerant of a missing adapter: [`RenderError::NoAdapter`]
//! is returned (rather than panicking) so applications and tests can degrade
//! gracefully on machines without a usable graphics device.

use std::fmt;

/// Anything that can go wrong while loading assets or rendering a frame.
#[derive(Debug)]
pub enum RenderError {
    /// No graphics adapter matched the request. On headless CI machines this is
    /// the expected outcome and callers should skip rendering instead of
    /// failing.
    NoAdapter,
    /// The adapter was found but the logical device could not be created.
    RequestDevice(String),
    /// A glTF document failed to parse or import.
    Gltf(String),
    /// An image failed to decode or had an unsupported pixel format.
    Image(String),
    /// A filesystem operation failed.
    Io(String),
    /// The requested camera entity does not exist or has no [`ecs::components::Camera`].
    NoCamera,
    /// A referenced asset (mesh, material, texture) was missing.
    MissingAsset(String),
    /// The requested operation is not supported by this backend or configuration.
    Unsupported(String),
    /// Reading back the rendered frame from the GPU failed.
    Readback(String),
    /// Presenting to a window surface failed (acquire/configure/present).
    Surface(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::NoAdapter => write!(f, "no compatible graphics adapter was found"),
            RenderError::RequestDevice(msg) => write!(f, "failed to create device: {msg}"),
            RenderError::Gltf(msg) => write!(f, "glTF error: {msg}"),
            RenderError::Image(msg) => write!(f, "image error: {msg}"),
            RenderError::Io(msg) => write!(f, "io error: {msg}"),
            RenderError::NoCamera => write!(f, "camera entity not found"),
            RenderError::MissingAsset(msg) => write!(f, "missing asset: {msg}"),
            RenderError::Unsupported(msg) => write!(f, "unsupported: {msg}"),
            RenderError::Readback(msg) => write!(f, "frame readback failed: {msg}"),
            RenderError::Surface(msg) => write!(f, "surface error: {msg}"),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<gltf::Error> for RenderError {
    fn from(value: gltf::Error) -> Self {
        RenderError::Gltf(value.to_string())
    }
}

impl From<std::io::Error> for RenderError {
    fn from(value: std::io::Error) -> Self {
        RenderError::Io(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_non_empty() {
        let cases = [
            RenderError::NoAdapter,
            RenderError::RequestDevice("boom".into()),
            RenderError::Gltf("bad".into()),
            RenderError::Image("bad".into()),
            RenderError::Io("bad".into()),
            RenderError::NoCamera,
            RenderError::MissingAsset("mesh".into()),
            RenderError::Unsupported("x".into()),
            RenderError::Readback("x".into()),
            RenderError::Surface("x".into()),
        ];
        for case in cases {
            assert!(!case.to_string().is_empty());
        }
    }

    #[test]
    fn implements_std_error() {
        fn assert_error<E: std::error::Error>(_: &E) {}
        assert_error(&RenderError::NoAdapter);
    }
}
