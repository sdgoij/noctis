//! Errors produced by the platform host.

use std::fmt;

use render::RenderError;

/// Anything that can go wrong creating or running the windowed application.
#[derive(Debug)]
pub enum PlatformError {
    /// The winit event loop could not be created or run.
    EventLoop(String),
    /// No window was created, or the surface could not be created from it.
    Window(String),
    /// The renderer could not be initialised for the window surface.
    Render(RenderError),
    /// The scene has no camera to render from.
    NoCamera,
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlatformError::EventLoop(msg) => write!(f, "event loop error: {msg}"),
            PlatformError::Window(msg) => write!(f, "window error: {msg}"),
            PlatformError::Render(err) => write!(f, "renderer error: {err}"),
            PlatformError::NoCamera => write!(f, "the scene has no camera"),
        }
    }
}

impl std::error::Error for PlatformError {}

impl From<RenderError> for PlatformError {
    fn from(value: RenderError) -> Self {
        PlatformError::Render(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_non_empty() {
        let cases = [
            PlatformError::EventLoop("x".into()),
            PlatformError::Window("x".into()),
            PlatformError::Render(RenderError::NoAdapter),
            PlatformError::NoCamera,
        ];
        for case in cases {
            assert!(!case.to_string().is_empty());
        }
    }

    #[test]
    fn render_error_converts() {
        let err: PlatformError = RenderError::NoAdapter.into();
        assert!(matches!(err, PlatformError::Render(RenderError::NoAdapter)));
    }
}
