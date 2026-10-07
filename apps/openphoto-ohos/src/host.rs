//! The HarmonyOS application handler: owns the window, the renderer and the editor.
//!
//! This mirrors what `eframe` does for the desktop shell (`apps/openphoto`): build the editor
//! once, feed it input every frame, and hand the wgpu state to the canvas so the document renders
//! on the GPU. The frame itself is driven by HarmonyOS (`RedrawRequested` arrives when the system
//! asks for a frame); `Window::request_redraw` is a no-op on this backend, so there is no
//! repaint loop to run.
use std::sync::Arc;
use std::time::{Duration, Instant};

use openphoto_engine::Session;
use openphoto_ui_egui::OpenPhotoApp;
use winit_core::application::ApplicationHandler;
use winit_core::event::WindowEvent;
use winit_core::event_loop::ActiveEventLoop;
use winit_core::window::{Window, WindowAttributes, WindowId};

use crate::bridge::block_on;
use crate::gpu::Gpu;
use crate::input::Input;
use crate::services;

/// The editor and its platform shell.
pub struct OpenPhoto {
    /// The HarmonyOS ability handle (file dialogs, sandbox paths).
    ability: openharmony_ability::OpenHarmonyApp,
    /// The single HarmonyOS window (the XComponent surface); `None` before the surface exists.
    window: Option<Arc<dyn Window>>,
    gpu: Option<Gpu>,
    /// The editor: the same `OpenPhotoApp` the desktop shell runs.
    editor: Option<OpenPhotoApp>,
    ctx: egui::Context,
    input: Input,
    last_frame: Instant,
    /// Times startup so a slow first frame shows in the log, as on desktop.
    started_at: Instant,
}

impl OpenPhoto {
    pub fn new(app: openharmony_ability::OpenHarmonyApp) -> Self {
        Self {
            ability: app,
            window: None,
            gpu: None,
            editor: None,
            ctx: egui::Context::default(),
            input: Input::new(),
            last_frame: Instant::now(),
            started_at: Instant::now(),
        }
    }

    /// Builds the editor once the window exists.
    fn ensure_app(&mut self) {
        if self.editor.is_none() {
            self.editor = Some(OpenPhotoApp::new(Session::new(), services::build(&self.ability)));
            log::info!("editor started in {:.1?}", self.started_at.elapsed());
        }
    }

    /// Creates (once) the wgpu device and swap chain for the current surface, and re-attaches
    /// the swap chain whenever HarmonyOS hands us a new native window. The device, adapter and
    /// egui renderer — and every uploaded UI texture — survive backgrounding and screen lock.
    fn ensure_gpu(&mut self) {
        let Some(window) = self.window.clone() else { return };
        if let Some(gpu) = self.gpu.as_mut() {
            if !gpu.has_surface() {
                // New native window, same device: re-attach the swap chain.
                if let Err(error) = gpu.attach(&window) {
                    log::error!("cannot attach the surface: {error}");
                }
            }
            return;
        }
        // Device creation only resolves already-ready wgpu futures; blocking the ability thread
        // briefly here matches what eframe does on desktop startup. A failure retries on the next
        // redraw (the driver may not be ready yet).
        match block_on(Gpu::new(window)) {
            Ok(gpu) => {
                let [width, height] = gpu.size();
                if let Some(app) = self.editor.as_mut() {
                    // Same call the desktop shell makes with `cc.wgpu_render_state`.
                    app.set_wgpu(gpu.state.clone());
                }
                log::info!("renderer ready: {width}x{height}");
                self.gpu = Some(gpu);
            }
            Err(error) => log::error!("cannot start the renderer: {error}"),
        }
    }

    /// Runs one frame: layout, paint, present. Called only for `RedrawRequested`.
    fn draw(&mut self) {
        if self.editor.is_none() || self.gpu.is_none() {
            return;
        }
        let Some(window) = self.window.clone() else { return };
        let physical = window.surface_size();
        let pixels_per_point = (window.scale_factor() as f32).clamp(0.5, 4.0);
        let screen_rect =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(physical.width as f32 / pixels_per_point, physical.height as f32 / pixels_per_point));
        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last_frame).min(Duration::from_millis(250));
        self.last_frame = now;
        let mut raw_input = self.input.take(screen_rect, dt, pixels_per_point);
        if let Some(app) = self.editor.as_mut() {
            // Shortcut pre-processing (clipboard keys) and synthetic automation input, as eframe does.
            app.apply_raw_input_hook(&self.ctx, &mut raw_input);
        }
        // `run_ui` is exactly what eframe calls: logic first, then the UI into the root viewport.
        let full_output = self.ctx.run_ui(raw_input, |ui| {
            if let Some(app) = self.editor.as_mut() {
                // The same two passes the desktop shell runs through `eframe::App`.
                app.update_logic(ui.ctx());
                app.update_ui(ui);
            }
        });
        if let Some(output) = full_output.viewport_output.get(&egui::ViewportId::ROOT) {
            for command in &output.commands {
                handle_viewport_command(command);
            }
        }
        // Paint callbacks (the GPU document canvas) run inside `render`, as with eframe.
        let clipped = self.ctx.tessellate(full_output.shapes.clone(), full_output.pixels_per_point);
        let Some(gpu) = self.gpu.as_mut() else { return };
        match gpu.render(&clipped, &full_output.textures_delta, full_output.pixels_per_point) {
            crate::gpu::Frame::Done | crate::gpu::Frame::Skipped => {}
            crate::gpu::Frame::Lost => {
                // The swap chain died while attached: detach and re-attach the current window on
                // the next redraw. The device and all UI textures survive.
                log::warn!("surface lost; re-attaching");
                gpu.detach();
            }
        }
    }
}

/// Viewport commands the editor issues (title, attention, …). HarmonyOS owns the window, so they
/// are observed (and logged) rather than applied.
fn handle_viewport_command(command: &egui::ViewportCommand) {
    match command {
        egui::ViewportCommand::Title(title) => log::debug!("viewport title: {title}"),
        egui::ViewportCommand::RequestUserAttention(_) => log::debug!("viewport requests attention"),
        _ => {}
    }
}

impl ApplicationHandler for OpenPhoto {
    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_none() {
            match event_loop.create_window(WindowAttributes::default()) {
                Ok(window) => {
                    let window: Arc<dyn Window> = window.into();
                    let physical = window.surface_size();
                    log::info!("window: {}x{} at {}x scale", physical.width, physical.height, window.scale_factor());
                    self.window = Some(window);
                    self.ensure_app();
                }
                Err(error) => log::error!("cannot create the window: {error}"),
            }
        }
        self.ensure_gpu();
    }

    fn window_event(&mut self, event_loop: &dyn ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        match &event {
            WindowEvent::SurfaceResized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.resize(size.width, size.height);
                }
            }
            WindowEvent::CloseRequested => {
                log::info!("close requested");
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                self.ensure_gpu();
                self.draw();
            }
            _ => {}
        }
        let pixels_per_point = self.window.as_ref().map(|window| window.scale_factor() as f32).unwrap_or(1.0).clamp(0.5, 4.0);
        self.input.handle(&event, pixels_per_point);
    }

    fn suspended(&mut self, _event_loop: &dyn ActiveEventLoop) {
        // Release the swap chain while backgrounded; the device and all UI textures survive, and
        // the next redraw re-attaches the current window.
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.detach();
        }
    }

    fn destroy_surfaces(&mut self, _event_loop: &dyn ActiveEventLoop) {
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.detach();
        }
    }
}
