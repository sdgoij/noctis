# The billboard path

The engine's renderer draws meshes and text. A ported game sometimes needs a
**camera-facing textured quad** — a sprite — that is not a mesh and not a glyph:
goats wants a bang's fire, smoke and debris, a mine "tell", and the water's wake
and splashes ([`.notes/goats.md`](goats.md), M5). Today the only billboarded
thing the engine draws is `ecs::components::Text`, and it is font-specific. This
note is the contract for the general path.

It is the sibling of [`shaders.md`](shaders.md): the engine owns the pipeline,
the vertex layout and the pass; the game supplies a texture handle and per-quad
numbers. Nothing here is a plugin-defined pipeline.

## What it is

One **component**: a `Billboard` — a camera-facing textured quad drawn at the
entity's world position. One entity, one quad; a burst is several entities (or a
pool the game keeps and reuses), not a new engine concept.

The engine gains:

- **`ecs::components::Billboard`** — the quad's data (below).
- **`render::billboard`** — a pure `build_vertices` that turns a billboard and
  the camera basis into four [`BillboardVertex`]s; unit-tested without a GPU,
  exactly as `render::text::build_vertices` is.
- **A billboard pass** in `renderer.rs`, batched by `(texture, depth_test,
  blend)` and drawn after the opaque and shader passes, like the text pass.

This is deliberately **not** a particle simulation. The engine draws a quad at a
position; who moves it, how long it lives, and how it fades is the game's
business — goats already runs `apply_frame` every frame and can drive a small
pool. A future `ParticleEmitter` (an engine-side integrator) is an open question,
not part of this note.

## The `Billboard` component

```rust
pub struct Billboard {
    /// The sprite's texture (a `Texture` asset, sRGB or linear).
    pub texture: AssetHandle<Texture>,
    /// World-space size in metres (width, height).
    pub size: Vec2,
    /// Linear RGBA tint, multiplied into the texture.
    pub color: [f32; 4],
    /// An in-plane rotation, radians (a spinning ember or debris).
    pub rotation: f32,
    /// The sub-rectangle of the texture to sample, `[u0, v0, u1, v1]`
    /// (an atlas frame; the whole texture is `[0, 0, 1, 1]`).
    pub uv: [f32; 4],
    /// Alpha, additive, or opaque.
    pub blend: BillboardBlend,
    /// Whether the quad is depth-tested (against the scene) or drawn over it.
    pub depth_test: bool,
}
```

- **Position** comes from the entity's `GlobalTransform`/`LocalTransform`, as it
  does for `Text` — a billboard needs no new transform story.
- **`uv`** is the atlas frame, so one `Texture` holds a sprite sheet and a burst
  picks frames, exactly as a font atlas serves glyphs.
- **`size` + `rotation`** are enough for fire, smoke, debris and a splash;
  anisotropic scale or per-corner offsets (`size2`) are an open question.

`BillboardBlend { Opaque, Alpha, Additive }` chooses the pipeline's blend state.
Fire and embers are additive; smoke and splashes alpha; a mine "tell" is alpha.
An sRGB texture with `Additive` still needs the sRGB-to-linear on sample, so the
pipeline picks the format from the texture, as the PBR pass does.

### Serialization

`Billboard` is registered like any component: a `ComponentDesc` variant and
`component_descs`/`apply_component` in `ecs::scene`, so a scene can place a
**static** billboard — the mine "tell", a hand-placed torch flame. Following the
shader seam, it stays **out of `ComponentKind::ALL`** until a second consumer
proves it a first-class scene type; `SceneDesc` can still name it explicitly.

## The pass

The billboard pass mirrors the text pass and adds no new render target of its
own:

- **Batching.** Group this frame's billboards by `(texture, depth_test, blend)`
  into one vertex buffer each, as `build_text_batches` groups by
  `(font, depth_test)`. A burst with one atlas is one batch.
- **Bindings.** Group 0 is the existing camera/view bind group (shared with the
  text pass); group 1 is the quad's texture and the shared sampler. No new
  layout.
- **Order.** Drawn after the opaque, shader and text passes, into the same HDR
  target, so it composites last (sprites over the world). Back-to-front within a
  batch by view distance for correct alpha; the buffer is sorted once at build.
- **Depth.** `depth_test` reuses the text pass's two pipelines-per-setting idea;
  `depth_test: false` is the over-everything case (a HUD-ish flash, a muzzle
  flash), `true` is the in-world case (smoke behind a hill is hidden).
- **Lighting.** Unlit. A sprite outputs `texture * color`, tone-mapped with the
  frame; fire and smoke are emissive looks, not PBR surfaces.

## The vertex format and WGSL

`BillboardVertex { position: [f32; 3], uv: [f32; 2], color: [f32; 4] }` — the
text vertex plus alpha, so tint and fade are per-quad. A single built-in WGSL
shader (no seam needed): `vs_main` passes the vertex through `view_projection`,
`fs_main` samples the texture and multiplies the tint, returning linear HDR.

The quad is built in world space against the camera basis
(`view.right` / `view.up`) rotated by `billboard.rotation`, so a billboard always
faces the camera — the same construction `render::text::build_vertices` already
uses. A **cylindrical** billboard (yaw-only, for a standing flame that should not
lean back when looked at from above) is an open question below.

## What this buys, concretely

With the quad path in place, goats' M5 bang is ordinary work on top of it:

1. **fire** — a few additive frames from a fire atlas, spawned at the blast,
   shrinking and fading over ~0.4 s;
2. **smoke** — alpha frames rising and expanding for ~1.5 s;
3. **debris** — six to ten small dark quads on a short ballistic arc;
4. **the mine "tell"** — one faint alpha quad over an armed mine (a spark or a
   disturbed-turf sprite), or a static scene billboard;
5. **the water's wake and splash** — alpha quads at a wading goat's feet and at
   an impact.

The goats plugin owns the movement and lifetime; the engine owns the quad.

## Tests

- `render::billboard` — `build_vertices` is a pure function: a billboard facing
  the camera emits four vertices at the entity's position, the tint and atlas
  `uv` are carried through, and `rotation` spins the corners. GPU-free.
- `render` extraction/batching — billboards group by `(texture, depth_test,
  blend)` and are skipped when hidden, like the text batches. GPU-free where
  possible.
- GPU: a billboard pass draws a half-transparent tinted quad over a clear and the
  read-back pixel is the blend of the two — behind the existing `NoAdapter` skip.
- The harness's draw-count seam (`.notes/harness.md`) gains a billboard count
  when that seam lands; until then the goats harness asserts on the `Billboard`
  entities it can see in the world.

## Open questions

- **Engine emitter or game-driven quads?** This note starts with the primitive:
  the game drives a pool. If fire, smoke, debris, splash and wake all re-derive
  the same integrate-fade-die loop, an engine `ParticleEmitter` earns its place —
  the same "prove it twice" rule the shader seam used. Record it, do not build it
  yet.
- **Cylindrical vs spherical billboards.** Fire standing on a slope reads better
  yaw-only; a `BillboardMode { Spherical, Cylindrical }` is a small addition once
  a case needs it. Start spherical.
- **Sorting by distance, not depth.** Sprites within one batch sort by the
  entity's view distance; two intersecting bursts can pop. Per-vertex depth
  sorting or an OIT buffer is out of scope; note it.
- **Soft particles.** A sprite meeting the ground hard-cuts; a depth-fade would
  want the scene depth the seam does not yet expose (`.notes/engine-gaps.md`).
  Note it, do not build it here.
- **Where `Billboard` lives in `ComponentKind`.** Authorable in a scene, but out
  of `ALL` until a second consumer, per the seam precedent.
