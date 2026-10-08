//! # render — the Noctis PBR renderer
//!
//! A wgpu 30 renderer for the Noctis engine. It provides:
//!
//! * **glTF import** ([`gltf_loader`]) turning `.gltf`/`.glb` assets into
//!   [`ecs::assets::Mesh`], [`ecs::assets::Material`], and
//!   [`ecs::assets::Texture`] values plus a node hierarchy.
//! * A **metallic/roughness PBR** forward pipeline (Cook–Torrance GGX) written
//!   in WGSL.
//! * **Image-based lighting** ([`ibl`]) from an equirectangular HDR environment:
//!   diffuse irradiance, prefiltered specular, and a BRDF LUT, all precomputed
//!   on the CPU with the split-sum approximation.
//! * **Soft shadow maps** for directional and spot lights with PCF filtering
//!   ([`pcf`]).
//! * **Screen-space ambient occlusion** ([`ssao`]).
//! * An **HDR pipeline** (rgba16float) with tonemapping to sRGB ([`tonemap`]).
//! * **Editor gizmo** geometry ([`gizmo`]).
//! * A **scene renderer** ([`draw_list`]) that extracts, culls, and sorts the
//!   draw list from an [`ecs::world::World`].
//!
//! ## Testability
//!
//! Everything that does not require a GPU — frustum culling, draw-list sorting,
//! glTF parsing, tonemapping, IBL math, gizmo generation — is a pure function
//! with unit tests. GPU work is isolated behind [`renderer::Renderer`], whose
//! [`renderer::Renderer::headless`] constructor returns
//! [`RenderError::NoAdapter`] when no adapter exists so tests can skip cleanly.
//!
//! ```
//! use render::draw_list::extract_draw_list;
//! use render::camera::extract_camera;
//! use ecs::prelude::*;
//!
//! let mut world = World::new();
//! let camera = world.spawn_with(Camera::default());
//! let assets = AssetRegistry::with_defaults();
//! let view = extract_camera(&world, camera, 1280, 720).unwrap();
//! let list = extract_draw_list(&world, &assets, &view);
//! assert!(list.is_empty());
//! ```

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod camera;
pub mod draw_list;
pub mod error;
pub mod frustum;
pub mod gizmo;
pub mod gltf_loader;
pub mod gpu;
pub mod ibl;
pub mod lights;
pub mod pcf;
pub mod renderer;
pub mod tonemap;

pub use camera::{
    CameraUniform, CameraView, extract_camera, first_active_camera, projection_matrix,
};
pub use draw_list::{DrawItem, DrawList, extract_draw_list};
pub use error::RenderError;
pub use frustum::{Frustum, Plane, transform_aabb};
pub use gizmo::{GizmoAxis, GizmoGeometry, GizmoMode, GizmoVertex, build_gizmo};
pub use gltf_loader::{LoadedGltf, NodeDesc, RegisteredGltf, load_gltf, load_gltf_bytes};
pub use ibl::{IblConfig, IblData};
pub use lights::{ExtractedLight, LightsUniform, MAX_LIGHTS, extract_lights};
pub use pcf::{DEFAULT_PCF_KERNEL, pcf_kernel_offsets};
pub use renderer::{Frame, Renderer, RendererConfig};
pub use tonemap::Tonemap;
