//! Rustcraft — a lightweight Minecraft clone written in Rust.
//!
//! This entry point bootstraps logging, creates the `winit` event loop and
//! drives the graphics layer (see [`gfx`]).
//!
//! The current build renders a coloured cube through a first-person fly camera
//! (see [`camera`]). Voxel world generation, chunk meshing and block interaction
//! come next — see `AGENTS.md` for the roadmap.

mod camera;
mod gfx;
mod input;
mod world;

use std::sync::Arc;
use std::time::Instant;

use glam::Vec2;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::camera::Camera;
use crate::gfx::Renderer;
use crate::input::Input;
use crate::world::World;

/// Title shown in the window's title bar.
const WINDOW_TITLE: &str = "Rustcraft";

/// Default window size, in logical pixels.
const WINDOW_SIZE: LogicalSize<f64> = LogicalSize::new(1280.0, 720.0);

fn main() -> Result<(), winit::error::EventLoopError> {
    // `RUST_LOG=info` (or `debug`) enables logs; wgpu/winit log through `log`.
    env_logger::init();

    let event_loop = EventLoop::new()?;
    // A game renders continuously rather than waiting for input.
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::default();
    event_loop.run_app(&mut app)
}

/// The top-level application, connecting `winit` events to the [`Renderer`].
///
/// The window is created lazily in [`ApplicationHandler::resumed`] because some
/// platforms (Android, iOS, Wayland) forbid creating a surface before then.
#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    world: Option<World>,
    input: Input,
    camera: Camera,
    /// Time of the previous frame, for the camera's delta-time. `None` until the
    /// first frame is drawn.
    last_frame: Option<Instant>,
    /// Whether the cursor is currently grabbed for mouse-look.
    cursor_captured: bool,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Guard against redundant `Resumed` events.
        if self.window.is_some() {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title(WINDOW_TITLE)
            .with_inner_size(WINDOW_SIZE);

        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .expect("failed to create the window"),
        );

        // Grab the cursor so mouse-look feels like a first-person game.
        set_cursor_captured(&window, true);
        self.cursor_captured = true;

        // `Arc<Window>` lets the surface be `'static`, avoiding a
        // self-referential borrow between the window and its renderer.
        self.renderer = Some(Renderer::new(window.clone()));

        // Load the first chunks around the camera's starting position.
        let mut world = World::new();
        world.update(self.camera.position.x, self.camera.position.z);
        log::info!("world: {} chunks loaded", world.loaded_count());
        self.world = Some(world);

        self.window = Some(window);
        self.last_frame = None;

        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Record input first, so no event is missed.
        self.input.handle_window_event(&event);

        let (Some(window), Some(renderer)) = (self.window.as_ref(), self.renderer.as_mut()) else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            // Escape frees the cursor; a left click grabs it again.
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && event.physical_key == PhysicalKey::Code(KeyCode::Escape) =>
            {
                set_cursor_captured(window, false);
                self.cursor_captured = false;
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if !self.cursor_captured => {
                set_cursor_captured(window, true);
                self.cursor_captured = true;
            }

            WindowEvent::Resized(size) => renderer.resize(size.width, size.height),

            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = self
                    .last_frame
                    .map_or(0.0, |last| (now - last).as_secs_f32());
                self.last_frame = Some(now);

                // Sample this frame's intent. Mouse-look is ignored while the
                // cursor is free (e.g. after Escape).
                let mut input = self.input.snapshot();
                if !self.cursor_captured {
                    input.look = Vec2::ZERO;
                }
                // Clamp dt so a long stall (dragging the window, ...) can't
                // teleport the camera.
                self.camera.update(&input, dt.min(0.1));
                self.input.end_frame();

                // Stream chunks around the player and mesh a few dirty ones.
                if let Some(world) = self.world.as_mut() {
                    world.update(self.camera.position.x, self.camera.position.z);
                    renderer.update_chunks(world);
                }

                renderer.render(&self.camera);
                // Keep the frames coming.
                window.request_redraw();
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        // Raw mouse motion, independent of the cursor: drives camera look.
        self.input.handle_device_event(&event);
    }
}

/// Grab or release the cursor for first-person mouse-look.
///
/// `Locked` is the ideal mode but is unsupported on some platforms, so fall back
/// to `Confined`.
fn set_cursor_captured(window: &Window, captured: bool) {
    let mode = if captured {
        CursorGrabMode::Locked
    } else {
        CursorGrabMode::None
    };
    if let Err(err) = window.set_cursor_grab(mode) {
        if captured {
            let _ = window.set_cursor_grab(CursorGrabMode::Confined);
        }
        log::warn!("could not set cursor grab to {mode:?}: {err}");
    }
    window.set_cursor_visible(!captured);
}
