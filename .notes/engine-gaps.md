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
*Meanwhile:* each material carries its own `user` block, and the goats code writes
the sky's sun each frame — the sky animates, but nothing else can read that sun.

### A billboard / particle path

*What:* camera-facing textured quads (a sprite), or a small particle system.
*For:* the bang's fire, smoke and debris, the mine "tell", and the water's wake and
splashes (goats.md M5). `ecs::components::Text` is billboarded, but text-specific.
*Meanwhile:* none — the bang has no look beyond the crater it dishes into the
ground.

### Skin and shadows in a custom-shader mesh

*What:* a seam mesh form that can be skinned, and a shadow-sampling helper.
*For:* a custom-shaded skinned goat, and custom meshes that receive the engine's
shadows without each reimplementing the PCF loop.
*Meanwhile:* the goat uses the engine's skinned PBR path (now with vertex colour);
the seam's mesh form is unskinned and must sample `shadow_map` itself (the prelude
exposes it) if it wants shadows.

## UI (M6)

### The screen-UI seam

*What:* an egui-free `Ui` trait in `ecs` and a host implementation, so a plugin can
draw panels (goats.md, "The screen-UI seam").
*For:* the main / settings / mods / keymap menus and the console.
*Meanwhile:* `screen_text` HUD lines only.

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
