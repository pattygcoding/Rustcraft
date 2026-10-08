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
mod hotbar;
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
use crate::hotbar::Hotbar;
use crate::input::Input;
use crate::world::{Block, DEFAULT_SEED, SEA_LEVEL, World};

/// Title shown in the window's title bar.
const WINDOW_TITLE: &str = "Rustcraft";

/// Default window size, in logical pixels.
const WINDOW_SIZE: LogicalSize<f64> = LogicalSize::new(1280.0, 720.0);

/// How far (in blocks) the player can reach to break a block, like Minecraft's
/// block-reach.
const REACH: f32 = 6.0;

/// Eye height above the ground where the player spawns, in blocks.
const SPAWN_EYE_HEIGHT: f32 = 1.62;

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
    /// The blocks you can place, and the current selection.
    hotbar: Hotbar,
    /// Right-button state last frame, so breaking fires once per click.
    breaking: bool,
    /// Left-button state last frame, so placing fires once per click.
    placing: bool,
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

        // The terrain seed; set `RUSTCRAFT_SEED=<n>` for a different world.
        let seed = std::env::var("RUSTCRAFT_SEED")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_SEED);
        log::info!("terrain seed: {seed}");

        // Load the first chunks around the camera's starting position, then stand
        // the camera on the ground there.
        let mut world = World::new(seed);
        world.update(self.camera.position.x, self.camera.position.z);
        place_camera_on_surface(&mut self.camera, &world);
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

                // Scroll the mouse wheel to change the block you'd place.
                if self.cursor_captured {
                    let ticks = input.scroll.round() as i32;
                    if ticks != 0 {
                        self.hotbar.scroll(ticks);
                    }
                }

                // Right-click breaks, left-click places. Detect the press edges so
                // one click does exactly one of them.
                let breaking = input.secondary && self.cursor_captured;
                let placing = input.primary && self.cursor_captured;
                self.input.end_frame();

                // Stream chunks, mesh a few dirty ones, and edit the looked-at block.
                if let Some(world) = self.world.as_mut() {
                    if breaking && !self.breaking {
                        break_looked_at_block(world, &self.camera);
                    }
                    if placing && !self.placing {
                        place_looked_at_block(world, &self.camera, self.hotbar.selected());
                    }
                    world.update(self.camera.position.x, self.camera.position.z);
                    renderer.update_chunks(world);
                }
                self.breaking = breaking;
                self.placing = placing;

                renderer.render(&self.camera, &self.hotbar);
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

/// Raycast from the camera and clear the first breakable block it hits.
///
/// Free functions (rather than methods) so they can run while the renderer is
/// borrowed for the frame.
fn break_looked_at_block(world: &mut World, camera: &Camera) {
    let Some(hit) = world.raycast(camera.position, camera.forward(), REACH) else {
        return;
    };
    let [x, y, z] = hit.block;
    if world.block(x, y, z).is_breakable() {
        world.set_block(x, y, z, Block::Air);
    }
}

/// Raycast from the camera and place `block` in the empty cell in front of the
/// first block the ray hits (i.e. against the face it entered through).
fn place_looked_at_block(world: &mut World, camera: &Camera, block: Block) {
    let Some(hit) = world.raycast(camera.position, camera.forward(), REACH) else {
        return;
    };
    let (px, py, pz) = (
        hit.block[0] + hit.normal[0],
        hit.block[1] + hit.normal[1],
        hit.block[2] + hit.normal[2],
    );
    // Only place into space that is not already an opaque block. Water counts as
    // free space, so you can build into the sea.
    if !world.block(px, py, pz).is_opaque() {
        world.set_block(px, py, pz, block);
    }
}

/// Stand the camera at eye height on the ground at its current `x`/`z`.
///
/// `World::surface_height` reports the topmost *terrain* block, so the walkable
/// surface is one block above it — unless that ground is under water, in which case
/// the camera stands on the sea.
fn place_camera_on_surface(camera: &mut Camera, world: &World) {
    let x = camera.position.x.floor() as i32;
    let z = camera.position.z.floor() as i32;
    let ground = world.surface_height(x, z).max(SEA_LEVEL);
    camera.position.y = (ground + 1) as f32 + SPAWN_EYE_HEIGHT;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The camera should spawn standing on the ground — or on the sea, when the
    /// ground there is under water — never buried inside a block.
    #[test]
    fn the_starting_camera_stands_on_ground_or_sea() {
        let mut world = World::new(DEFAULT_SEED);
        let mut camera = Camera::default();
        world.update(camera.position.x, camera.position.z);
        place_camera_on_surface(&mut camera, &world);

        let x = camera.position.x.floor() as i32;
        let z = camera.position.z.floor() as i32;
        let underfoot = world.surface_height(x, z).max(SEA_LEVEL);

        // Ground where the spawn is on land, water where it is over the sea.
        let below = world.block(x, underfoot, z);
        assert!(
            below.is_opaque() || below == Block::Water,
            "nothing to stand on: {below:?}"
        );
        // ...and the cell the camera occupies is clear, so it is not buried.
        let head = camera.position.y.floor() as i32;
        assert!(
            !world.block(x, head, z).is_opaque(),
            "spawned inside a block"
        );
    }
}
