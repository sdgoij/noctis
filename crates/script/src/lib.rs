//! # script — JavaScript behaviours for the Noctis engine
//!
//! This crate is the scripting layer. It has two halves that meet at the
//! [`ScriptHost`] trait:
//!
//! * **The engine side** ([`ScriptRuntime`], [`ScriptApi`]) discovers
//!   [`Script`](ecs::components::Script) components in a
//!   [`World`](ecs::world::World), compiles each unique source once, and drives
//!   the `on_start` / `on_update` / `on_destroy` lifecycle. It exposes an engine
//!   API to scripts and records every effect as a [`ScriptCommand`].
//! * **The host side** ([`ScriptHost`]) actually executes script code. The real
//!   JavaScript engine is the optional `slag` feature; the default build ships
//!   [`NoopHost`] and [`RecordingHost`], which make the whole runtime testable
//!   without a JS engine.
//!
//! ## The `slag` feature
//!
//! Scripting is JavaScript, executed by the "Slag" engine. Because pulling in a
//! JS engine (its WebAssembly runtime, the Cranelift JIT, and an enormous
//! test262 submodule) is not appropriate for every build, the real backend lives
//! behind the **off-by-default** `slag` feature:
//!
//! ```text
//! cargo build -p script --features slag
//! ```
//!
//! The feature enables Slag's **WebAssembly JS API** (scripts see the
//! `WebAssembly` global and can build and instantiate modules) and its
//! **Cranelift JIT hook** (`SlagHost::jit_enabled`, hot certified bodies run at
//! machine speed).
//!
//! Without the feature the crate is fully functional but inert: scripts are
//! discovered and their lifecycles are tracked, but no user code runs. See
//! [`slag`] (only compiled with the feature) for the backend.
//!
//! ## Example
//!
//! ```
//! use script::{ScriptRuntime, RecordingHost, ScriptApi};
//! use ecs::prelude::*;
//! use math::vec3;
//!
//! let mut host = RecordingHost::new();
//! host.on("on_update", |api: &mut ScriptApi<'_, '_>| {
//!     if let Some(entity) = api.current_entity() {
//!         api.translate(entity, vec3(1.0, 0.0, 0.0));
//!     }
//! });
//!
//! let mut runtime = ScriptRuntime::new(host);
//! let assets = AssetRegistry::new();
//!
//! let mut world = World::new();
//! let entity = world.spawn();
//! world.insert(entity, Script::from_source("spin();"));
//!
//! runtime.run_update(&mut world, &assets, 1.0 / 60.0);
//! assert_eq!(runtime.instance_count(), 1);
//! ```

#![forbid(unsafe_code)]

pub mod api;
pub mod host;
pub mod runtime;

#[cfg(feature = "slag")]
pub mod slag;

pub use api::{EntityId, ScriptApi, ScriptCommand, ScriptContext, entity_from_id, entity_id};
pub use ecs::world::Entity;
pub use host::{
    LifecycleCall, ModuleId, NoopHost, PendingApi, RecordingHost, ScriptBehaviour, ScriptHost,
};
pub use runtime::{ON_DESTROY, ON_START, ON_UPDATE, RunReport, ScriptInstance, ScriptRuntime};

/// The most commonly used imports.
pub mod prelude {
    pub use crate::api::{EntityId, ScriptApi, ScriptCommand, ScriptContext};
    pub use crate::host::{
        LifecycleCall, ModuleId, NoopHost, PendingApi, RecordingHost, ScriptBehaviour, ScriptHost,
    };
    pub use crate::runtime::{
        ON_DESTROY, ON_START, ON_UPDATE, RunReport, ScriptInstance, ScriptRuntime,
    };
    pub use ecs::prelude::*;
    pub use math;
}
