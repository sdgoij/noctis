//! How a viewport is created — shared by the native (winit) and web hosts.

/// The `<canvas>` element the web host renders into, when the page does not
/// provide one.
pub const DEFAULT_CANVAS_ID: &str = "noctis-canvas";

/// How the viewport is created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowConfig {
    /// The window title (native only).
    pub title: String,
    /// The initial width in logical pixels.
    pub width: u32,
    /// The initial height in logical pixels.
    pub height: u32,
    /// MSAA sample count for the viewport (`1` disables it). The renderer uses
    /// the largest supported count that does not exceed this.
    pub sample_count: u32,
    /// The id of the `<canvas>` to render into (web only): an existing canvas with
    /// this id is used, else one is created. Defaults to [`DEFAULT_CANVAS_ID`].
    pub canvas_id: String,
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
            canvas_id: DEFAULT_CANVAS_ID.to_string(),
        }
    }
}
