# Reflections

The port's water is a flat, translucent plane with a view-distance gradient
([`.notes/goats.md`](goats.md) M5). The original's water *reflects*, in three
tiers, and that is the last M5 gap. This note is the contract for the engine
feature the top tier needs — a **user render target** ([`.notes/engine-gaps.md`](engine-gaps.md),
"User render targets") — and the record of the two cheaper tiers the port can
reach without it.

It is the sibling of [`shaders.md`](shaders.md): the engine owns the pass, the
bind groups and the layouts; the game supplies a plane and samples the result.

**Shipped:** tiers 0 and 1 in the goats water (`WATER_WGSL`): a fresnel, a ripple
normal from the wave field, a reflection of the sky (the shared `sky_colour`), and
a march against the ground baked from the terrain grid (`terrain::bake_reflection`,
re-baked with the ground on a crater). **Also shipped: tier 2's engine pass** —
the `PlanarReflection` component and the mirrored scene render
(`crates/render/src/reflection.rs` for the maths, `crates/render/src/gpu/reflection.rs`
for the target), with a GPU test that drives a receiver's shader; the port's next
step is to put the component on the goats water. **Not yet:** billboards are left
out of the pass ("What the pass draws").

## What the original does

`lighting.js`'s water fragment shader computes a fresnel and a reflected ray,
then answers "what does the reflection see?" three ways (`TUNING.water.reflection`):

- **Tier 0 — sky only.** `sky = waterSky * (0.55 + 0.45 * clamp(refl.y, 0, 1))`.
  No sampling: a horizon-dimmed sky colour, mixed in by the fresnel. This is what
  M20b–M20d shipped.
- **Tier 1 — the baked ground.** The terrain's own grid, baked to one texture
  (its vertex colour in `rgb`, the ground's height packed in `a`, one texel per
  grid vertex), rebuilt with the terrain. The fragment marches the reflected ray
  against it and keeps the sky wherever the ray escapes. Reflects the ground and
  the sky, and *not* the goat — deterministic, and no pass.
- **Tier 2 — the raised mirror.** The world is rendered from the camera the table
  mirrors the player's into, into a render target (`waterMirrorRT`), which the
  shader samples **at the fragment's own screen position** — the reflected ray
  leaves the same pixel it entered, so this is `texture(mirror, gl_FragCoord.xy /
  screen)`, mixed by the pass's own alpha (a ray that met nothing shows the sky,
  not the clear colour). This reflects the whole scene, the goat included.

The fresnel is the one cue the tiers share:

```
ndv  = max(dot(n, viewDir), 0)
fres = fresnel + (1 - fresnel) * (1 - ndv)^5
color = mix(color, sky, fres)
```

## The tiers, and what each needs

| Tier | Reflects | Needs |
|---|---|---|
| 0 — sky | the sky | nothing: one shader (the seam's mesh form) |
| 1 — ground bake | ground + sky | nothing: a `Texture` the goats side bakes from its own terrain grid |
| 2 — mirror | the whole scene | **the engine pass below** |

Tier 0 is a small change to the existing water shader and the biggest single
"wet" cue. Tier 1 is a goats-side bake: the port already has the terrain's grid
and per-vertex biome colour, so it is the original's bake with no new engine
surface. Tier 2 is the engine feature.

## The engine feature: a planar reflection pass

### The component

```rust
pub struct PlanarReflection {
    /// A point on the mirror plane (the water's level, in world space).
    pub point: Vec3,
    /// The plane's normal (unit; the water's is +Y).
    pub normal: Vec3,
    /// The target's size as a fraction of the frame (0.5 = half res).
    pub resolution: f32,
    /// Metres the clip plane is nudged above `point`, so the surface itself is
    /// not reflected into itself.
    pub clip_bias: f32,
    pub enabled: bool,
}
```

On the **receiver** entity (the water's `ShaderMeshRenderer`). The engine mirrors
the primary camera about the plane, renders the scene into an offscreen texture,
and hands that texture to the receiver's shader. Like the shadow map, it is
**one primary reflection** — the first enabled `PlanarReflection` — because a
frame pays a whole extra scene pass for it ([`primary_shadow_light`](../../crates/render/src/lights.rs)
is the precedent).

### The pass

1. **Mirror the view.** `view' = view · reflect(plane)` — the world is reflected
   about the plane, so the mirrored camera sees what the surface would.
2. **Oblique clip.** The near plane is tilted onto the mirror plane (the
   classic oblique-projection trick, as a plane in view space), so geometry
   *below* the water does not leak into the reflection.
3. **Render the scene** — the sky, opaque and seam meshes — into an offscreen
   colour+depth target at `resolution` of the frame (taken by the receiver's own
   entity). The **receiver is skipped**
   (the water must not reflect itself), by the same rule the shadow pass uses to
   skip a caster: an entity with the component is not drawn into its own pass.
   The **sky** runs first, as it does in the main pass, so the reflected ray that
   escapes geometry sees the sky.

A pass per frame is the cost, and it is the whole reason the original made it a
tier: the setting can fall back to tier 0/1 on a weak device.

### The sampler

The seam's WGSL prelude gains three bindings on **group 0** — the frame's shared
resources, which are already per-pass:

```
struct ReflectionUniform { params: vec4<f32>, };
@group(0) @binding(10) var reflection: texture_2d<f32>;
@group(0) @binding(11) var reflection_sampler: sampler;
@group(0) @binding(12) var<uniform> reflection_params: ReflectionUniform; // screen w, h, clip_bias, enabled
```

Every seam shader gets the bindings; one that does not reflect ignores them. A
receiver's fragment samples them at its own `@builtin(position).xy / params.xy` —
the same arithmetic as the original — and mixes by the pass's alpha (a ray that met
nothing shows the sky, not the clear colour).

The note this replaces proposed a fourth bind group (`@group(4)`). It is group 0
instead: a WebGPU device is only *guaranteed* four bind groups, so a fifth would
have to be requested as a raised limit, and the browser target could refuse it —
whereas group 0 is already the frame's shared, per-pass bundle, which is exactly
the reflection's granularity. The pass builds its **own** group 0 (its camera is
the mirrored one), pointing these bindings at a 1x1 white texture: a pass must not
sample the texture it writes, and a reflection wants no part of the frame's
ambient occlusion. The frame's group 0 holds the real target, which the receiver's
draw binds. Nothing else moves — the PBR pipelines declare group 0 too and simply
ignore the three entries.

**Alternative considered:** a reflection *cubemap* probe (render six faces from a
point, sample by the reflected direction). Rejected for the water: a flat plane
is exactly a single reflected *view*, so a planar pass is one image instead of
six and is exact where a cubemap is not. A cubemap earns its place for a rough
or curved reflector, which the port has none of yet — record it, do not build it.

## Extraction and the draw list

The pass calls the *same* pure extraction, `extract_draw_list_filtered`, with the
mirrored `CameraView` and one exclusion (the receiver). A second call, not a second
implementation — and the cull has to be redone, because what a surface reflects is
mostly what the primary camera cannot see. The frame and the reflection then share
one object-uniform ring: the frame's draws take the first slots, the reflection's
the next.

## What the pass draws

The sky (the frame's screen passes, drawn first so a reflected ray that meets no
geometry sees the sky), opaque PBR meshes, and the seam's mesh draws (opaque and
blended). The receiver is excluded, and the near plane is the surface, so nothing
beneath it is drawn.

**Not yet drawn:** transparent PBR meshes, world-space text, gizmos, and
**billboards** — the last because billboard quads are built CPU-side against the
primary camera, so a reflection needs a second per-frame build, which is real work
rather than a line. The port's scenes use meshes, so nothing looks wrong yet.
Recorded here rather than hidden.

## Tests

- **Pure** (`crates/render/src/reflection.rs`): the mirror matrix fixes its plane
  and flips the rest, and the mirrored view reflects the eye and the basis; the
  oblique clip is a re-parameterised near plane (a plane parallel to the view
  reproduces `perspective(.., d, ..)` exactly), puts a point on the plane exactly on
  the new near plane, keeps the kept side in front and clips the camera side, and
  degenerates to the untouched projection for a singular matrix or a plane sitting
  on the far corner; a point on the plane projects to the same pixel from either
  view; `reflection_view` looks out of the surface with the world beneath it
  clipped; the first enabled component wins; the clip bias lifts the plane the pass
  cuts at.
- **Extraction** (`crates/render/src/draw_list.rs`): an excluded PBR mesh, shader
  mesh or screen pass drops out of its list while its neighbours stay, and an
  exclusion is not a cull.
- **GPU** (`crates/render/src/renderer.rs`): a receiver's shader samples the pass,
  and the frame shows a green quad that stands *behind* the camera — visible in the
  reflection alone, which proves both the mirrored pass and its own cull — with the
  disabled flag turning the same fragment red. It runs once single-sampled and once
  under MSAA (behind the `NoAdapter` skip).

## What the port does meanwhile

1. **Tier 0 — shipped.** The water's seam shader computes the fresnel and the
   reflected ray, and evaluates the sky the ray leaves into (the shared
   `sky_colour`, so the two can never disagree). No sampling and no engine change.
2. **Tier 1 — shipped.** The ground is baked into a texture with the terrain
   (`terrain::bake_reflection`, one texel per grid vertex, colour in `rgb`, a
   packed height in `a`), and the fragment marches the reflected ray against it,
   keeping the sky wherever the ray escapes.
3. **Tier 2, engine side — shipped.** A `PlanarReflection` on a receiver with a
   seam shader turns the pass on. What remains is the port's own step: put the
   component on the goats water at the table's level and have `WATER_WGSL` sample
   the group-0 reflection instead of (or over) tiers 0 and 1.

## Settled, and still open

Settled by the implementation, for the record:

- **Where the receiver's exclusion lives.** The rule: an entity carrying the
  component is not drawn into its own pass, exactly as the shadow pass skips a
  caster. The pass names it in `ExtractFilter::exclude`. A second component
  (`NotReflected`) remains the escape hatch if a scene ever wants one.
- **The target's format.** Linear `rgba16float`, the frame's own HDR format, so the
  reflection is tonemapped with the frame. `resolution` is the cost knob.
- **Half-resolution and MSAA.** The target takes the frame's sample count (its own
  multisampled colour and depth, resolving into the colour the receiver samples),
  because the pipelines are built for that count and would otherwise be rejected.
  A GPU test pins both cases.
- **Sky in the mirror.** Drawn in the pass, as the original does.

Still open:

- **A cubemap probe** for a future rough/curved reflector, noted above.
- **Draw counters.** The harness's draw-side checks want a way to count what a pass
  drew; the reflection pass is one more caller of that seam when it lands
  ([`.notes/harness.md`](harness.md)).
