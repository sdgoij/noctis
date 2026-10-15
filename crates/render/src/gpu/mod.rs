//! GPU-side building blocks for the renderer.
//!
//! Each submodule owns one concern:
//!
//! * [`uniforms`] — the `Pod` uniform blocks that mirror the WGSL structs.
//! * [`resources`] — lazy upload of meshes, textures, and materials.
//! * [`targets`] — the offscreen HDR/LDR/depth/SSAO targets and readback buffer.
//! * [`ibl_gpu`] — upload of the CPU-precomputed IBL textures.
//! * [`shadow`] — the shadow-map texture.
//! * [`pipelines`] — pipeline layouts, render pipelines, and bind groups.
//!
//! These modules are only exercised when a GPU adapter is present; the pure math
//! that feeds them lives in the crate's CPU modules.

pub mod ibl_gpu;
pub mod pipelines;
pub mod reflection;
pub mod resources;
pub mod shader_seam;
pub mod shadow;
pub mod targets;
pub mod uniforms;
