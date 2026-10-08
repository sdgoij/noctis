//! Light extraction and the GPU light uniform block.
//!
//! Lights live on entities as the [`Light`] component, optionally paired with a
//! [`GlobalTransform`]. [`extract_lights`] is a pure function that flattens the
//! world into [`ExtractedLight`] values with resolved world positions and
//! normalised directions. [`LightsUniform`] packs a fixed-size array of lights
//! for the forward pass.
//!
//! The shadow-matrix helpers build the light-space view-projection matrices used
//! by the shadow pass: an orthographic box for directional lights and a
//! perspective frustum for spots.

use bytemuck::{Pod, Zeroable};
use ecs::components::{GlobalTransform, Light, LightKind, LocalTransform};
use ecs::world::{Entity, World};
use math::{Mat4, Vec3, vec3};

/// The maximum number of dynamic lights uploaded per frame.
pub const MAX_LIGHTS: usize = 16;

/// The kind of a light, encoded as the `x` component of a GPU light's params.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum LightType {
    /// Directional (sun) light.
    Directional = 0,
    /// Omnidirectional point light.
    Point = 1,
    /// Cone-shaped spot light.
    Spot = 2,
}

/// A light resolved into world space and ready for shading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExtractedLight {
    /// The owning entity.
    pub entity: Entity,
    /// The light shape.
    pub kind: LightKind,
    /// World-space position (zero for directional lights).
    pub position: Vec3,
    /// Unit direction the light travels in (towards the scene).
    pub direction: Vec3,
    /// Linear RGB color.
    pub color: Vec3,
    /// Intensity.
    pub intensity: f32,
    /// Whether the light casts shadows.
    pub cast_shadows: bool,
    /// Shadow map bias.
    pub shadow_bias: f32,
    /// Falloff range (point/spot); ignored for directional lights.
    pub range: f32,
}

impl ExtractedLight {
    /// The numeric type tag.
    pub fn light_type(&self) -> LightType {
        match self.kind {
            LightKind::Directional { .. } => LightType::Directional,
            LightKind::Point { .. } => LightType::Point,
            LightKind::Spot { .. } => LightType::Spot,
        }
    }

    /// The cone inner/outer angle cosines for spot lights, or `None` otherwise.
    pub fn spot_angles(&self) -> Option<(f32, f32)> {
        match self.kind {
            LightKind::Spot {
                inner_angle,
                outer_angle,
                ..
            } => Some((inner_angle.cos(), outer_angle.cos())),
            _ => None,
        }
    }
}

fn normalized_or(v: Vec3, fallback: Vec3) -> Vec3 {
    let len_sq = v.x * v.x + v.y * v.y + v.z * v.z;
    if len_sq > 1e-12 {
        let inv = len_sq.sqrt().recip();
        vec3(v.x * inv, v.y * inv, v.z * inv)
    } else {
        fallback
    }
}

fn world_position(world: &World, entity: Entity) -> Vec3 {
    if let Some(global) = world.get::<GlobalTransform>(entity) {
        global.position()
    } else if let Some(local) = world.get::<LocalTransform>(entity) {
        local.translation
    } else {
        vec3(0.0, 0.0, 0.0)
    }
}

/// Flatten every [`Light`] in the world into an [`ExtractedLight`].
///
/// The order is the ECS query order, which is stable for a given world.
pub fn extract_lights(world: &World) -> Vec<ExtractedLight> {
    let mut out = Vec::new();
    for (entity, light) in world.query::<Light>() {
        let position = world_position(world, entity);
        let (direction, range) = match light.kind {
            LightKind::Directional { direction } => {
                (normalized_or(direction, vec3(0.0, -1.0, 0.0)), 0.0)
            }
            LightKind::Point { range } => (vec3(0.0, 0.0, 0.0), range),
            LightKind::Spot {
                direction, range, ..
            } => (normalized_or(direction, vec3(0.0, -1.0, 0.0)), range),
        };
        out.push(ExtractedLight {
            entity,
            kind: light.kind,
            position,
            direction,
            color: light.color,
            intensity: light.intensity,
            cast_shadows: light.cast_shadows,
            shadow_bias: light.shadow_bias,
            range,
        });
    }
    out
}

/// The index of the light that should own the primary shadow map: the first
/// shadow-casting directional light, falling back to the first shadow-casting
/// spot light.
pub fn primary_shadow_light(lights: &[ExtractedLight]) -> Option<usize> {
    if let Some(i) = lights
        .iter()
        .position(|l| l.cast_shadows && matches!(l.kind, LightKind::Directional { .. }))
    {
        return Some(i);
    }
    lights
        .iter()
        .position(|l| l.cast_shadows && matches!(l.kind, LightKind::Spot { .. }))
}

/// Build a light-space view-projection for a directional light shadow map.
///
/// The light eye is pushed back along the light direction so the whole sphere of
/// `radius` around `center` fits between the near and far planes. The
/// projection is a symmetric orthographic box of side `2 * radius`.
pub fn directional_shadow_matrix(
    direction: Vec3,
    center: Vec3,
    radius: f32,
    near: f32,
    far: f32,
) -> Mat4 {
    let dir = normalized_or(direction, vec3(0.0, -1.0, 0.0));
    let radius = radius.max(1e-3);
    let eye = center - dir * (radius + near);
    let up = if dir.y.abs() > 0.99 {
        vec3(0.0, 0.0, 1.0)
    } else {
        vec3(0.0, 1.0, 0.0)
    };
    let view = math::look_at_rh(eye, center, up);
    let projection = math::orthographic(-radius, radius, -radius, radius, near, far);
    projection * view
}

/// Build a light-space view-projection for a spot light shadow map.
pub fn spot_shadow_matrix(
    position: Vec3,
    direction: Vec3,
    outer_angle: f32,
    near: f32,
    far: f32,
) -> Mat4 {
    let dir = normalized_or(direction, vec3(0.0, -1.0, 0.0));
    let up = if dir.y.abs() > 0.99 {
        vec3(0.0, 0.0, 1.0)
    } else {
        vec3(0.0, 1.0, 0.0)
    };
    let view = math::look_at_rh(position, position + dir, up);
    // A small margin keeps shadow casters just outside the cone from clipping.
    let fov = (2.0 * outer_angle).clamp(1e-3, std::f32::consts::PI - 1e-3);
    let projection = math::perspective(fov, 1.0, near, far);
    projection * view
}

/// One light as uploaded to the GPU.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct LightData {
    /// `xyz` = world position, `w` = unused.
    pub position: [f32; 4],
    /// `xyz` = direction, `w` = unused.
    pub direction: [f32; 4],
    /// `rgb` = color, `a` = intensity.
    pub color_intensity: [f32; 4],
    /// `x` = type (see [`LightType`]), `y` = range, `z` = cos(inner),
    /// `w` = cos(outer).
    pub params: [f32; 4],
}

impl LightData {
    /// Pack an [`ExtractedLight`] for the GPU.
    pub fn from_light(light: &ExtractedLight) -> LightData {
        let (inner, outer) = light.spot_angles().unwrap_or((0.0, 0.0));
        LightData {
            position: [light.position.x, light.position.y, light.position.z, 0.0],
            direction: [light.direction.x, light.direction.y, light.direction.z, 0.0],
            color_intensity: [light.color.x, light.color.y, light.color.z, light.intensity],
            params: [light.light_type() as u32 as f32, light.range, inner, outer],
        }
    }
}

/// The GPU light array. `count.x` is the number of active lights.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct LightsUniform {
    /// `x` = number of lights, remaining components unused.
    pub count: [u32; 4],
    /// The light data, padded to [`MAX_LIGHTS`].
    pub lights: [LightData; MAX_LIGHTS],
}

impl Default for LightsUniform {
    fn default() -> Self {
        LightsUniform {
            count: [0, 0, 0, 0],
            lights: [LightData {
                position: [0.0; 4],
                direction: [0.0; 4],
                color_intensity: [0.0; 4],
                params: [0.0; 4],
            }; MAX_LIGHTS],
        }
    }
}

impl LightsUniform {
    /// Pack up to [`MAX_LIGHTS`] extracted lights, ignoring the rest.
    pub fn from_lights(lights: &[ExtractedLight]) -> LightsUniform {
        let mut uniform = LightsUniform::default();
        let n = lights.len().min(MAX_LIGHTS);
        for (slot, light) in uniform.lights.iter_mut().zip(lights.iter()).take(n) {
            *slot = LightData::from_light(light);
        }
        uniform.count[0] = n as u32;
        uniform
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ecs::components::{GlobalTransform, LocalTransform, Name};
    use math::{Quat, radians};

    #[test]
    fn extract_directional_normalises_direction() {
        let mut world = World::new();
        let e = world.spawn_with(Light::directional(vec3(0.0, -10.0, 0.0)));
        let lights = extract_lights(&world);
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].entity, e);
        assert!(math::vec3_approx_eq(
            lights[0].direction,
            vec3(0.0, -1.0, 0.0),
            1e-5
        ));
        assert_eq!(lights[0].light_type(), LightType::Directional);
    }

    #[test]
    fn extract_point_uses_translation() {
        let mut world = World::new();
        let e = world.spawn_with(ecs::components::Light::point(
            vec3(1.0, 0.5, 0.25),
            3.0,
            10.0,
        ));
        world.insert(
            e,
            GlobalTransform::from_local(&LocalTransform::from_translation(vec3(2.0, 3.0, 4.0))),
        );
        let lights = extract_lights(&world);
        assert!(math::vec3_approx_eq(
            lights[0].position,
            vec3(2.0, 3.0, 4.0),
            1e-5
        ));
        assert!((lights[0].range - 10.0).abs() < 1e-6);
        assert_eq!(lights[0].light_type(), LightType::Point);
    }

    #[test]
    fn spot_angles_are_cosines() {
        let mut world = World::new();
        let light = Light {
            kind: LightKind::Spot {
                direction: vec3(0.0, -1.0, 0.0),
                inner_angle: radians(20.0),
                outer_angle: radians(30.0),
                range: 25.0,
            },
            ..Light::default()
        };
        world.spawn_with(light);
        let lights = extract_lights(&world);
        let (inner, outer) = lights[0].spot_angles().unwrap();
        assert!((inner - radians(20.0).cos()).abs() < 1e-6);
        assert!((outer - radians(30.0).cos()).abs() < 1e-6);
        assert_eq!(lights[0].light_type(), LightType::Spot);
    }

    #[test]
    fn primary_shadow_light_prefers_directional() {
        let mut world = World::new();
        let spot = Light {
            kind: LightKind::Spot {
                direction: vec3(0.0, -1.0, 0.0),
                inner_angle: 0.1,
                outer_angle: 0.2,
                range: 10.0,
            },
            ..Light::default()
        };
        let spot_entity = world.spawn_with(Name::new("spot"));
        world.insert(spot_entity, spot);
        let dir = world.spawn_with(Light::directional(vec3(0.0, -1.0, 0.0)));
        let lights = extract_lights(&world);
        let idx = primary_shadow_light(&lights).expect("a shadow light");
        assert_eq!(lights[idx].entity, dir);
    }

    #[test]
    fn primary_shadow_light_none_when_no_shadow_casters() {
        let mut world = World::new();
        let mut light = Light::directional(vec3(0.0, -1.0, 0.0));
        light.cast_shadows = false;
        world.spawn_with(light);
        let lights = extract_lights(&world);
        assert!(primary_shadow_light(&lights).is_none());
    }

    #[test]
    fn lights_uniform_packs_and_clamps() {
        let mut world = World::new();
        for i in 0..(MAX_LIGHTS + 5) {
            world.spawn_with(Light {
                color: vec3(1.0, 0.0, 0.0),
                intensity: i as f32,
                ..Light::default()
            });
        }
        let lights = extract_lights(&world);
        let uniform = LightsUniform::from_lights(&lights);
        assert_eq!(uniform.count[0], MAX_LIGHTS as u32);
        assert!((uniform.lights[0].color_intensity[3] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn directional_shadow_matrix_keeps_center_in_clip() {
        let m =
            directional_shadow_matrix(vec3(0.0, -1.0, 0.0), vec3(0.0, 0.0, 0.0), 10.0, 0.1, 50.0);
        let p = m * math::vec4(0.0, 0.0, 0.0, 1.0);
        let ndc = vec3(p.x / p.w, p.y / p.w, p.z / p.w);
        assert!(ndc.x.abs() < 1e-3 && ndc.y.abs() < 1e-3, "ndc = {ndc:?}");
        assert!(ndc.z > 0.0 && ndc.z < 1.0, "z = {}", ndc.z);
    }

    #[test]
    fn directional_shadow_matrix_covers_radius() {
        let center = vec3(3.0, 1.0, -2.0);
        let m = directional_shadow_matrix(vec3(0.3, -1.0, 0.2), center, 5.0, 0.1, 50.0);
        for offset in [
            vec3(5.0, 0.0, 0.0),
            vec3(-5.0, 0.0, 0.0),
            vec3(0.0, 5.0, 0.0),
            vec3(0.0, 0.0, 5.0),
        ] {
            let p = m * math::vec4(
                center.x + offset.x,
                center.y + offset.y,
                center.z + offset.z,
                1.0,
            );
            assert!(p.x.abs() <= p.w + 1e-3, "x out of range");
            assert!(p.y.abs() <= p.w + 1e-3, "y out of range");
        }
    }

    #[test]
    fn spot_shadow_matrix_looks_down_the_cone() {
        let m = spot_shadow_matrix(
            vec3(0.0, 5.0, 0.0),
            vec3(0.0, -1.0, 0.0),
            radians(30.0),
            0.1,
            50.0,
        );
        let p = m * math::vec4(0.0, 0.0, 0.0, 1.0);
        assert!(p.z / p.w > 0.0 && p.z / p.w < 1.0);
    }

    #[test]
    fn uniform_is_pod_sized() {
        // 16 bytes header + 16 lights * 64 bytes.
        assert_eq!(std::mem::size_of::<LightsUniform>(), 16 + 16 * 64);
        assert_eq!(std::mem::size_of::<LightsUniform>() % 16, 0);
        let _ = Quat::identity();
    }
}
