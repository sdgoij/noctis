//! # platform — the window and event-loop host
//!
//! This crate owns the window and the event loop, and drives *any*
//! [`AppDriver`] against a surface-backed [`render::Renderer`]. It deliberately
//! does not depend on `runtime`: instead, a game (or the editor) implements
//! [`AppDriver`] and hands itself to [`run`]. That keeps the dependency graph
//! acyclic (`platform` <- `runtime`).
//!
//! Two hosts implement the same [`AppDriver`] seam:
//!
//! - **native** (winit + egui-winit + egui-wgpu) — the windowed host, with an
//!   egui overlay;
//! - **web** (`wasm32`) — a `<canvas>` and a `requestAnimationFrame` loop driven
//!   through `web-sys` (see `.notes/web.md`). The browser client comes first, so the
//!   web host draws no egui overlay yet.
//!
//! The windowed loop blocks and needs a display, so it is not unit-tested; the
//! pure pieces (key mapping, configuration) are.
//!
//! ```no_run
//! use platform::{AppDriver, WindowConfig};
//! use ecs::assets::AssetRegistry;
//! use ecs::resources::InputState;
//! use ecs::world::{Entity, World};
//!
//! struct MyGame { world: World, assets: AssetRegistry, input: InputState }
//! impl AppDriver for MyGame {
//!     fn update(&mut self, _dt: f32) {}
//!     fn world(&self) -> &World { &self.world }
//!     fn assets(&self) -> &AssetRegistry { &self.assets }
//!     fn camera(&self) -> Option<Entity> { None }
//!     fn input_mut(&mut self) -> Option<&mut InputState> { Some(&mut self.input) }
//! }
//!
//! // let game = MyGame { /* ... */ };
//! // platform::run(game, WindowConfig::default()).expect("window");
//! ```

#![forbid(unsafe_code)]

mod config;
pub mod driver;
mod egui_ui;
pub mod error;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
mod web_egui;
#[cfg(not(target_arch = "wasm32"))]
mod window;

pub use config::{DEFAULT_CANVAS_ID, WindowConfig};
pub use driver::AppDriver;
pub use egui_ui::EguiUi;
pub use error::PlatformError;

#[cfg(target_arch = "wasm32")]
pub use web::{browser_download, init_logging, run};
#[cfg(not(target_arch = "wasm32"))]
pub use window::{WindowHost, map_key, run};

/// The egui version this crate draws with, re-exported so drivers can build UI
/// without depending on `egui` directly.
pub use egui;
