//! # platform — the window and event-loop host
//!
//! This crate owns the window and the winit event loop, and drives *any*
//! [`AppDriver`] against a window-backed [`render::Renderer`]. It deliberately
//! does not depend on `runtime`: instead, a game (or the editor) implements
//! [`AppDriver`] and hands itself to [`run`]. That keeps the dependency graph
//! acyclic (`platform` <- `runtime`).
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

pub mod driver;
pub mod error;
mod window;

pub use driver::AppDriver;
pub use error::PlatformError;
pub use window::{WindowConfig, WindowHost, map_key, run};
