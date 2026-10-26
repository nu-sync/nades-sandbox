//! The renderer: the GPU device, and a pack's map drawn in CS2's look ([`map`]): its materials, its baked light
//! (irradiance, directional irradiance and sun shadows), the sky and the 3D skybox, fog, then exposure, bloom, the tone
//! curve and the colour lookup table ([`post`]). [`view`] holds the camera. The smoke arrives in M3 (SPEC.md).

pub mod map;
pub mod post;
pub mod view;

pub use view::View;

use thiserror::Error;

/// What the device can do that the packs and the profiler rely on.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    /// BC1–BC7 and BC6H textures: the game's own blocks, uploaded as they are stored.
    pub bc: bool,
    /// Texture binding arrays indexed per material: every texture of a map in one bind group.
    pub bindless: bool,
    /// Buffers the CPU writes straight into (Apple silicon's unified memory): no staging copy for geometry.
    pub mappable_primary: bool,
    /// GPU timestamps, for per-pass timings.
    pub timestamps: bool,
}

#[derive(Debug, Error)]
pub enum GpuError {
    #[error("no GPU adapter: {0}")]
    Adapter(#[from] wgpu::RequestAdapterError),
    #[error("the GPU refused the device: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
}

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub capabilities: Capabilities,
}

/// The instance: Metal on macOS, DX12 or Vulkan on Windows. None of them needs the display handle (GL would).
pub fn instance() -> wgpu::Instance {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::PRIMARY;
    wgpu::Instance::new(descriptor)
}

impl Gpu {
    /// The high-performance adapter (that can present to `surface`, when there is one: on Metal every adapter
    /// presents, so the device need not wait for the window), and a device with the adapter's own limits (big
    /// textures and buffers) and every wanted feature it has.
    pub fn new(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Result<Self, GpuError> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface,
            ..Default::default()
        }))?;
        let bindless = wgpu::Features::TEXTURE_BINDING_ARRAY | wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING;
        // BC volumes: the light probes' atlases
        let wanted = wgpu::Features::TEXTURE_COMPRESSION_BC | wgpu::Features::TEXTURE_COMPRESSION_BC_SLICED_3D | wgpu::Features::MAPPABLE_PRIMARY_BUFFERS | wgpu::Features::TIMESTAMP_QUERY | bindless;
        let features = adapter.features() & wanted;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("nades-sandbox"),
            required_features: features,
            required_limits: adapter.limits(),
            ..Default::default()
        }))?;
        let capabilities = Capabilities {
            bc: features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
            bindless: features.contains(bindless),
            mappable_primary: features.contains(wgpu::Features::MAPPABLE_PRIMARY_BUFFERS),
            timestamps: features.contains(wgpu::Features::TIMESTAMP_QUERY),
        };
        Ok(Self { instance, adapter, device, queue, capabilities })
    }

    /// The adapter's name, e.g. "Apple M5 Max".
    pub fn name(&self) -> String {
        self.adapter.get_info().name
    }

    /// Waits until the GPU has run everything submitted so far.
    pub fn wait_idle(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }
}

/// A swap-chain image taken straight from the window's layer, for the first frame. wgpu's Metal surface refuses to
/// hand one out until the window server reports the window visible (a guard against a stall when an occluded
/// window's drawables run out, gfx-rs/wgpu#8309), which a new window is only ≈40 ms after AppKit made it. A fresh
/// layer has every drawable free, so its first frame can be drawn and presented at once; the window then appears
/// with the map in it.
pub struct EarlyFrame {
    pub texture: wgpu::Texture,
    present: Box<dyn FnOnce()>,
}

impl EarlyFrame {
    /// The layer's next drawable as a texture `config` describes; None off macOS or where the layer has none.
    pub fn acquire(device: &wgpu::Device, surface: &wgpu::Surface<'_>, config: &wgpu::SurfaceConfiguration) -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            use objc2_metal::{MTLDrawable as _, MTLTextureType};
            use objc2_quartz_core::CAMetalDrawable as _;
            use wgpu::hal::api::Metal;
            // SAFETY: the drawable's texture is the layer's configured format and size (`config`), it is only drawn
            // into by the frame it is acquired for, and `present` runs after that frame has finished on the GPU
            unsafe {
                let drawable = surface.as_hal::<Metal>()?.render_layer().lock().nextDrawable()?;
                let extent = wgpu::hal::CopyExtent { width: config.width, height: config.height, depth: 1 };
                let raw = wgpu::hal::metal::Device::texture_from_raw(drawable.texture(), config.format, MTLTextureType::Type2D, 1, 1, extent, None);
                let texture = device.create_texture_from_hal::<Metal>(
                    raw,
                    &wgpu::TextureDescriptor {
                        label: Some("first frame"),
                        size: wgpu::Extent3d { width: config.width, height: config.height, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: config.format,
                        usage: config.usage,
                        view_formats: &[],
                    },
                    // the frame clears it
                    wgpu::TextureUses::UNINITIALIZED,
                );
                Some(Self { texture, present: Box::new(move || drawable.present()) })
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (device, surface, config);
            None
        }
    }

    /// Shows the frame; call once the GPU has finished drawing it.
    pub fn present(self) {
        (self.present)();
    }
}
