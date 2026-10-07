//! The wgpu + egui-wgpu renderer for the HarmonyOS surface.
//!
//! This is the same pairing `eframe` uses on desktop (`egui-wgpu` over `wgpu`), so the UI, its
//! paint callbacks and the GPU document canvas are unchanged. The only platform-specific part is
//! surface creation: HarmonyOS hands the XComponent's `OH_NativeWindow` to winit, which publishes
//! it as `raw-window-handle`'s `OhosNdk` handle; `wgpu-hal` turns that into a Vulkan surface or an
//! EGL/GLES surface (`Rwh::OhosNdk` in `wgpu-hal`'s `vulkan` and `gles` backends).
use std::sync::Arc;

use egui_wgpu::{RenderState, Renderer, RendererOptions, ScreenDescriptor, SurfaceConfig, WgpuSetupCreateNew};

/// egui asks for 8192 px textures (large canvases stay on the GPU), as `eframe` does.
const MAX_TEXTURE_DIMENSION_2D: u32 = 8192;

/// Used when the platform reports no usable format (some emulator builds).
const FALLBACK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Outcome of one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// Painted and presented.
    Done,
    /// No image this vsync (timeout, occluded): try again on the next redraw.
    Skipped,
    /// The surface is gone: the host must rebuild the renderer.
    Lost,
}

/// Everything needed to paint frames: the long-lived wgpu device and egui's renderer, plus
/// the swap chain for the current surface.
///
/// The device, adapter and egui renderer live for the whole app session: dropping them loses every
/// UI texture egui uploaded (fonts, icons), which egui would never re-upload. Only the surface is
/// recreated when HarmonyOS hands us a new native window (background/foreground, lock/unlock).
pub struct Gpu {
    /// The wgpu state the UI hands to the canvas paint callbacks (device, queue, adapter).
    pub state: RenderState,
    surface: Option<wgpu::Surface<'static>>,
    config: Option<wgpu::SurfaceConfiguration>,
    size: [u32; 2],
}

impl Gpu {
    /// Creates the instance, device and egui renderer, and attaches the first surface.
    ///
    /// The same backend set `eframe` offers is used (Vulkan/Metal/DX12 plus GL): Vulkan on real
    /// HarmonyOS hardware, OpenGL ES on the emulator images, which have no Vulkan driver.
    ///
    /// The device is created once per session; later surfaces come from [`Self::attach`], so UI
    /// textures survive backgrounding (see the [`Gpu`] docs).
    pub async fn new(window: Arc<dyn winit_core::window::Window>) -> Result<Self, String> {
        // Same defaults eframe builds its instance from (`PRIMARY | GL`, env overrides apply).
        let setup = WgpuSetupCreateNew::without_display_handle();
        let instance = wgpu::Instance::new(setup.instance_descriptor);
        let surface = instance.create_surface(window.clone()).map_err(|error| format!("cannot create the HarmonyOS surface: {error}"))?;
        let physical = window.surface_size();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|error| format!("no wgpu adapter for the HarmonyOS surface: {error}"))?;
        let backend = adapter.get_info().backend;
        // Request exactly what the adapter offers (the emulator's GLES driver reports lower
        // limits than desktop GL), keeping the large texture size egui asks for where allowed.
        let mut limits = adapter.limits();
        limits.max_texture_dimension_2d = limits.max_texture_dimension_2d.min(MAX_TEXTURE_DIMENSION_2D);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("openphoto wgpu device"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::Performance,
                ..Default::default()
            })
            .await
            .map_err(|error| format!("cannot create the wgpu device: {error}"))?;
        device.on_uncaptured_error(Arc::new(|error| log::error!("wgpu: {error}")));
        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|format| format.is_srgb()).or_else(|| caps.formats.first().copied()).unwrap_or_else(|| {
            log::warn!("wgpu: no surface format reported; assuming {FALLBACK_FORMAT:?}");
            FALLBACK_FORMAT
        });
        let present_mode = caps
            .present_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::PresentMode::AutoVsync)
            .or_else(|| caps.present_modes.iter().copied().find(|mode| *mode == wgpu::PresentMode::Fifo))
            .unwrap_or_else(|| caps.present_modes.first().copied().unwrap_or(wgpu::PresentMode::Fifo));
        let config = wgpu::SurfaceConfiguration {
            // The emulator's GLES swap chain only supports COLOR_TARGET; nothing reads the
            // presented image back, so no COPY_SRC is needed.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: physical.width.max(1),
            height: physical.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        let renderer = Renderer::new(
            &device,
            format,
            RendererOptions { msaa_samples: 1, depth_stencil_format: None, dithering: true, predictable_texture_filtering: false },
        );
        let info = adapter.get_info();
        log::info!("wgpu: {backend:?} adapter {:?} ({:?}), surface {format:?} {}x{}", info.name, info.device_type, config.width, config.height);
        let state = RenderState {
            adapter,
            available_adapters: vec![],
            instance,
            device,
            queue,
            target_format: format,
            renderer: Arc::new(egui::epaint::mutex::RwLock::new(renderer)),
            surface_config: SurfaceConfig { present_mode, desired_maximum_frame_latency: Some(2) },
        };
        let size = [config.width, config.height];
        Ok(Self { state, surface: Some(surface), config: Some(config), size })
    }

    /// Attaches a (new) native window: creates its surface and configures the swap chain with the
    /// stored format, keeping the device, adapter and all uploaded UI textures.
    pub fn attach(&mut self, window: &Arc<dyn winit_core::window::Window>) -> Result<(), String> {
        let surface = self.state.instance.create_surface(window.clone()).map_err(|error| format!("cannot create the HarmonyOS surface: {error}"))?;
        let physical = window.surface_size();
        let Some(config) = self.config.as_mut() else {
            return Err("no surface configuration stored".into());
        };
        config.width = physical.width.max(1);
        config.height = physical.height.max(1);
        surface.configure(&self.state.device, config);
        self.size = [config.width, config.height];
        self.surface = Some(surface);
        log::info!("surface attached at {}x{}", config.width, config.height);
        Ok(())
    }

    /// Releases the swap chain while the native window is gone (backgrounded, screen locked).
    /// The device and every uploaded texture survive; [`Self::attach`] brings frames back.
    pub fn detach(&mut self) {
        self.surface = None;
    }

    /// Whether a surface is attached (frames can be presented).
    pub fn has_surface(&self) -> bool {
        self.surface.is_some()
    }

    /// Re-configures the swap chain after the surface changed size.
    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.size == [width, height] {
            return;
        }
        self.size = [width, height];
        if let Some(config) = self.config.as_mut() {
            config.width = width;
            config.height = height;
            if let Some(surface) = self.surface.as_ref() {
                surface.configure(&self.state.device, config);
            }
        }
        log::info!("surface resized to {width}x{height}");
    }

    /// Surface size in physical pixels (egui's `screen_rect`).
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Paints one frame and presents it. When no surface is attached (backgrounded), queued
    /// texture uploads are still applied so nothing leaks, and [`Frame::Skipped`] is reported.
    /// [`Frame::Lost`] means the swap chain died while attached: the host detaches and attaches
    /// the current window again instead of tearing the app down.
    pub fn render(&mut self, clipped: &[egui::ClippedPrimitive], textures: &egui::TexturesDelta, pixels_per_point: f32) -> Frame {
        let [width, height] = self.size;
        let screen = ScreenDescriptor { size_in_pixels: [width, height], pixels_per_point };
        let mut encoder = self.state.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("openphoto frame") });
        {
            let mut renderer = self.state.renderer.write();
            for (id, image_deltas) in &textures.set {
                for image_delta in image_deltas {
                    renderer.update_texture(&self.state.device, &self.state.queue, *id, image_delta);
                }
            }
            renderer.update_buffers(&self.state.device, &self.state.queue, &mut encoder, clipped, &screen);
        }
        let Some(surface) = self.surface.as_ref() else {
            return Frame::Skipped;
        };
        let surface_texture = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            // The swap chain died while attached: the host re-attaches the current window.
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Validation => return Frame::Lost,
            // Nothing to draw into this vsync: wait for the next redraw.
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return Frame::Skipped,
        };
        let view = surface_texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut render_pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("openphoto frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.055, g: 0.055, b: 0.063, a: 1.0 }), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        self.state.renderer.read().render(&mut render_pass, clipped, &screen);
        drop(render_pass);
        for id in &textures.free {
            self.state.renderer.write().free_texture(id);
        }
        self.state.queue.submit(Some(encoder.finish()));
        self.state.queue.present(surface_texture);
        Frame::Done
    }
}
