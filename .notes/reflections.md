# Reflections

The port's water is a flat, translucent plane with a view-distance gradient
([`.notes/goats.md`](goats.md) M5). The original's water *reflects*, in three
tiers, and that is the last M5 gap. This note is the contract for the engine
feature the top tier needs — a **user render target** ([`.notes/engine-gaps.md`](engine-gaps.md),
"User render targets") — and the record of the two cheaper tiers the port can
reach without it.

It is the sibling of [`shaders.md`](shaders.md): the engine owns the pass, the
bind groups and the layouts; the game supplies a plane and samples the result.

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
   classic oblique-projection trick, as a plane in clip space), so geometry
   *below* the water does not leak into the reflection.
3. **Render the scene** — opaque, seam meshes and billboards — into an offscreen
   colour+depth target at `resolution` of the frame. The **receiver is skipped**
   (the water must not reflect itself), by the same rule the shadow pass uses to
   skip a caster: an entity with the component is not drawn into its own pass.
   The **sky** runs first, as it does in the main pass, so the reflected ray that
   escapes geometry sees the sky.

A pass per frame is the cost, and it is the whole reason the original made it a
tier: the setting can fall back to tier 0/1 on a weak device.

### The sampler

The seam's WGSL prelude gains a fourth group the engine fills (`shaders.md`
records the three it has):

```
@group(4) @binding(0) var reflection: texture_2d<f32>;
@group(4) @binding(1) var reflection_sampler: sampler;
@group(4) @binding(2) var<uniform> reflection_params: vec4<f32>; // screen w, h, clip_bias, enabled
```

Every seam shader gets the binding; one that does not reflect ignores it. A
receiver's fragment samples it at its own `@builtin(position).xy / screen` — the
same arithmetic as the original — and mixes by the pass's alpha. The frame's
screen size is already in `FrameUniform.resolution`, so `reflection_params` may
only need the enabled flag and the bias.

**Alternative considered:** a reflection *cubemap* probe (render six faces from a
point, sample by the reflected direction). Rejected for the water: a flat plane
is exactly a single reflected *view*, so a planar pass is one image instead of
six and is exact where a cubemap is not. A cubemap earns its place for a rough
or curved reflector, which the port has none of yet — record it, do not build it.

## Extraction and the draw list

The reflection pass reuses the frame's draw list: it filters out the receiver
entity and re-sorts by the mirrored `CameraView`. So `extract_draw_list` is the
same pure function with a different view and one exclusion — no second
extraction path.

## Tests

- **Pure:** the mirror matrix (`reflect_view(view, plane)`) maps the plane to
  itself and the camera to its mirror; the oblique clip keeps a point above the
  plane in front of the near plane and drops a point below it; the receiver is
  excluded from its own pass.
- **GPU:** the pass renders a known scene into the target and the receiver's
  fragment samples it — a read-back proves a coloured object appears in the
  reflection where the shader says it should (behind the `NoAdapter` skip).

## What the port does meanwhile

1. **Tier 0 now** — fresnel + the sky reflection, in the water's seam shader.
   The reflected ray is evaluated against the same sky the sky pass draws (the
   body is shared), so no sampling and no engine change. This is the port's
   current water plus the fresnel it lacks.
2. **Tier 1 next** — bake the ground into a texture with the terrain and march
   the reflected ray, as the original does. A `Texture` asset and shader work,
   still no engine change.
3. **Tier 2 when the pass lands** — the mirror, setting `PlanarReflection` on the
   water to the table's level.

## Open questions

- **Where the receiver's exclusion lives.** A rule (a component marks it) or an
  explicit entity on the reflection. The shadow pass's own-caster skip is the
  precedent to follow; a second component (`NotReflected`) is the escape hatch.
- **The target's format.** Linear `rgba16float` (the HDR target's own format) so
  the reflection is tonemapped with the frame, or an LDR target for half the
  bandwidth. The HDR target is the correct default; the half-res tier is the
  cost knob.
- **Half-resolution and MSAA.** At half res the reflection is softer and cheaper;
  whether it takes the frame's MSAA is a quality call the pass's own test pins.
- **Sky in the mirror.** The original's mirror pass draws the sky (a screen pass
  first). The port could instead evaluate the sky shader on the reflected ray and
  leave the sky out of the pass — cheaper, but then two sky implementations must
  agree. Draw it in the pass, as the original does.
- **A cubemap probe** for a future rough/curved reflector, noted above.
