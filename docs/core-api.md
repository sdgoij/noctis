# Noctis core API contract

This document is the frozen interface for the `math` and `ecs` crates. The
`render`, `physics`, `audio`, and `script` crates build on it. **Do not change
these crates to fit a consumer** — implement consumers against this contract.

Crate names are `math` and `ecs` (no prefix). Edition 2024, Rust 1.98.

## `math`

Types (all `f32`):

- `Vec2 { x, y }`, `Vec3 { x, y, z }`, `Vec4 { x, y, z, w }` — `glm` types.
- `Mat4` — column-major, `#[repr(C)]`, fields `c0, c1, c2, c3: Vec4`.
  Supports `Index<usize> -> &Vec4`, `as_array() -> &[Vector4<f32>; 4]`,
  `Mat4 * Mat4`, `Mat4 * Vec4`.
- `Quat { x, y, z, w }` — `#[repr(C)]`, custom (glm has no quaternion).
- `Transform { translation: Vec3, rotation: Quat, scale: Vec3 }`.
- `Aabb { min: Vec3, max: Vec3 }`.

Functions:

- Constructors: `vec2`, `vec3`, `vec4`, `zero3`, `one3`, `unit_x/y/z`.
- Matrices: `identity()`, `translation(Vec3)`, `scaling(Vec3)`, `rotation_x/y/z(f32)`,
  `from_quat(Quat) -> Mat4`, `trs(Vec3, Quat, Vec3) -> Mat4`, `from_cols`,
  `perspective(fov_y, aspect, near, far)`, `orthographic(l, r, b, t, near, far)`,
  `look_at_rh(eye, center, up)`, `look_at_lh(..)`,
  `inverse(&Mat4) -> Option<Mat4>`, `transpose(&Mat4) -> Mat4`,
  `to_cols_array(&Mat4) -> [[f32;4];4]`, `from_cols_array([[f32;4];4]) -> Mat4`,
  `trs_to_cols_array(Vec3, Quat, Vec3) -> [[f32;4];4]`.
- Scalars: `radians`, `degrees`, `lerp`, `clamp`, `approx_eq`, `vec3_approx_eq`.
- Re-exported from glm: `dot`, `cross`, `normalize`, `length`, `distance`.

`Quat` methods: `identity`, `new`, `from_axis_angle(axis, angle)`,
`from_rotation_x/y/z`, `from_euler_xyz(roll, pitch, yaw)`, `to_euler_xyz`,
`mul` / `*`, `rotate_vec3` / `* Vec3`, `slerp`, `normalized`, `conjugate`,
`inverse`, `dot`, `look_rotation(forward, up)`.

`Transform` methods: `identity`, `from_translation/rotation/scale`,
`with_translation/rotation/scale`, `to_matrix() -> Mat4`,
`to_cols_array() -> [[f32;4];4]`, `transform_point(Vec3)`,
`transform_vector(Vec3)`, `inverse() -> Option<Transform>` (exact for uniform
scale), `inverse_matrix() -> Option<Mat4>` (exact for any scale), `mul` / `*`,
`lerp(&Transform, t)`.

`Aabb` methods: `new`, `from_center_half_extents`, `from_points(&[Vec3])`,
`is_empty`, `center`, `size`, `half_extents`, `corners() -> [Vec3; 8]`,
`contains_point`, `intersects`, `expand_point`, `expand_box`, `union`,
`expanded(f32)`.

## `ecs`

### World & entities

```rust
use ecs::world::{World, Entity};

world.spawn() -> Entity
world.spawn_with<C>(c) -> Entity
world.despawn(e) -> bool
world.is_alive(e) -> bool
world.insert::<C>(e, c) -> Option<C>
world.get::<C>(e) -> Option<&C>
world.get_mut::<C>(e) -> Option<&mut C>
world.remove::<C>(e) -> Option<C>
world.has::<C>(e) -> bool
world.count::<C>() -> usize
world.query::<C>() -> impl Iterator<Item = (Entity, &C)>
world.query_mut::<C>() -> impl Iterator<Item = (Entity, &mut C)>
world.query_with::<A, B>() -> impl Iterator<Item = (Entity, &mut B)>   // entities with A and B
world.entities_with::<C>() -> Vec<Entity>
world.entities() -> &Entities   // .len(), .is_empty(), .iter() -> Entity
```

`Entity`: `Copy + Eq + Ord + Hash`, `index() -> u32`, `generation() -> u32`,
`from_raw(index, generation)`.

Component types must be `Send + Sync + 'static`.

### Components (`ecs::components`)

- `LocalTransform` — alias of `math::Transform`.
- `GlobalTransform { matrix: Mat4 }` — `position() -> Vec3`, `to_matrix() -> Mat4`,
  `from_local(&Transform)`, `GlobalTransform::compose(parent, &child_local)`.
- `Name(String)`, `Active(bool)`, `Visible(bool)`, `Parent(Entity)`, `Children(Vec<Entity>)`.
- `Camera { projection: Projection, is_active: bool, priority: i32, clear_color: [f32;4], hdr: bool }`.
- `Projection::Perspective { fov_y: f32, near: f32, far: f32 } | Orthographic { height: f32, near: f32, far: f32 }`
  — `.near()`, `.far()`.
- `Light { kind: LightKind, color: Vec3, intensity: f32, cast_shadows: bool, shadow_bias: f32 }`.
- `LightKind::Directional { direction: Vec3 } | Point { range: f32 } | Spot { direction: Vec3, inner_angle: f32, outer_angle: f32, range: f32 }`.
- `MeshRenderer { mesh: AssetHandle<Mesh>, material: AssetHandle<Material>, cast_shadows: bool, receive_shadows: bool }`.
- `RigidBody { body_type: BodyType, mass, restitution, friction, gravity_scale, linear_velocity: Vec3, angular_velocity: Vec3, linear_damping, angular_damping, ccd: bool }`.
- `BodyType::Dynamic | Fixed | Kinematic`.
- `Collider { shape: ColliderShape, offset: Vec3, rotation: Quat, is_sensor: bool, friction, restitution }`.
- `ColliderShape::Cuboid { half_extents: Vec3 } | Ball { radius: f32 } | Capsule { half_height, radius } | Cylinder { half_height, radius }`.
- `AudioSource { clip: AssetHandle<AudioClip>, volume, pitch, looped, playing, spatial, max_distance, rolloff }`.
- `Script { source: Option<String>, path: Option<String>, enabled: bool }` — `from_source`, `from_path`, `has_code`.
- `Text { text, font: AssetHandle<FontAsset>, size, color, anchor, depth_test }` —
  `new`, `Default`; in-world billboarded text (not UI). `TextAnchor`
  (`Center` (default), `TopLeft`/`TopCenter`/`TopRight`, `MiddleLeft`/`MiddleRight`,
  `BottomLeft`/`BottomCenter`/`BottomRight`) places the label relative to the
  entity. `size` is the line height in world units; glyphs are rasterized on
  demand by the renderer.

### Custom components (`ecs::reflect`)

The built-in components are a closed set; a consumer may register its own so they
round-trip through a scene like a built-in. `World` already stores any
`Send + Sync + 'static` type — only *reflection* is added. See
[`docs/embedding.md`](embedding.md).

- `Component` — for a registered type: `const NAME: &'static str`, and
  `Send + Sync + 'static + Serialize + DeserializeOwned + Default`. Optional
  `fn fields() -> Vec<Field>` (the editor/reflection schema) and
  `fn references(&self) -> Vec<String>` (asset references for the bundler).
- `Field { name: &'static str, kind: FieldKind }`; `FieldKind::Number | Integer |
  Boolean | Text | Vec3 | Color | Asset | Enum(&'static [&'static str])`, with
  `Field::number/integer/boolean/text/vec3/color/asset/enumeration` builders.
- `ComponentRegistry` — `new()`, `register::<C>()`, `has(name)`, `names()` (sorted),
  `fields(name)`, `insert_default`, `has_on`, `serialize`, `deserialize`, `remove`,
  `serialize_entity(world, entity)` (sorted — the save path), `references_of(name,
  data)`. An unknown name is an error, never a panic.
- `Scene { name, mode, world, components: ComponentRegistry }` — the scene carries a
  registry snapshot; `Scene::with_component_registry` sets it and
  `Scene::from_*_with` load one in.
- `Scene::from_json_with` / `from_desc_with` / `try_from_desc_with`,
  `instantiate_into_with`, and `Prefab::instantiate_with` take a
  `&ComponentRegistry`; the plain variants use an empty registry (built-ins only).
- `SceneDesc` gains `ComponentDesc::Custom { name: String, data: serde_json::Value }`
  (serialized `{ "type": "custom", "name": ..., "data": ... }`), and
  `SceneError::UnknownComponent(String)` reports a `Custom` entry whose type is not
  registered.

### Assets (`ecs::assets`)

- `AssetId(u32)` — `NONE`, `is_none()`, `Copy + Eq + Ord + Hash`.
- `AssetHandle<T>` — `Copy + Eq + Hash + Debug + Default`, `NONE`, `new(AssetId)`,
  `from_raw(u32)`, `id() -> AssetId`, `is_none() -> bool`.
- `Assets<T>` — `new`, `insert(value) -> AssetHandle<T>`,
  `insert_named(path, value) -> AssetHandle<T>` (replacing keeps the id),
  `get(handle) -> Option<&T>`, `get_shared(handle) -> Option<Arc<T>>`,
  `handle(path: &str) -> Option<AssetHandle<T>>`, `get_by_path`,
  `path_of(handle) -> Option<&str>`, `remove(handle) -> bool`, `len`, `is_empty`,
  `iter() -> impl Iterator<Item = (AssetId, Option<&str>, &T)>`.
- `AssetRegistry { meshes, materials, textures, audio, scripts, fonts, hdr }` — each
  the corresponding `Assets<_>`; `new()`, `with_defaults()` (registers `$white`,
  `$normal` textures and a `$default` material),
  `with_builtins()` (the defaults plus the six primitive meshes under
  `primitives/<name>`; use it when loading scene documents so a built-in mesh
  always resolves), `install_primitives()`, `len()`, `is_empty()`.
- `ecs::primitives` — procedural mesh generators (`cube`, `plane`, `sphere`,
  `cylinder`, `capsule`, `torus`), each returning a `Mesh` with positions,
  normals, UVs, tangents, indices, and bounds.
- `ecs::scene::ComponentDesc` includes `Text { text, font, size, color, anchor: TextAnchorDesc, depth_test }`; a `Text` font reference is validated at load like a mesh or clip.
- `Vertex { position: [f32;3], normal: [f32;3], uv: [f32;2], tangent: [f32;4] }` (`#[repr(C)]`, `Pod`-friendly).
- `Mesh { name: String, vertices: Vec<Vertex>, indices: Vec<u32>, bounds: Option<Aabb> }`
  — `compute_bounds()`, `triangle_count()`.
- `Material { name, base_color: [f32;4], metallic: f32, roughness: f32, emissive: [f32;3], emissive_strength: f32, base_color_texture, normal_texture, metallic_roughness_texture, emissive_texture, occlusion_texture: AssetHandle<Texture>, alpha_mode: AlphaMode, double_sided: bool }`.
- `AlphaMode::Opaque | Mask | Blend`.
- `Texture { name, width: u32, height: u32, format: TextureFormat, data: Vec<u8> }`
  — `solid(name, [u8;4], format)`; `TextureFormat::Rgba8Srgb | Rgba8Unorm`.
- `HdrTexture { name, width, height, data: Vec<f32> }` — linear RGB, `width*height*3` floats (equirectangular).
- `AudioClip { name, sample_rate: u32, channels: u16, samples: Vec<f32> }` — `duration()`.
- `ScriptAsset { name, source }`.
- `FontAsset { name, data: Vec<u8> }` — a font file's bytes; `is_font_data(&[u8])`
  recognises TrueType/OpenType signatures. Glyph rasterization (an atlas) is not
  built yet.

### Resources & app

- `ecs::resources`: `Time` (`advance`, `step_fixed`, `delta`, `elapsed`, `frame`,
  `fixed_delta`, `scale`, `set_scale`, `interpolation_alpha`),
  `InputState` (`press_key`, `release_key`, `key_down/pressed/released`, `begin_frame`,
  `press_mouse`, `release_mouse`, `set_mouse_position`, `add_scroll`, `axis`, `mouse`),
  `Key`, `MouseButton`, `MouseState`, `ActionMap`, `Events<E>`.
- `ecs::app`: `App` (`new`, `on_startup/on_fixed/on_update/on_late`, `tick(dt)`,
  `run_headless(max_frames, dt)`, `context`, `fixed_steps_taken`),
  `EngineContext { scene, assets, time, input, actions, events, exit_requested }`,
  `EngineEvent`.
  Systems run on the thread that owns the `App`, in registration order, so they
  are **not** required to be `Send`/`Sync` — a system may own single-threaded
  state (e.g. a JavaScript engine). Component *types* are still
  `Send + Sync + 'static`.
- `ecs::scene`: `Scene { name, mode, world }`, `SceneMode { TwoD, ThreeD }`
  (`as_str`, `is_2d`, `FromStr`; the presentation hint stored in `SceneDesc.mode`),
  `Scene::with_mode`, `Scene::try_from_desc` (errors on a missing asset
  reference).
  `propagate_transforms(&mut World)`,
  `set_parent(&mut World, child, Option<Entity>)`, `children_of(&World, Entity)`,
  `instantiate_into(&mut World, &SceneDesc, &AssetRegistry, Option<Entity>)` (the
  lenient primitive; `from_json`/`try_from_desc` are the strict load paths),
  `Prefab`, `SceneDesc`, `SceneError { Parse, Version, MissingAsset }`.
- `ecs::prelude::*` re-exports all of the above plus `math`.

## Conventions for consumer crates

- `#![forbid(unsafe_code)]` at the crate root unless a dependency makes it
  impossible (it does not for wgpu/rapier/rodio).
- Module-level `//!` docs and `///` docs on public items.
- rustfmt `max_width = 100` (see `rustfmt.toml`).
- Unit tests in `#[cfg(test)] mod tests` in every module that has logic.
- Every crate must pass `cargo test -p <crate>` and `cargo build -p <crate>`.
