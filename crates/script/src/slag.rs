//! The JavaScript backend: the embedded [Slag](https://github.com/sdgoij/slag)
//! engine, including its WebAssembly JS API and its Cranelift JIT hook.
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
//! * Composite getters (`get_position`, `get_rotation_euler`, `get_scale`,
//!   `get_velocity`, `mouse_move`, `mouse_position`, `viewport_size`) return a
//!   JavaScript **array** (`[x, y, z]` / `[a, b]`), matching the WebAssembly
//!   ABI's multi-value floats; `FunctionCall::create_array` builds them.
//! * JS runs on the thread that owns the host; the host is not `Send`.

use std::cell::RefCell;
use std::collections::HashMap;
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
    /// Whether each compiled module reads reflected components, so its calls
    /// build the component snapshot. See [`source_reads_components`].
    reflect: HashMap<ModuleId, bool>,
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
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut context = slag::Context::new().map_err(engine_err)?;
        // The Cranelift JIT hook is native-only (Slag's `jit` feature is not built
        // for wasm), so on the web scripts run on the interpreter.
        #[cfg(not(target_arch = "wasm32"))]
        let jit = match slag::install_jit(&mut context) {
            Ok(()) => true,
            Err(err) => {
                log::warn!("Slag JIT hook unavailable ({err}); running interpretively");
                false
            }
        };
        #[cfg(target_arch = "wasm32")]
        let jit = false;
        let mut host = Self {
            context,
            bridge: Rc::new(RefCell::new(Bridge::default())),
            handles: Rc::new(RefCell::new(Vec::new())),
            next_module: 1,
            reflect: HashMap::new(),
            errors: Vec::new(),
            jit,
        };
        host.install_api().map_err(engine_err)?;
        Ok(host)
    }

    /// `true` if the Cranelift JIT hook was installed. Only ever `false` when the
    /// hook failed to install, in which case scripts still run interpretively; it
    /// is always `false` on wasm, where the JIT is not built.
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
            Box::new(move |call| match arg_entity(&h.borrow(), call, 0) {
                Some(id) => vec_array(call, b.borrow().position(id)),
                None => Ok(slag::JsValue::undefined()),
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
            Box::new(move |call| match arg_entity(&h.borrow(), call, 0) {
                Some(id) => vec_array(call, b.borrow().rotation(id)),
                None => Ok(slag::JsValue::undefined()),
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
            Box::new(move |call| match arg_entity(&h.borrow(), call, 0) {
                Some(id) => vec_array(call, b.borrow().scale(id)),
                None => Ok(slag::JsValue::undefined()),
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
        self.register(
            "mouse_move",
            0,
            Box::new(move |call| {
                let d = b.borrow().input.mouse.delta;
                pair_array(call, d[0], d[1])
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "mouse_position",
            0,
            Box::new(move |call| {
                let p = b.borrow().input.mouse.position;
                pair_array(call, p[0], p[1])
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "mouse_scroll",
            0,
            Box::new(move |_call| Ok(slag::JsValue::number(b.borrow().input.mouse.scroll as f64))),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "set_velocity",
            4,
            Box::new(move |call| {
                if let (Some(id), Some(v)) = (arg_entity(&h.borrow(), call, 0), arg_vec3(call, 1)) {
                    b.borrow_mut().set_velocity(id, v);
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "add_velocity",
            4,
            Box::new(move |call| {
                if let (Some(id), Some(v)) = (arg_entity(&h.borrow(), call, 0), arg_vec3(call, 1)) {
                    b.borrow_mut().add_velocity(id, v);
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "apply_impulse",
            4,
            Box::new(move |call| {
                if let (Some(id), Some(v)) = (arg_entity(&h.borrow(), call, 0), arg_vec3(call, 1)) {
                    b.borrow_mut().apply_impulse(id, v);
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "get_velocity",
            1,
            Box::new(move |call| match arg_entity(&h.borrow(), call, 0) {
                Some(id) => vec_array(call, b.borrow().velocity(id)),
                None => Ok(slag::JsValue::undefined()),
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "is_grounded",
            1,
            Box::new(move |call| {
                let grounded =
                    arg_entity(&h.borrow(), call, 0).is_some_and(|id| b.borrow().is_grounded(id));
                Ok(slag::JsValue::boolean(grounded))
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "load_scene",
            1,
            Box::new(move |call| {
                if let Some(path) = arg_string(call, 0) {
                    b.borrow_mut().load_scene(path);
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "teleport_scene",
            2,
            Box::new(move |call| {
                if let (Some(path), Some(carry)) = (arg_string(call, 0), arg_string(call, 1)) {
                    b.borrow_mut().teleport_scene(path, carry);
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "viewport_size",
            0,
            Box::new(move |call| {
                let v = b.borrow().input.viewport();
                pair_array(call, v[0], v[1])
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "set_mouse_captured",
            1,
            Box::new(move |call| {
                if let Some(captured) = arg_bool(call, 0) {
                    b.borrow_mut().set_mouse_captured(captured);
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "is_overlapping",
            2,
            Box::new(move |call| {
                let overlapping = match (
                    arg_entity(&h.borrow(), call, 0),
                    arg_entity(&h.borrow(), call, 1),
                ) {
                    (Some(first), Some(second)) => b.borrow().is_overlapping(first, second),
                    _ => false,
                };
                Ok(slag::JsValue::boolean(overlapping))
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

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "has_component",
            2,
            Box::new(move |call| {
                let has = match (arg_entity(&h.borrow(), call, 0), arg_string(call, 1)) {
                    (Some(id), Some(name)) => b.borrow().has_component(id, &name),
                    _ => false,
                };
                Ok(slag::JsValue::boolean(has))
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "get_component",
            2,
            Box::new(move |call| {
                // A JSON string, or `""` when the entity or component is absent.
                let json = match (arg_entity(&h.borrow(), call, 0), arg_string(call, 1)) {
                    (Some(id), Some(name)) => b.borrow().component_json(id, &name),
                    _ => None,
                };
                Ok(slag::JsValue::string(json.unwrap_or_default()))
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "set_component",
            3,
            Box::new(move |call| {
                if let (Some(id), Some(name), Some(json)) = (
                    arg_entity(&h.borrow(), call, 0),
                    arg_string(call, 1),
                    arg_string(call, 2),
                ) {
                    b.borrow_mut().set_component(id, &name, json);
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        let h = Rc::clone(&self.handles);
        self.register(
            "remove_component",
            2,
            Box::new(move |call| {
                if let (Some(id), Some(name)) =
                    (arg_entity(&h.borrow(), call, 0), arg_string(call, 1))
                {
                    b.borrow_mut().remove_component(id, &name);
                }
                Ok(slag::JsValue::undefined())
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

fn arg_bool(call: &slag::FunctionCall<'_>, index: usize) -> Option<bool> {
    call.arg(index).and_then(|v| v.as_boolean())
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

/// A local vector as a JavaScript array, `[x, y, z]`.
fn vec_array(call: &slag::FunctionCall<'_>, v: Vec3) -> Result<slag::JsValue, slag::JsError> {
    call.create_array(&[
        slag::JsValue::number(v.x as f64),
        slag::JsValue::number(v.y as f64),
        slag::JsValue::number(v.z as f64),
    ])
}

/// A 2-vector (pointer delta/position, viewport size) as `[a, b]`.
fn pair_array(
    call: &slag::FunctionCall<'_>,
    a: f32,
    b: f32,
) -> Result<slag::JsValue, slag::JsError> {
    call.create_array(&[
        slag::JsValue::number(a as f64),
        slag::JsValue::number(b as f64),
    ])
}

impl ScriptHost for SlagHost {
    fn eval_script(&mut self, source: &str) -> ModuleId {
        let id = self.next_module;
        self.next_module += 1;
        self.reflect
            .insert(id, crate::bridge::source_reads_components(source));
        let program = self.module_program(id, source);
        if let Err(err) = self.context.eval(&program) {
            self.errors.push(format!("module {id}: {err}"));
        }
        id
    }

    fn call_lifecycle(&mut self, module: ModuleId, func: &str, ctx: &mut ScriptContext<'_>) {
        let reflect = self.reflect.get(&module).copied().unwrap_or(true);
        self.bridge.borrow_mut().begin(ctx, reflect);
        self.handles.borrow_mut().clear();

        // Reach the lifecycle function through the module's namespace object
        // rather than `eval`ing `globalThis.__noctis_ns_N.func` on every call:
        // the parser allocated ~40 times per call, per entity, per frame — the
        // bulk of the wasm stutter. A property get uses the engine's caches and
        // keeps the callee reachable from the global object, so it stays valid
        // across the call's GC (no handle rooting needed here).
        let callee = self
            .context
            .global()
            .ok()
            .and_then(|global| global.get(&format!("__noctis_ns_{module}")).ok())
            .and_then(|namespace| namespace.as_object())
            .and_then(|namespace| namespace.get(func).ok());

        if let Some(callee) = callee
            && callee.is_callable()
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

    #[test]
    fn mouse_reads_reach_a_real_script() {
        const MOUSE: &str = "
            function on_update(entity, dt) {
                var m = mouse_move();
                log('dx=' + m[0] + ' arr=' + Array.isArray(m) + ' scroll=' + mouse_scroll());
            }
        ";
        let host = SlagHost::new().expect("slag engine starts");
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Script::from_source(MOUSE));

        // Moving the cursor from the origin adds its position to this frame's delta.
        runtime.input_mut().set_mouse_position(50.0, 0.0);
        runtime.run_update(&mut world, &assets, 0.016);

        assert!(
            runtime.host().errors().is_empty(),
            "unexpected script errors: {:?}",
            runtime.host().errors()
        );
        assert!(
            runtime.command_log().iter().any(|c| matches!(
                c, ScriptCommand::Log { message, .. } if message.contains("dx=50")
            )),
            "mouse_move did not reach the script: {:?}",
            runtime.command_log()
        );
    }

    #[test]
    fn vector_getters_return_arrays() {
        const GETTERS: &str = "
            function on_update(entity, dt) {
                var p = get_position(entity);
                log('isArray=' + Array.isArray(p) + ' len=' + p.length +
                    ' x=' + p[0] + ' y=' + p[1] + ' z=' + p[2]);
            }
        ";
        let host = SlagHost::new().expect("slag engine starts");
        let mut runtime = ScriptRuntime::with_host(host);
        let assets = AssetRegistry::new();
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            LocalTransform {
                translation: vec3(1.0, 2.0, 3.0),
                ..Default::default()
            },
        );
        world.insert(entity, Script::from_source(GETTERS));

        runtime.run_update(&mut world, &assets, 0.016);

        assert!(
            runtime.host().errors().is_empty(),
            "unexpected script errors: {:?}",
            runtime.host().errors()
        );
        assert!(
            runtime.command_log().iter().any(|c| matches!(
                c, ScriptCommand::Log { message, .. }
                    if message.contains("isArray=true")
                        && message.contains("len=3")
                        && message.contains("x=1")
                        && message.contains("y=2")
                        && message.contains("z=3")
            )),
            "get_position did not yield an array: {:?}",
            runtime.command_log()
        );
    }

    #[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize, ecs::Component)]
    struct Health {
        current: f32,
        max: f32,
    }

    fn health_runtime() -> ScriptRuntime<SlagHost> {
        let host = SlagHost::new().expect("slag engine starts");
        let mut runtime = ScriptRuntime::with_host(host);
        let mut registry = ecs::reflect::ComponentRegistry::new();
        registry.register::<Health>();
        runtime.set_components(registry);
        runtime
    }

    const REFLECT_SOURCE: &str = "
        function on_update(entity, dt) {
            var health = JSON.parse(get_component(entity, 'Health'));
            health.current = health.current + 5;
            set_component(entity, 'Health', JSON.stringify(health));
        }
    ";

    #[test]
    fn a_script_reads_and_writes_a_registered_component() {
        let mut runtime = health_runtime();
        let assets = AssetRegistry::new();
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            Health {
                current: 10.0,
                max: 20.0,
            },
        );
        world.insert(entity, Script::from_source(REFLECT_SOURCE));

        runtime.run_update(&mut world, &assets, 0.016);

        assert_eq!(
            world.get::<Health>(entity),
            Some(&Health {
                current: 15.0,
                max: 20.0
            })
        );
        assert!(
            runtime.host().errors().is_empty(),
            "unexpected script errors: {:?}",
            runtime.host().errors()
        );
    }

    #[test]
    fn a_script_removes_a_registered_component() {
        let mut runtime = health_runtime();
        let assets = AssetRegistry::new();
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(
            entity,
            Health {
                current: 1.0,
                max: 2.0,
            },
        );
        world.insert(
            entity,
            Script::from_source(
                "function on_update(entity, dt) { if (has_component(entity, 'Health')) { remove_component(entity, 'Health'); } }",
            ),
        );

        runtime.run_update(&mut world, &assets, 0.016);

        assert!(world.get::<Health>(entity).is_none());
        assert!(
            runtime.host().errors().is_empty(),
            "unexpected script errors: {:?}",
            runtime.host().errors()
        );
    }
}
