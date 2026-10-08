//! # noctis — the Noctis engine
//!
//! `noctis` is both the engine's multicall command-line tool and its embedding
//! facade.
//!
//! * The **binary** dispatches subcommands — `noctis editor`, `noctis run`,
//!   `noctis build`, `noctis rename` — each implemented by the crate that owns
//!   it.
//! * The **library** exposes the whole engine through [`prelude`], so an
//!   application can `use noctis::prelude::*` and drive the very crates the tool
//!   does. Everything is re-exported from the engine crates, never reimplemented
//!   here.
//!
//! ```no_run
//! use noctis::prelude::*;
//!
//! let mut scene = Scene::new("Game");
//! let camera = scene.world.spawn();
//! scene.world.insert(camera, LocalTransform::from_translation(Vec3::new(0.0, 1.5, 6.0)));
//! scene.world.insert(camera, Camera::default());
//!
//! let mut runtime = Runtime::new(scene, RuntimeConfig::default());
//! let summary = runtime.run_headless(60, 1.0 / 60.0);
//! assert_eq!(summary.frames, 60);
//! ```

#![forbid(unsafe_code)]

pub mod prelude;
