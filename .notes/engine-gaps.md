# Engine features the goats port still needs

The port leans on the engine, and the engine does not yet do everything the
original — or its look — needs. This is the running list of what is missing, what
the port wants it for, and what it does meanwhile. It complements
[`.notes/goats.md`](goats.md) ("The engine features this needs") and
[`.notes/shaders.md`](shaders.md).

## Rendering

### User render targets (reflection probes)

*What:* a way for a shader to sample the scene from another view — or for the
engine to precompute a reflection cubemap from the scene.
*For:* the water's sky / ground / mirror reflections (goats.md's capability map).
*Meanwhile:* the water is a flat translucent plane with a view-distance gradient;
no reflections.
*Designed:* [`.notes/reflections.md`](reflections.md) — a **planar reflection
pass** (`PlanarReflection` component, a mirrored scene pass into an offscreen
texture, and the seam's reflection binding the receiver samples). All three tiers
are reachable now: tiers 0 (sky) and 1 (a baked ground texture) are **shipped** in
the goats water and needed no engine change, tier 2 — the pass — is **shipped in
the engine**, and the goats water is its receiver (the component tracks the table's
level; the shader samples the pass as its tier 2).

### Scene depth in the shader contract

*What:* the depth prepass the engine already runs, exposed to a seam shader as a
depth texture (and its linearised value).
*For:* depth-varying water (deep vs shallow where the ground is far), soft
water/terrain intersections, and depth fog.
*Meanwhile:* the water's colour varies only with view distance; the terrain's depth
test alone confines it to the pools.

### Time, sun and weather in the frame uniform

*What:* the frame uniform carries `time`, the sun's direction / colour and the
weather, so every shader agrees without each repeating them.
*For:* the sky's day/night and the weather's grey-out, and anything else that should
follow the clock (a wet sheen, the grass's sway).
*Shipped, partly:* the **ambient** is drivable per frame — a plugin publishes
`EngineContext::frame_ambient` and the runtime applies it to the renderer, so the
frame uniform's `ambient` (which the PBR pass *and* every seam shader already read)
follows a clock (`crates/ecs/src/app.rs`, `crates/runtime/src/runtime.rs`). The sun
needs no seam: it is a scene `Light`, and a plugin driving that component reaches the
PBR pass and the shadow map at once. *Still open:* `time`, the sun's direction and
the weather in the uniform itself — the goats port keeps them in each material's own
`user` block, which works but repeats the sun per shader.

### A billboard / particle path

*Resolved (kept for the record).* The **billboard quad** shipped
([`.notes/billboards.md`](billboards.md)): `ecs::components::Billboard` and the
batched pass, first used by the goats bang's fire, smoke, grit and the mine
"tell" (`3033dd5`). Still open: an engine `ParticleEmitter` (deferred until a
second consumer proves it), cylindrical and soft billboards, and the water's
wake and splashes.

### Skin and shadows in a custom-shader mesh

*What:* a seam mesh form that can be skinned, and a shadow-sampling helper.
*For:* a custom-shaded skinned goat, and custom meshes that receive the engine's
shadows without each reimplementing the PCF loop.
*Meanwhile:* the goat uses the engine's skinned PBR path (now with vertex colour);
the seam's mesh form is unskinned and must sample `shadow_map` itself (the prelude
exposes it) if it wants shadows.

## UI (M6)

### The screen-UI seam

*Shipped.* `ecs::ui::Ui` — an egui-free widget trait (`panel`, `label`, `button`,
`toggle`, `slider`, `combo`, `text_field`) — and `EngineContext::ui`, a callback a
plugin installs. `platform::EguiUi` implements the trait over the host's egui and
the runtime bridges them; `ecs::ui::RecordingUi` drives a menu in a headless test.
Designed in [`.notes/ui.md`](ui.md).
*For:* the main / settings / mods / keymap menus and the console (goats M6).
*Meanwhile (before):* `screen_text` HUD lines only.

## Tooling

### A draw-count seam

*What:* counters (draws, meshes, instances) the harness can read.
*For:* the harness's draw-side cases (`.notes/harness.md`).
*Meanwhile:* the harness asserts on state, not pixels.

## Resolved (kept for the record)

- **Vertex colour** — shipped: `Vertex::color` + `Material::vertex_colors`, and the
  terrain uses it.
- **Skeletal animation** — shipped ([`.notes/animation.md`](animation.md)).
- **The shader seam** — shipped ([`.notes/shaders.md`](shaders.md)); both forms are
  drawn (the sky and the water).
