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
use std::collections::HashMap;
use std::rc::Rc;

use ecs::components::{LocalTransform, Name};
use ecs::resources::{InputState, Key, MouseButton};
use math::{Quat, Vec3, vec3};

use crate::api::{EntityId, ScriptApi, ScriptCommand, ScriptContext, entity_id};
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

/// The generation stamped on placeholder handles handed to JavaScript for
/// entities spawned during a call. Chosen so it can never collide with a real
/// entity's generation.
const PLACEHOLDER_GENERATION: u32 = u32::MAX;

fn placeholder_id(slot: u64) -> EntityId {
    ((PLACEHOLDER_GENERATION as u64) << 32) | slot
}

/// Shared, single-threaded state the registered host functions read and write.
#[derive(Default)]
struct Bridge {
    entity: Option<EntityId>,
    delta: f32,
    elapsed: f32,
    input: InputState,
    positions: HashMap<EntityId, Vec3>,
    rotations: HashMap<EntityId, Vec3>,
    scales: HashMap<EntityId, Vec3>,
    names: HashMap<String, EntityId>,
    next_placeholder: u64,
    commands: Vec<ScriptCommand>,
}

impl Bridge {
    fn begin(&mut self, ctx: &ScriptContext<'_>) {
        self.entity = ctx.entity;
        self.delta = ctx.delta;
        self.elapsed = ctx.elapsed;
        self.input = ctx.input.clone();
        self.positions.clear();
        self.rotations.clear();
        self.scales.clear();
        self.names.clear();
        self.next_placeholder = 0;
        self.commands.clear();

        for entity in ctx.world.entities().iter() {
            let id = entity_id(entity);
            if let Some(name) = ctx.world.get::<Name>(entity) {
                self.names.insert(name.0.clone(), id);
            }
            if let Some(t) = ctx.world.get::<LocalTransform>(entity) {
                self.positions.insert(id, t.translation);
                self.rotations.insert(id, euler(t.rotation));
                self.scales.insert(id, t.scale);
            }
        }
    }

    fn spawn(&mut self, name: Option<String>) -> EntityId {
        let id = placeholder_id(self.next_placeholder);
        self.next_placeholder += 1;
        if let Some(name) = &name {
            self.names.insert(name.clone(), id);
        }
        self.positions.insert(id, vec3(0.0, 0.0, 0.0));
        self.rotations.insert(id, vec3(0.0, 0.0, 0.0));
        self.scales.insert(id, vec3(1.0, 1.0, 1.0));
        self.commands
            .push(ScriptCommand::Spawn { name, entity: id });
        id
    }

    fn position(&self, id: EntityId) -> Vec3 {
        self.positions
            .get(&id)
            .copied()
            .unwrap_or_else(|| vec3(0.0, 0.0, 0.0))
    }

    fn set_position(&mut self, id: EntityId, position: Vec3) {
        self.positions.insert(id, position);
        self.commands.push(ScriptCommand::SetPosition {
            entity: id,
            position,
        });
    }

    fn translate(&mut self, id: EntityId, delta: Vec3) {
        let next = self.position(id) + delta;
        self.positions.insert(id, next);
        self.commands
            .push(ScriptCommand::Translate { entity: id, delta });
    }

    fn rotation(&self, id: EntityId) -> Vec3 {
        self.rotations
            .get(&id)
            .copied()
            .unwrap_or_else(|| vec3(0.0, 0.0, 0.0))
    }

    fn set_rotation(&mut self, id: EntityId, euler: Vec3) {
        self.rotations.insert(id, euler);
        self.commands
            .push(ScriptCommand::SetRotationEuler { entity: id, euler });
    }

    fn scale(&self, id: EntityId) -> Vec3 {
        self.scales
            .get(&id)
            .copied()
            .unwrap_or_else(|| vec3(1.0, 1.0, 1.0))
    }

    fn set_scale(&mut self, id: EntityId, scale: Vec3) {
        self.scales.insert(id, scale);
        self.commands
            .push(ScriptCommand::SetScale { entity: id, scale });
    }

    fn find(&self, name: &str) -> Option<EntityId> {
        self.names.get(name).copied()
    }
}

fn euler(q: Quat) -> Vec3 {
    let (roll, pitch, yaw) = q.to_euler_xyz();
    vec3(roll, pitch, yaw)
}

/// Replay a script call's queued commands against the real world, translating
/// placeholder handles spawned during the call to the entities they became.
fn replay(ctx: &mut ScriptContext<'_>, commands: Vec<ScriptCommand>) {
    fn resolve(map: &HashMap<EntityId, EntityId>, id: EntityId) -> EntityId {
        map.get(&id).copied().unwrap_or(id)
    }

    let mut map: HashMap<EntityId, EntityId> = HashMap::new();
    let mut api = ScriptApi::new(ctx);
    for command in commands {
        match command {
            ScriptCommand::Log { message, .. } => api.log(message),
            ScriptCommand::Spawn { name, entity } => {
                let real = api.spawn(name);
                map.insert(entity, real);
            }
            ScriptCommand::Destroy { entity } => api.destroy(resolve(&map, entity)),
            ScriptCommand::DestroyByName { name } => api.destroy_by_name(&name),
            ScriptCommand::SetPosition { entity, position } => {
                api.set_position(resolve(&map, entity), position);
            }
            ScriptCommand::Translate { entity, delta } => {
                api.translate(resolve(&map, entity), delta);
            }
            ScriptCommand::SetRotationEuler { entity, euler } => {
                api.set_rotation_euler(resolve(&map, entity), euler);
            }
            ScriptCommand::SetScale { entity, scale } => {
                api.set_scale(resolve(&map, entity), scale);
            }
            ScriptCommand::AddScript { entity, source } => {
                api.add_script(resolve(&map, entity), source);
            }
            ScriptCommand::InstantiatePrefab { json, parent, .. } => {
                let parent = parent.map(|p| resolve(&map, p));
                api.instantiate_prefab(&json, parent);
            }
        }
    }
}

/// The Slag-backed [`ScriptHost`].
pub struct SlagHost {
    context: slag::Context,
    bridge: Rc<RefCell<Bridge>>,
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
        self.register(
            "spawn",
            1,
            Box::new(move |call| {
                let name = arg_name(call, 0);
                let id = b.borrow_mut().spawn(name);
                Ok(slag::JsValue::number(id as f64))
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "destroy",
            1,
            Box::new(move |call| {
                if let Some(id) = arg_entity(call, 0) {
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
        self.register(
            "find_entity",
            1,
            Box::new(move |call| {
                let found = arg_string(call, 0).and_then(|name| b.borrow().find(&name));
                Ok(match found {
                    Some(id) => slag::JsValue::number(id as f64),
                    None => slag::JsValue::undefined(),
                })
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "get_position",
            1,
            Box::new(move |call| {
                let out = arg_entity(call, 0).map(|id| vec_json(b.borrow().position(id)));
                Ok(out.map_or_else(slag::JsValue::undefined, slag::JsValue::string))
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "set_position",
            4,
            Box::new(move |call| {
                if let (Some(id), Some(p)) = (arg_entity(call, 0), arg_vec3(call, 1)) {
                    b.borrow_mut().set_position(id, p);
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "translate",
            4,
            Box::new(move |call| {
                if let (Some(id), Some(d)) = (arg_entity(call, 0), arg_vec3(call, 1)) {
                    b.borrow_mut().translate(id, d);
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "get_rotation_euler",
            1,
            Box::new(move |call| {
                let out = arg_entity(call, 0).map(|id| vec_json(b.borrow().rotation(id)));
                Ok(out.map_or_else(slag::JsValue::undefined, slag::JsValue::string))
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "set_rotation_euler",
            4,
            Box::new(move |call| {
                if let (Some(id), Some(e)) = (arg_entity(call, 0), arg_vec3(call, 1)) {
                    b.borrow_mut().set_rotation(id, e);
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "get_scale",
            1,
            Box::new(move |call| {
                let out = arg_entity(call, 0).map(|id| vec_json(b.borrow().scale(id)));
                Ok(out.map_or_else(slag::JsValue::undefined, slag::JsValue::string))
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "set_scale",
            4,
            Box::new(move |call| {
                if let (Some(id), Some(s)) = (arg_entity(call, 0), arg_vec3(call, 1)) {
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
            "add_script",
            2,
            Box::new(move |call| {
                if let (Some(id), Some(source)) = (arg_entity(call, 0), arg_string(call, 1)) {
                    b.borrow_mut()
                        .commands
                        .push(ScriptCommand::AddScript { entity: id, source });
                }
                Ok(slag::JsValue::undefined())
            }),
        )?;

        let b = Rc::clone(&self.bridge);
        self.register(
            "instantiate_prefab",
            2,
            Box::new(move |call| {
                if let Some(json) = arg_string(call, 0) {
                    let parent = arg_entity(call, 1);
                    b.borrow_mut()
                        .commands
                        .push(ScriptCommand::InstantiatePrefab {
                            json,
                            parent,
                            spawned: Vec::new(),
                        });
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

fn arg_entity(call: &slag::FunctionCall<'_>, index: usize) -> Option<EntityId> {
    call.arg(index)
        .and_then(|v| v.as_number())
        .map(|n| n as u64)
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

        let function = format!("globalThis.__noctis_ns_{module}.{func}");
        if let Ok(callee) = self.context.eval(&function)
            && !callee.is_null()
            && !callee.is_undefined()
            && let Some(entity) = ctx.entity
        {
            let args = [
                slag::JsValue::number(entity as f64),
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
    use ecs::components::{LocalTransform, Script};
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
