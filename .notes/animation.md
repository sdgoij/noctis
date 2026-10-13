# Skeletal animation (glTF)

How the engine plays a **skinned, animated glTF model**: the rig, the clips, and
the per-frame pose that deforms a mesh. This document is the contract for the
animation data model, the render path, and the `Animator` / `SkinnedMeshRenderer`
components; update it deliberately when the design changes.

Background: glTF import is **static** today. `render::load_gltf_bytes`
(`crates/render/src/gltf_loader.rs`) reads meshes, materials, textures and the node
hierarchy into `LoadedGltf`, and `LoadedGltf::instantiate` spawns one entity per
node (with a `MeshRenderer` child for each mesh primitive). It ignores `skins`,
`animations`, and the `JOINTS_0` / `WEIGHTS_0` vertex attributes. This note adds
them. The first consumer is the goats port ([`.notes/goats.md`](goats.md)), whose
goat is a 15-bone rig with fourteen baked clips.

## Decisions

1. **GPU skinning, with one joint matrix buffer per drawn instance.** The vertex
   shader blends four weighted joint matrices read from a storage buffer, so the
   CPU never rewrites vertex positions. (The goat's own `PERF.md` measured CPU
   skinning as the frame's bottleneck; per-instance buffers are what let a shared
   mesh serve a whole herd.)
2. **Animation is data plus a component, not a rendering special case.** Clips and
   skins are [`AssetRegistry`] assets; `Animator` is a scene-authorable, reflectable
   component like every other, with an editor form and a script binding.
3. **The skeleton is the node hierarchy the engine already builds.** A glTF node
   becomes an entity with a `LocalTransform` and a `Parent` link; a joint is just
   such a node. No second, parallel skeleton representation is introduced.
4. **Non-deform joints are kept.** glTF exported with `export_def_bones=False`
   (the goat does this) carries bones with no skin weights — the goat's `Root`
   drives the jump arc. The joint list is "the nodes the skin names", not "the
   nodes with weights".
5. **The animator writes `LocalTransform`s and nothing else.** Sampling a clip
   writes each joint entity's local transform; the existing
   `propagate_transforms` pass (`ecs::scene`) composes worlds, and extraction
   reads them. Playback therefore composes with the ECS instead of bypassing it.

## The data model

Two new asset kinds, beside `Mesh` / `Material` / `Texture` in
`crates/ecs/src/assets.rs`:

```rust
/// A skin: the joints that deform a mesh, and their bind-time inverse
/// transforms.
pub struct Skin {
    pub name: String,
    /// The joint node indices, in skin order (glTF `skin.joints`).
    pub joints: Vec<usize>,
    /// `inverse(mesh_node_world) * joint_world`, at bind time, per joint.
    pub inverse_bind: Vec<Mat4>,
}

/// One animation, as channels of TRS keyframes targeting nodes.
pub struct AnimationClip {
    pub name: String,
    pub duration: f32,
    pub channels: Vec<AnimationChannel>,
}

pub struct AnimationChannel {
    /// The node this channel animates.
    pub node: usize,
    pub property: AnimationProperty, // Translation | Rotation | Scale
    /// Keyframe times, ascending.
    pub times: Vec<f32>,
    /// The value per time: `[x, y, z, _]` for T/S, `[x, y, z, w]` for R.
    pub values: Vec<[f32; 4]>,
}
```

The per-vertex influence — up to four joints and their weights — is **not** on
the `Skin` but on the `Mesh`, as `Mesh::influences: Vec<VertexInfluence>`,
parallel to `vertices` and empty for an unskinned mesh. glTF stores it as a
primitive attribute (`JOINTS_0`/`WEIGHTS_0`) and one skin can serve several
primitives, so it belongs with the geometry the vertex list describes (the goat
is one skin over five primitives).

The registry gains `animations: Assets<AnimationClip>` and `skins: Assets<Skin>`
(`AssetRegistry`, `crates/ecs/src/assets.rs`). A clip registers under its name, so
"play `GoatRun`" is `assets.animations.handle("GoatRun")` — the analogue of the
goat's `modelAnimationName`.

Two components (`crates/ecs/src/components.rs`), both in `ComponentKind`,
`ComponentDesc` and the reflection registry:

- **`SkinnedMeshRenderer`** — the skinned twin of `MeshRenderer`:

  ```rust
  pub struct SkinnedMeshRenderer {
      pub mesh: AssetHandle<Mesh>,
      pub skin: AssetHandle<Skin>,
      pub material: AssetHandle<Material>,
      pub cast_shadows: bool,
      pub receive_shadows: bool,
  }
  ```

- **`Animator`** — playback state:

  ```rust
  pub struct Animator {
      pub clip: AssetHandle<AnimationClip>,
      pub time: f32,
      pub speed: f32,
      pub looped: bool,
      pub playing: bool,
  }
  ```

The joint *entities* are a structural, runtime-resolved link (like `Parent` /
`Children`): the loader resolves a skin's node indices to the entities
`instantiate` just spawned and stores them in a non-serialized `Skeleton` marker
on the skinned entity, so a saved scene stays free of raw entity ids and rebinds
on load. A script or the editor addresses the animator by clip name, not by joint.

## The frame

Skinning needs joint **world** matrices, which the engine already produces. The
order inside `App::tick` (`crates/ecs/src/app.rs`) is:

1. **`animator` system** (engine-installed, in the `update` stage, ahead of
   plugin systems): advance `Animator::time`, sample each channel at `time`
   (linear for translation/scale, shortest-path `slerp` for rotation, step when a
   channel has no interpolation), and write the sampled TRS into the target joint
   entities' `LocalTransform`s.
2. **`propagate_transforms`** (already in `App::tick`): composes `GlobalTransform`
   for every joint.
3. **Extraction** (`render`): for each `SkinnedMeshRenderer`, read its `Skeleton`'s
   joint `GlobalTransform`s, multiply by `mesh_world⁻¹` and the bind-time
   `inverse_bind`, and push `joint_count` `Mat4`s into that instance's slot of a
   joint-matrix storage buffer. The instance's draw item carries the buffer offset
   and the `SkinnedMeshRenderer` flag.
4. **Forward and shadow passes**: a skinned pipeline variant blends the four
   weighted matrices from the storage buffer before the model transform. Both
   passes use it, so a goat casts its own shadow with the animated pose.

A clip's loop wraps `time` into `[0, duration)`; a one-shot holds on the last
frame and clears `playing`. Clip selection is a handle swap on `Animator::clip`.

## glTF import

`build_loaded` (`crates/render/src/gltf_loader.rs`) gains two reads; `LoadedGltf`
gains `skins` and `animations`, and `NodeDesc` gains `skin` (the node's `skin`
index):

- **Skins** — `document.skins()`: the joint node indices and the
  `inverse_bind_matrices` accessor, verbatim, into a `Skin`. The per-vertex
  `JOINTS_0` / `WEIGHTS_0` accessors are read where primitives are built and
  stored on the mesh (`Mesh::influences`, four influences per vertex); a mesh
  with no weights gets an empty influence list.
- **Animations** — `document.animations()`: each animation becomes an
  `AnimationClip` named after the glTF animation, its channels resolved from the
  input/output accessors. Interpolation (`LINEAR` / `STEP` / `CUBICSPLINE`) is
  recorded per channel; `CUBICSPLINE` is sampled as `LINEAR` until it earns its
  own path.

`LoadedGltf::instantiate` attaches a `SkinnedMeshRenderer` (not a `MeshRenderer`)
to the geometry child of a skinned node and fills the `Skeleton` from the node
entities, and an `Animator` on the same child when the model carries clips, set to
the first clip paused — a loaded model animates the moment a system plays a clip,
and does nothing until then.

## Editor and scene format

- The importer attaches the clips; the **Animator** inspector shows a clip picker
  (the model's clip names), a time scrub, play/pause, speed and loop — enough to
  check a rig without leaving the editor.
- `SkinnedMeshRenderer` gets a mesh/skin/material picker like `MeshRenderer`'s.
- `ComponentKind`, `ComponentDesc`, `component_descs` / `apply_component` and the
  reflection registry all gain the two components (the same checklist as any
  component, `AGENTS.md`).
- A custom component that references a skin clip is an ordinary asset reference;
  the bundler (`project::referenced_files_with`) needs nothing new beyond carrying
  `.glb` files it already carries.

## Testing

- `crates/render`: import a small skinned glTF fixture (two bones, one triangle),
  assert the skin, the clip and the sampled pose; assert the joint matrices of a
  known pose.
- `crates/ecs`: `Animator` advances and loops deterministically; a script can set
  `Animator::clip` (reflection round-trip).
- `crates/testscene`: an animated model present, with a coverage assertion that
  its joints move between two frames (the `testscene` spine).
- GPU tests skip gracefully with no adapter, per doctrine.

## Rejected alternatives

- **CPU skinning as the default.** It rewrites every vertex host-side each frame;
  the goat's `PERF.md` measured it as the frame's cost. Keep it only as a
  degenerate path for a mesh with no skin buffer, never as the design.
- **Baking each clip frame into a mesh.** Memory and upload grow with clip length
  and instance count, and it cannot blend.
- **A parallel skeleton outside the ECS.** Two sources of truth for the same node
  transforms; writing `LocalTransform`s lets propagation, picking, the inspector
  and hierarchy all see the animated pose.
- **Decoding clips in the scripting layer.** Clips are per-frame data for every
  instance; they belong on the render path, not in JS.

## What already exists

- `gltf` is already a dependency and drives `load_gltf_bytes`; `document.skins()`
  and `document.animations()` are the same crate's API.
- `LoadedGltf::register` / `instantiate` are the seams the new fields thread
  through; `NodeDesc.parent` is the hierarchy the skeleton reuses.
- `AssetRegistry` (`meshes` / `materials` / `textures` / …) is the store to
  extend; `Assets::insert_named` keys a clip by its name.
- The render extraction / `DrawItem` (`crates/render/src/draw_list.rs`) and the
  forward + shadow pipelines (`crates/render/src/gpu/pipelines.rs`) are where the
  skinned vertex path and the joint storage buffer attach.
- `propagate_transforms` already gives joints their world matrices; the animator
  only writes locals.
- `MeshRenderer` is the template for `SkinnedMeshRenderer`; the component
  checklist in `AGENTS.md` is the path through the registry, the scene format and
  the test scene.

## Phasing

1. **Import.** *(Shipped.)* `Skin` / `AnimationClip` assets, the `skin`/
   `animations`/`JOINTS_0`/`WEIGHTS_0` reads, the two registry stores, and a
   fixture test (`the_skin_and_clip_import`). No playback: the model still draws
   at bind pose.
2. **Skinned draw.** *(Not built.)* `SkinnedMeshRenderer`, the joint storage
   buffer, the skinned vertex pipeline and its shadow-pass twin.
3. **Playback.** *(Not built.)* `Animator`, the `update`-stage sampler, loop and
   one-shot, the editor form, and the `testscene` joint-motion assertion.
4. **Fidelity.** *(Not built.)* `CUBICSPLINE`, cross-fade between clips, additive
   or masked layers, root-motion extraction — only as a consumer needs it.

## Open questions

- **Edge in the texture binary.** `.glb` files reach the engine as loose files
  today; the goat embeds its textures in the `.glb`, so nothing new is needed —
  but a project that ships an animated model in a **release** build must carry
  the `.glb` loose until models join the embedded-script pack (see
  [`.notes/scripts.md`](scripts.md)).
- **Joint buffer growth.** One `Mat4` per joint per instance; a herd of ten goats
  at fifteen bones is 150 matrices a frame. A single storage buffer with a free
  list, or a fixed cap, are both fine — measure before choosing.
- **Blend trees vs. clip switches.** The goat hard-switches clips with its own
  timing; a cross-fade API is probably the first writer's need, not an engine
  requirement yet.
- **Skin without a mesh node transform.** glTF allows the mesh node and the
  skeleton root to differ; the bind-time `mesh_world⁻¹` term covers it, but it
  needs a fixture to prove.
