//! The real JavaScript backend, compiled only with the `slag` feature.
//!
//! This module embeds the [Slag](https://github.com/sdgoij/slag) JavaScript
//! engine, including its WebAssembly JS API and its Cranelift JIT hook. It is
//! **off by default** because those pull a from-scratch WASM engine, Cranelift,
//! and a very large pinned `test262` submodule, which is inappropriate for the
//! ordinary engine build. Enable it with:
//!
//! ```text
//! cargo build -p script --features slag
//! ```
//!
//! # How the bridge works
//!
//! Slag's registered host functions are `'static` closures that cannot borrow
//! the [`ScriptContext`] for the duration of a script call. So each call is
//! two-phase:
//!
//! 1. **Snapshot + run.** The host copies the parts of the engine the API can
//!    read (transforms, names, input, timing) into a shared [`Bridge`], then
//!    runs the script's lifecycle function. Registered functions read from the
//!    snapshot and append [`ScriptCommand`]s to the bridge's queue. Entities
//!    spawned from JavaScript get *placeholder* handles, and the snapshot is
//!    updated in place so a later `set_position(spawn(), ...)` in the same call
//!    sees the change.
//! 2. **Replay.** After the script returns, every queued command is replayed
//!    through [`ScriptApi`] against the real world, translating placeholder
//!    handles to the real entities they became.
//!
//! This keeps the world-mutation logic in one place ([`ScriptApi`]) and means
//! the backend never needs `unsafe`.
//!
//! # Caveats
//!
//! * This targets the pinned embedding API (`Context`, `JsValue`,
//!   `FunctionCall`, `Context::register_fn` / `eval` / `call`). Slag is a
//!   fast-moving, spec-first engine; if a revision renames part of that
//!   surface the backend may need a small update.
//! * Vector getters (`get_position`, ...) return a JSON string (`"[x,y,z]"`)
//!   because the minimal embedding surface shown in Slag's README does not
//!   describe composite value construction.
//! * JS runs on the thread that owns the host; the host is not `Send`.

use std::cell::RefCell;
use std::rc::Rc;

use ecs::resources::{Key, MouseButton};
use math::{Vec3, vec3};

use crate::api::{EntityId, ScriptCommand, ScriptContext};
use crate::bridge::{Bridge, entity_of, handle_of, replay};
use crate::host::{ModuleId, PendingApi, ScriptHost};

/// A failure constructing or driving the Slag backend.
#[derive(Debug)]
pub enum SlagError {
    /// The engine could not be initialised.
    Engine(String),
    /// A script failed to compile or run.
    Script(String),
}

impl std::fmt::Display for SlagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SlagError::Engine(msg) => write!(f, "slag engine error: {msg}"),
            SlagError::Script(msg) => write!(f, "slag script error: {msg}"),
        }
    }
}

impl std::error::Error for SlagError {}

/// The Slag-backed [`ScriptHost`].
pub struct SlagHost {
    context: slag::Context,
    bridge: Rc<RefCell<Bridge>>,
    /// The per-call handle table handed to JavaScript (see [`handle_of`]): scripts
    /// receive small indices, never raw `EntityId`s.
    handles: Rc<RefCell<Vec<EntityId>>>,
    next_module: ModuleId,
    errors: Vec<String>,
    jit: bool,
}

impl SlagHost {
    /// Create the engine, install the Cranelift JIT hook, and install the Noctis
    /// API globals.
    ///
    /// The JIT is an optimisation: if the hook cannot be installed, scripts still
    /// run on the interpreter, so the failure is logged rather than fatal.
    pub fn new() -> Result<Self, SlagError> {
        let mut context = slag::Context::new().map_err(engine_err)?;
        let jit = match slag::install_jit(&mut context) {
            Ok(()) => true,
            Err(err) => {
                log::warn!("Slag JIT hook unavailable ({err}); running interpretively");
                false
            }
        };
        let mut host = Self {
            context,
            bridge: Rc::new(RefCell::new(Bridge::default())),
            handles: Rc::new(RefCell::new(Vec::new())),
            next_module: 1,
            errors: Vec::new(),
            jit,
        };
        host.install_api().map_err(engine_err)?;
        Ok(host)
    }

    /// `true` if the Cranelift JIT hook was installed. Only ever `false` when the
    /// hook failed to install, in which case scripts still run interpretively.
    pub fn jit_enabled(&self) -> bool {
        self.jit
    }

    /// Errors from scripts that failed to compile or run.
    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    fn register(&mut self, name: &str, arity: u32, f: slag::HostFn) -> Result<(), slag::JsError> {
        self.context.register_fn(name, arity, f).map(|_| ())
    }

    fn install_api(&mut self) -> Result<(), slag::JsError> {
        let b = Rc::clone(&self.bridge);
        self.register(
            "log",
            1,
            Box::new(move |call| {
                let message = arg_string(call, 0).unwrap_or_default();
                let mut bridge = b.borrow_mut();
                let entity = bridge.entity;
                bridge.commands.push(ScriptCommand::Log { message, entity });
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "spawn",
            1,
            Box::new(move |call| {
                let name = arg_name(call, 0);
                let id = b.borrow_mut().spawn(name);
                let handle = handle_of(&mut h.borrow_mut(), id);
                Ok(slag::JsValue::number(handle as f64))
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "destroy",
            1,
            Box::new(move |call| {
                if let Some(id) = arg_entity(&h.borrow(), call, 0) {
                    b.borrow_mut()
                        .commands
                        .push(ScriptCommand::Destroy { entity: id });
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "destroy_by_name",
            1,
            Box::new(move |call| {
                if let Some(name) = arg_string(call, 0) {
                    b.borrow_mut()
                        .commands
                        .push(ScriptCommand::DestroyByName { name });
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "find_entity",
            1,
            Box::new(move |call| {
                let found = arg_string(call, 0)
                    .and_then(|name| b.borrow().find(&name))
                    .map(|id| handle_of(&mut h.borrow_mut(), id));
                Ok(match found {
                    Some(handle) => slag::JsValue::number(handle as f64),
                    None => slag::JsValue::undefined(),
                })
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "get_position",
            1,
            Box::new(move |call| {
                let out =
                    arg_entity(&h.borrow(), call, 0).map(|id| vec_json(b.borrow().position(id)));
                Ok(out.map_or_else(slag::JsValue::undefined, slag::JsValue::string))
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "set_position",
            4,
            Box::new(move |call| {
                if let (Some(id), Some(p)) = (arg_entity(&h.borrow(), call, 0), arg_vec3(call, 1)) {
                    b.borrow_mut().set_position(id, p);
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "translate",
            4,
            Box::new(move |call| {
                if let (Some(id), Some(d)) = (arg_entity(&h.borrow(), call, 0), arg_vec3(call, 1)) {
                    b.borrow_mut().translate(id, d);
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "get_rotation_euler",
            1,
            Box::new(move |call| {
                let out =
                    arg_entity(&h.borrow(), call, 0).map(|id| vec_json(b.borrow().rotation(id)));
                Ok(out.map_or_else(slag::JsValue::undefined, slag::JsValue::string))
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "set_rotation_euler",
            4,
            Box::new(move |call| {
                if let (Some(id), Some(e)) = (arg_entity(&h.borrow(), call, 0), arg_vec3(call, 1)) {
                    b.borrow_mut().set_rotation(id, e);
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "get_scale",
            1,
            Box::new(move |call| {
                let out = arg_entity(&h.borrow(), call, 0).map(|id| vec_json(b.borrow().scale(id)));
                Ok(out.map_or_else(slag::JsValue::undefined, slag::JsValue::string))
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "set_scale",
            4,
            Box::new(move |call| {
                if let (Some(id), Some(s)) = (arg_entity(&h.borrow(), call, 0), arg_vec3(call, 1)) {
                    b.borrow_mut().set_scale(id, s);
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "time_delta",
            0,
            Box::new(move |_call| Ok(slag::JsValue::number(b.borrow().delta as f64))),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "time_elapsed",
            0,
            Box::new(move |_call| Ok(slag::JsValue::number(b.borrow().elapsed as f64))),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "key_down",
            1,
            Box::new(move |call| {
                let down = arg_string(call, 0)
                    .and_then(|name| Key::from_name(&name))
                    .is_some_and(|key| b.borrow().input.key_down(key));
                Ok(slag::JsValue::boolean(down))
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "mouse_down",
            1,
            Box::new(move |call| {
                let button =
                    arg_string(call, 0).and_then(|s| match s.to_ascii_lowercase().as_str() {
                        "left" => Some(MouseButton::Left),
                        "right" => Some(MouseButton::Right),
                        "middle" => Some(MouseButton::Middle),
                        _ => None,
                    });
                let down = button.is_some_and(|btn| b.borrow().input.mouse.is_down(btn));
                Ok(slag::JsValue::boolean(down))
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "add_script",
            2,
            Box::new(move |call| {
                if let (Some(id), Some(source)) =
                    (arg_entity(&h.borrow(), call, 0), arg_string(call, 1))
                {
                    b.borrow_mut()
                        .commands
                        .push(ScriptCommand::AddScript { entity: id, source });
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "instantiate_prefab",
            2,
            Box::new(move |call| {
                // Return a handle for the prefab's root so the script can address
                // it immediately (see `Bridge::instantiate_prefab`).
                let root = arg_string(call, 0).map(|json| {
                    let parent = arg_entity(&h.borrow(), call, 1);
                    b.borrow_mut().instantiate_prefab(json, parent)
                });
                Ok(match root {
                    Some(id) => {
                        let handle = handle_of(&mut h.borrow_mut(), id);
                        slag::JsValue::number(handle as f64)
                    }
                    None => slag::JsValue::undefined(),
                })
            }),
        )?;

        Ok(())
    }

    fn module_program(&self, id: ModuleId, source: &str) -> String {
        // Wrap the user's source so the lifecycle functions become properties of
        // a namespace stored on a well-known global. `typeof` guards keep
        // scripts that only implement one hook valid.
        format!(
            "globalThis.__noctis_ns_{id} = (function() {{ {source}\n;\n  \
             return {{ \
               on_start: (typeof on_start === 'function') ? on_start : null, \
               on_update: (typeof on_update === 'function') ? on_update : null, \
               on_destroy: (typeof on_destroy === 'function') ? on_destroy : null \
             }}; \
             }})();"
        )
    }
}

fn engine_err(err: slag::JsError) -> SlagError {
    SlagError::Engine(err.to_string())
}

fn arg_string(call: &slag::FunctionCall<'_>, index: usize) -> Option<String> {
    let value = call.arg(index)?;
    call.to_string(&value).ok()
}

/// Like [`arg_string`], but treats missing/`undefined`/`null`/empty as absent.
fn arg_name(call: &slag::FunctionCall<'_>, index: usize) -> Option<String> {
    arg_string(call, index).filter(|s| !s.is_empty() && s != "undefined" && s != "null")
}

fn arg_entity(
    handles: &[EntityId],
    call: &slag::FunctionCall<'_>,
    index: usize,
) -> Option<EntityId> {
    let handle = call.arg(index).and_then(|v| v.as_number())?;
    entity_of(handles, handle as i32)
}

fn arg_number(call: &slag::FunctionCall<'_>, index: usize) -> Option<f32> {
    call.arg(index)
        .and_then(|v| v.as_number())
        .map(|n| n as f32)
}

fn arg_vec3(call: &slag::FunctionCall<'_>, index: usize) -> Option<Vec3> {
    // Accepted forms: a JSON string "[x,y,z]" emitted by our own getters, or
    // three consecutive numeric arguments starting at `index`.
    if let Some(v) = arg_string(call, index).and_then(|s| parse_vec_str(&s)) {
        return Some(v);
    }
    let x = arg_number(call, index)?;
    let y = arg_number(call, index + 1)?;
    let z = arg_number(call, index + 2)?;
    Some(vec3(x, y, z))
}

fn parse_vec_str(text: &str) -> Option<Vec3> {
    let text = text.trim();
    if !text.starts_with('[') || !text.ends_with(']') {
        return None;
    }
    let inner = &text[1..text.len() - 1];
    let mut parts = inner.split(',');
    let x = parts.next()?.trim().parse::<f32>().ok()?;
    let y = parts.next()?.trim().parse::<f32>().ok()?;
    let z = parts.next()?.trim().parse::<f32>().ok()?;
    Some(vec3(x, y, z))
}

fn vec_json(v: Vec3) -> String {
    format!("[{},{},{}]", v.x, v.y, v.z)
}

impl ScriptHost for SlagHost {
    fn eval_script(&mut self, source: &str) -> ModuleId {
        let id = self.next_module;
        self.next_module += 1;
        let program = self.module_program(id, source);
        if let Err(err) = self.context.eval(&program) {
            self.errors.push(format!("module {id}: {err}"));
        }
        id
    }

    fn call_lifecycle(&mut self, module: ModuleId, func: &str, ctx: &mut ScriptContext<'_>) {
        self.bridge.borrow_mut().begin(ctx);
        self.handles.borrow_mut().clear();

        let function = format!("globalThis.__noctis_ns_{module}.{func}");
        if let Ok(callee) = self.context.eval(&function)
            && !callee.is_null()
            && !callee.is_undefined()
            && let Some(entity) = ctx.entity
        {
            let handle = handle_of(&mut self.handles.borrow_mut(), entity);
            let args = [
                slag::JsValue::number(handle as f64),
                slag::JsValue::number(ctx.delta as f64),
            ];
            if let Err(err) = self
                .context
                .call(&callee, &slag::JsValue::undefined(), &args)
            {
                self.errors.push(format!("module {module}.{func}: {err}"));
            }
        }

        let commands = std::mem::take(&mut self.bridge.borrow_mut().commands);
        replay(ctx, commands);
    }

    fn set_pending_api(&mut self, _api: PendingApi) {
        // The snapshot taken in `call_lifecycle` already carries this data.
    }
}

impl std::fmt::Debug for SlagHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlagHost")
            .field("modules", &self.next_module)
            .field("errors", &self.errors.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::ScriptRuntime;
    use ecs::assets::AssetRegistry;
    use ecs::components::{LocalTransform, Name, Script};
    use ecs::world::World;

    const SOURCE: &str = "
        function on_update(entity, dt) {
            var child = spawn('Child');
            set_position(child, 1, 2, 3);
            set_position(entity, 4, 5, 6);
            log('tick');
        }
    ";

    #[test]
    fn runs_a_real_script_against_the_world() {
        let host = SlagHost::new().expect("slag engine starts");
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();

        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Script::from_source(SOURCE));

        runtime.run_update(&mut world, &assets, 0.016);

        // The script's own entity moved to (4,5,6) through the bridge.
        assert_eq!(
            world.get::<LocalTransform>(entity).unwrap().translation,
            vec3(4.0, 5.0, 6.0)
        );

        // The entity it spawned exists, is named, and sits at (1,2,3).
        let child = world
            .query::<Name>()
            .find(|(_, n)| n.0 == "Child")
            .map(|(e, _)| e)
            .expect("spawn produced a named child");
        assert_eq!(
            world.get::<LocalTransform>(child).unwrap().translation,
            vec3(1.0, 2.0, 3.0)
        );

        assert!(
            runtime.host().errors().is_empty(),
            "unexpected script errors: {:?}",
            runtime.host().errors()
        );
        assert!(
            runtime
                .command_log()
                .iter()
                .any(|c| matches!(c, ScriptCommand::Log { .. }))
        );
    }
}
