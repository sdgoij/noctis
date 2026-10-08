//! The directional/spot shadow map resource.

/// A depth-only shadow map texture and its view.
#[derive(Debug)]
pub struct ShadowResources {
    /// The depth texture.
    pub texture: wgpu::Texture,
    /// The depth view used as a render attachment and sampled in the PBR pass.
    pub view: wgpu::TextureView,
    /// The square edge length in texels.
    pub size: u32,
}

impl ShadowResources {
    /// Create a `size` x `size` `depth32float` shadow map (clamped to >= 1).
    pub fn new(device: &wgpu::Device, size: u32) -> ShadowResources {
        let size = size.max(1);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow.map"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        ShadowResources {
            texture,
            view,
            size,
        }
    }

    /// The size of one texel in UV space (`1 / size`).
    pub fn texel_size(&self) -> f32 {
        1.0 / self.size as f32
    }
}
