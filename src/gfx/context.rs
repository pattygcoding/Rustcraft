//! One-time GPU setup shared by the whole application.
//!
//! [`GraphicsContext`] performs the boilerplate every wgpu app needs: create a
//! [`wgpu::Instance`], pick an [`wgpu::Adapter`] that can present to the window,
//! open a [`wgpu::Device`] + [`wgpu::Queue`], and configure the window's
//! presentation [`wgpu::Surface`]. It also knows how to reconfigure the swap
//! chain when the window is resized.
//!
//! It deliberately knows nothing about *what* is drawn — that lives in
//! [`crate::gfx::Renderer`].

use std::sync::Arc;

use winit::window::Window;

/// Owns the wgpu device, queue and the window's configured surface.
pub struct GraphicsContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

impl GraphicsContext {
    /// Build the GPU context for `window`.
    ///
    /// `window` is an [`Arc`] so the created [`wgpu::Surface`] can be `'static`
    /// (the surface keeps its own reference to the window), which avoids a
    /// self-referential borrow in the owning application.
    ///
    /// # Panics
    ///
    /// Panics if no compatible GPU adapter is found or the device cannot be
    /// created — acceptable for a bootstrap that draws nothing yet.
    pub fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        // The instance is only needed to bootstrap; wgpu resources keep their
        // own internal references, so it can be dropped once setup is done.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let surface = instance
            .create_surface(window)
            .expect("failed to create a wgpu surface for the window");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            ..Default::default()
        }))
        .expect("failed to find a suitable GPU adapter");

        log::info!("using adapter: {:?}", adapter.get_info());

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rustcraft-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .expect("failed to create the wgpu device");

        let mut config = surface
            .get_default_config(&adapter, width, height)
            .expect("the chosen adapter cannot present to this surface");
        // Vertical sync: tear-free and keeps the fan quiet.
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&device, &config);

        log::info!(
            "surface configured: {}x{} format={:?}",
            config.width,
            config.height,
            config.format
        );

        Self {
            device,
            queue,
            surface,
            config,
        }
    }

    /// The window's presentation surface.
    pub fn surface(&self) -> &wgpu::Surface<'static> {
        &self.surface
    }

    /// The logical GPU device.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The queue used to submit command buffers and present frames.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// The current swap-chain configuration (size, format, present mode, ...).
    pub fn config(&self) -> &wgpu::SurfaceConfiguration {
        &self.config
    }

    /// Reconfigure the swap chain after the window changed size.
    ///
    /// A zero-sized request (e.g. while minimised) is ignored because wgpu
    /// rejects zero-sized surfaces.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.reconfigure();
    }

    /// Re-apply the current [`wgpu::SurfaceConfiguration`] to the surface.
    ///
    /// Must not be called while a frame obtained from
    /// [`wgpu::Surface::get_current_texture`] is still alive: `configure` panics
    /// if an old [`wgpu::SurfaceTexture`] still references the surface.
    pub fn reconfigure(&self) {
        self.surface.configure(&self.device, &self.config);
    }
}
