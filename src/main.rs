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
mod inventory;
mod world;

use std::sync::Arc;
use std::time::{Duration, Instant};

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
use crate::inventory::{Inventory, SLOTS};
use crate::world::{Block, DEFAULT_SEED, SEA_LEVEL, World, chunk_of};

/// Title shown in the window's title bar.
const WINDOW_TITLE: &str = "Rustcraft";

/// Default window size, in logical pixels.
const WINDOW_SIZE: LogicalSize<f64> = LogicalSize::new(1280.0, 720.0);

/// How far (in blocks) the player can reach to break a block, like Minecraft's
/// block-reach.
const REACH: f32 = 6.0;

/// Eye height above the ground where the player spawns, in blocks.
const SPAWN_EYE_HEIGHT: f32 = 1.62;

/// A frame slower than this (33 ms ≈ 30 fps) is worth reporting, with what it spent its time on.
///
/// Now that chunks stream in from other threads, a long frame is either meshing on this thread or
/// the bookkeeping around a chunk-boundary crossing — the two things that still happen here — so
/// the numbers in that log line are where streaming shows up from the outside.
const SLOW_FRAME: Duration = Duration::from_millis(33);

/// How often the frame summary is logged, in seconds. A second is long enough to see a stutter and
/// short enough to see it pass.
const STATS_INTERVAL: Duration = Duration::from_secs(1);

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
    /// The blocks you can place, and the creative screen that fills them.
    inventory: Inventory,
    /// Right-button state last frame, so breaking fires once per click.
    breaking: bool,
    /// Left-button state last frame, so placing fires once per click.
    placing: bool,
    /// `E` state last frame, so the screen toggles once per press.
    inventory_key: bool,
    /// 1–9 state last frame, so each press stocks or picks a slot exactly once.
    slots: [bool; SLOTS],
    /// The worst frame since the last summary, and how many frames that summary covers.
    worst_frame: Duration,
    frames: u32,
    /// When the last frame summary was logged. `None` until the first frame.
    stats_at: Option<Instant>,
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

        // Stand the camera on real ground before the first frame, but only wait for the chunks
        // around it: the rest of the patch streams in over the first frames, so the window opens
        // on a world that fills in around you rather than on a frozen one.
        let mut world = World::new(seed);
        world.update(self.camera.position.x, self.camera.position.z);
        world.flush_around(chunk_of(self.camera.position.x, self.camera.position.z));
        place_camera_on_surface(&mut self.camera, &world);
        log::info!("world: {} chunks loaded on the spawn", world.loaded_count());
        self.world = Some(world);

        self.window = Some(window);
        self.last_frame = None;
        self.stats_at = Some(Instant::now());

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

            // Escape closes the inventory if it is open, and otherwise just frees the cursor.
            // A left click grabs the cursor back — unless the inventory wants that click.
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && event.physical_key == PhysicalKey::Code(KeyCode::Escape) =>
            {
                let captured = self.inventory.is_open();
                self.inventory.set_open(false);
                set_cursor_captured(window, captured);
                self.cursor_captured = captured;
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if !self.cursor_captured && !self.inventory.is_open() => {
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

                // Sample this frame's intent. Mouse-look is ignored while the cursor is free
                // — after Escape, or while the inventory is open.
                let mut input = self.input.snapshot();
                if !self.cursor_captured {
                    input.look = Vec2::ZERO;
                }
                // Clamp dt so a long stall (dragging the window, ...) can't
                // teleport the camera.
                self.camera.update(&input, dt.min(0.1));

                // The inventory reads the cursor, so it needs one aspect ratio everybody
                // agrees on.
                let size = window.inner_size();
                let aspect = size.width as f32 / size.height.max(1) as f32;
                let cursor = cursor_ndc(input.cursor, size.width, size.height);
                self.inventory.set_cursor(cursor, aspect);

                // `E` opens and closes the creative screen, and hands the cursor over with it.
                if input.inventory && !self.inventory_key {
                    let open = !self.inventory.is_open();
                    self.inventory.set_open(open);
                    set_cursor_captured(window, !open);
                    self.cursor_captured = !open;
                }
                self.inventory_key = input.inventory;

                // 1–9: choose a slot in the world, or stock one on the creative screen.
                for (index, &held) in input.slots.iter().enumerate() {
                    if held && !self.slots[index] {
                        self.inventory.number_key(index + 1);
                    }
                }
                self.slots = input.slots;

                let open = self.inventory.is_open();

                // Scroll the mouse wheel to change the block you'd place.
                if !open {
                    let ticks = input.scroll.round() as i32;
                    if ticks != 0 {
                        self.inventory.scroll(ticks);
                    }
                }

                // Right-click breaks, left-click places. Detect the press edges on the *raw*
                // buttons so one click does exactly one thing, and let the screen have those
                // clicks while it is open — the cursor is free then, so gating on it would
                // mean the screen never saw a click at all.
                let left_pressed = input.primary && !self.placing;
                let right_pressed = input.secondary && !self.breaking;
                self.placing = input.primary;
                self.breaking = input.secondary;
                if open && (left_pressed || right_pressed) {
                    self.inventory.click(cursor, aspect);
                }
                self.input.end_frame();

                // Stream chunks, mesh a few dirty ones, and edit the looked-at block.
                if let Some(world) = self.world.as_mut() {
                    let in_world = !open && self.cursor_captured;
                    if in_world && right_pressed {
                        break_looked_at_block(world, &self.camera);
                    }
                    // Left-click places — if the selected slot holds anything at all.
                    let to_place = (in_world && left_pressed)
                        .then(|| self.inventory.selected())
                        .flatten();
                    if let Some(block) = to_place {
                        place_looked_at_block(world, &self.camera, block);
                    }
                    world.update(self.camera.position.x, self.camera.position.z);
                    renderer.update_chunks(world);
                }

                renderer.render(&self.camera, &self.inventory);
                // Keep the frames coming.
                window.request_redraw();

                self.log_frame_time(now);
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

impl App {
    /// Report a frame that ran long, and summarise the last second of them.
    ///
    /// Chunk streaming is what used to make frames stall, so it is what this watches: neither the
    /// state of the generator threads nor a frame's own bookkeeping can be seen from the outside,
    /// and the numbers here say which of them a stutter was.
    fn log_frame_time(&mut self, started: Instant) {
        let frame = started.elapsed();
        self.worst_frame = self.worst_frame.max(frame);
        self.frames += 1;

        if frame > SLOW_FRAME {
            let meshed = self.renderer.as_ref().map_or(0.0, Renderer::mesh_millis);
            log::warn!(
                "slow frame: {:.0} ms, {meshed:.1} ms of it meshing chunks",
                frame.as_secs_f32() * 1000.0
            );
        }

        if !self
            .stats_at
            .is_none_or(|at| at.elapsed() >= STATS_INTERVAL)
        {
            return;
        }
        let (generated, average, worst) = self.world.as_ref().map_or((0, 0.0, 0.0), |world| {
            let stats = world.generation_stats();
            (stats.chunks, stats.average_millis, stats.worst_millis)
        });
        log::debug!(
            "{} fps, worst frame {} ms, {generated} chunks generated ({average:.1} ms each, worst {worst:.1} ms)",
            self.frames,
            self.worst_frame.as_millis(),
        );
        self.frames = 0;
        self.worst_frame = Duration::ZERO;
        self.stats_at = Some(Instant::now());
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
    // Only place into free space: air, or a fluid you are building over.
    if world.block(px, py, pz).is_replaceable() {
        world.set_block(px, py, pz, block);
    }
}

/// Stand the camera at eye height on the ground at its current `x`/`z`.
///
/// `World::surface_height` reports the topmost *terrain* block, so the walkable
/// surface is one block above it — unless that ground is under water, in which case
/// the camera stands on the sea. An oak growing on the ground is solid, so the camera
/// steps up over one rather than spawning buried inside a trunk.
fn place_camera_on_surface(camera: &mut Camera, world: &World) {
    let x = camera.position.x.floor() as i32;
    let z = camera.position.z.floor() as i32;
    let mut ground = world.surface_height(x, z).max(SEA_LEVEL);
    while !world.block(x, ground + 1, z).is_replaceable() {
        ground += 1;
    }
    camera.position.y = (ground + 1) as f32 + SPAWN_EYE_HEIGHT;
}

/// A cursor position in physical pixels, as NDC — the space the HUD is drawn in, with
/// `(-1, -1)` at the bottom left and `(1, 1)` at the top right.
fn cursor_ndc(pixels: Vec2, width: u32, height: u32) -> [f32; 2] {
    [
        (pixels.x / width.max(1) as f32) * 2.0 - 1.0,
        1.0 - (pixels.y / height.max(1) as f32) * 2.0,
    ]
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

    /// The camera should spawn standing on solid ground — or on the sea, when the
    /// ground there is under water — never buried inside a block, even after an oak
    /// has grown on that ground.
    #[test]
    fn the_starting_camera_stands_on_solid_ground_and_is_never_buried() {
        let mut world = World::new(DEFAULT_SEED);
        let mut camera = Camera::default();
        world.update(camera.position.x, camera.position.z);
        // The game waits only for the spawn neighbourhood before standing the camera on it, which
        // is what this mirrors.
        world.flush_around(chunk_of(camera.position.x, camera.position.z));
        place_camera_on_surface(&mut camera, &world);

        let x = camera.position.x.floor() as i32;
        let z = camera.position.z.floor() as i32;

        // The walkable surface is the ground, or the top of an oak standing on it.
        let mut ground = world.surface_height(x, z).max(SEA_LEVEL);
        while !world.block(x, ground + 1, z).is_replaceable() {
            ground += 1;
        }
        // Something solid underfoot: ground on land, water over the sea.
        let underfoot = world.block(x, ground, z);
        assert!(
            underfoot != Block::Air,
            "nothing to stand on: {underfoot:?}"
        );

        // ...and the cells the camera occupies are free space, so it is not buried.
        for y in [ground + 1, ground + 2] {
            assert!(
                world.block(x, y, z).is_replaceable(),
                "spawned inside a block at y = {y}"
            );
        }
    }
}
