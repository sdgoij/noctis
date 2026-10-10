//! How a viewport is created — shared by the native (winit) and web hosts.

/// How the viewport is created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowConfig {
    /// The window title.
    pub title: String,
    /// The initial width in logical pixels.
    pub width: u32,
    /// The initial height in logical pixels.
    pub height: u32,
    /// MSAA sample count for the viewport (`1` disables it). The renderer uses
    /// the largest supported count that does not exceed this.
    pub sample_count: u32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "Noctis".to_string(),
            width: 1280,
            height: 720,
            // A window is looked at, so anti-alias it by default; a project's
            // `[render] sample_count` overrides this.
            sample_count: 4,
        }
    }
}
