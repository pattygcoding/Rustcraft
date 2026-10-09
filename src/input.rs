//! Keyboard and mouse input.
//!
//! [`Input`] folds raw `winit` events into a small queryable state, then
//! [`Input::snapshot`] turns that into a per-frame [`PlayerInput`] describing the
//! player's intent. The mapping mirrors Minecraft:
//!
//! * WASD to move, Space to jump (or ascend while flying), Shift to sneak (or
//!   descend while flying), Ctrl to sprint.
//! * Mouse motion steers the camera; left/right buttons break/place; the wheel and
//!   the 1–9 keys pick an inventory slot; `E` toggles the creative inventory, and
//!   while that is open the screen reads the cursor position and the same buttons.
//! * **Double-tapping Space toggles flight.**
//!
//! Wiring from the winit `ApplicationHandler`:
//!
//! ```ignore
//! fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
//!     self.input.handle_window_event(&event);
//!     // ... once per frame:
//!     let player = self.input.snapshot(); // build movement/camera from this
//!     self.input.end_frame();             // clear per-frame amounts
//! }
//!
//! fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
//!     self.input.handle_device_event(&event); // raw mouse motion for look
//! }
//! ```

use std::collections::HashSet;
use std::time::{Duration, Instant};

use glam::Vec2;
use winit::event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

/// How quickly a second press must follow the first to count as a double-tap.
pub const DEFAULT_DOUBLE_TAP: Duration = Duration::from_millis(300);

/// The number keys, in slot order. In the world they choose a quick-access slot; on the
/// creative screen they stock one.
const SLOT_KEYS: [KeyCode; 9] = [
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

/// Tracks the instantaneous state of the keyboard and mouse.
pub struct Input {
    keys: HashSet<KeyCode>,
    mouse_buttons: HashSet<MouseButton>,
    look_delta: Vec2,
    scroll: f32,
    /// Where the cursor is, in physical pixels. Only the inventory reads it: mouse-look uses
    /// the raw motion above instead, so the two never fight.
    cursor: Vec2,
    flying: bool,
    space_double_tap: DoubleTap,
    double_tap_window: Duration,
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    /// Create an input state using the default double-tap window.
    pub fn new() -> Self {
        Self {
            keys: HashSet::new(),
            mouse_buttons: HashSet::new(),
            look_delta: Vec2::ZERO,
            scroll: 0.0,
            cursor: Vec2::ZERO,
            flying: false,
            space_double_tap: DoubleTap::default(),
            double_tap_window: DEFAULT_DOUBLE_TAP,
        }
    }

    /// Record a window event (keyboard, mouse buttons, wheel, focus).
    pub fn handle_window_event(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::KeyboardInput { event, .. } => match event.state {
                ElementState::Pressed => self.press_key(event.physical_key, event.repeat),
                ElementState::Released => self.release_key(event.physical_key),
            },
            WindowEvent::MouseInput { state, button, .. } => match state {
                ElementState::Pressed => {
                    self.mouse_buttons.insert(*button);
                }
                ElementState::Released => {
                    self.mouse_buttons.remove(button);
                }
            },
            WindowEvent::MouseWheel { delta, .. } => self.scroll += wheel_ticks(delta),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Vec2::new(position.x as f32, position.y as f32);
            }
            // Drop all held input if we lose focus, so keys don't "stick".
            WindowEvent::Focused(false) => self.release_all(),
            _ => {}
        }
    }

    /// Record a device event (used for raw, cursor-independent mouse motion).
    pub fn handle_device_event(&mut self, event: &DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.look_delta.x += delta.0 as f32;
            self.look_delta.y += delta.1 as f32;
        }
    }

    /// Snapshot the current intent for one frame.
    ///
    /// Call [`Input::end_frame`] after consuming the result to reset the
    /// per-frame amounts (`look`, `scroll`).
    pub fn snapshot(&self) -> PlayerInput {
        let mut movement = Vec2::ZERO;
        movement.y += self.axis(KeyCode::KeyW, KeyCode::KeyS);
        movement.x += self.axis(KeyCode::KeyD, KeyCode::KeyA);

        PlayerInput {
            // Normalise so moving diagonally isn't faster than moving straight.
            movement: movement.normalize_or_zero(),
            jump: self.is_key_pressed(KeyCode::Space),
            sneak: self.is_key_pressed(KeyCode::ShiftLeft)
                || self.is_key_pressed(KeyCode::ShiftRight),
            sprint: self.is_key_pressed(KeyCode::ControlLeft)
                || self.is_key_pressed(KeyCode::ControlRight),
            flying: self.flying,
            look: self.look_delta,
            scroll: self.scroll,
            primary: self.is_mouse_pressed(MouseButton::Left),
            secondary: self.is_mouse_pressed(MouseButton::Right),
            inventory: self.is_key_pressed(KeyCode::KeyE),
            slots: std::array::from_fn(|index| self.is_key_pressed(SLOT_KEYS[index])),
            cursor: self.cursor,
        }
    }

    /// Clear per-frame amounts. Call once per frame, after [`Input::snapshot`].
    pub fn end_frame(&mut self) {
        self.look_delta = Vec2::ZERO;
        self.scroll = 0.0;
    }

    /// Is `key` currently held?
    pub fn is_key_pressed(&self, key: KeyCode) -> bool {
        self.keys.contains(&key)
    }

    /// Is `button` currently held?
    pub fn is_mouse_pressed(&self, button: MouseButton) -> bool {
        self.mouse_buttons.contains(&button)
    }

    /// `+1` if `positive` is held, `-1` if `negative` is held, else `0`.
    fn axis(&self, positive: KeyCode, negative: KeyCode) -> f32 {
        f32::from(self.is_key_pressed(positive)) - f32::from(self.is_key_pressed(negative))
    }

    fn press_key(&mut self, key: PhysicalKey, repeat: bool) {
        let PhysicalKey::Code(code) = key else {
            return;
        };
        self.keys.insert(code);

        // Auto-repeat must not count toward double-tap detection.
        if repeat {
            return;
        }
        if code == KeyCode::Space
            && self
                .space_double_tap
                .press(Instant::now(), self.double_tap_window)
        {
            self.flying = !self.flying;
            log::info!(
                "flight {}",
                if self.flying { "enabled" } else { "disabled" }
            );
        }
    }

    fn release_key(&mut self, key: PhysicalKey) {
        if let PhysicalKey::Code(code) = key {
            self.keys.remove(&code);
        }
    }

    fn release_all(&mut self) {
        self.keys.clear();
        self.mouse_buttons.clear();
        self.look_delta = Vec2::ZERO;
        self.scroll = 0.0;
        self.space_double_tap.reset();
        // `flying` is a persistent mode, so it survives a focus change.
    }
}

/// A per-frame description of what the player wants to do.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerInput {
    /// Desired horizontal movement relative to the camera: `x` = right, `y` =
    /// forward, in the range -1..=1 (diagonals are normalised to unit length).
    pub movement: Vec2,
    /// Space held: jump when grounded, ascend while flying.
    pub jump: bool,
    /// Shift held: sneak when grounded, descend while flying.
    pub sneak: bool,
    /// Ctrl held: sprint.
    pub sprint: bool,
    /// Flight mode is enabled.
    pub flying: bool,
    /// Mouse motion since the previous frame, for camera yaw/pitch.
    pub look: Vec2,
    /// Mouse-wheel ticks since the previous frame (positive = away from the user).
    pub scroll: f32,
    /// Left mouse button held (break/attack in the world, pick up/drop on the inventory).
    pub primary: bool,
    /// Right mouse button held (place/use in the world, pick up/drop on the inventory).
    pub secondary: bool,
    /// `E` held: toggles the creative inventory.
    pub inventory: bool,
    /// The 1–9 keys, in slot order.
    pub slots: [bool; 9],
    /// Cursor position in physical pixels, for the inventory screen.
    pub cursor: Vec2,
}

/// Convert a wheel event into fractional "ticks".
fn wheel_ticks(delta: &MouseScrollDelta) -> f32 {
    match delta {
        // One line is a single detent of a typical mouse wheel.
        MouseScrollDelta::LineDelta(_, y) => *y,
        // Touch-pads report pixels; treat ~100 px as one tick.
        MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / 100.0,
    }
}

/// Detects two presses that land within a short window of each other.
#[derive(Debug, Default)]
struct DoubleTap {
    last_press: Option<Instant>,
}

impl DoubleTap {
    /// Record a press happening at `now`; returns `true` if it completed a
    /// double-tap. After a double-tap the detector resets, so a third quick
    /// press starts a fresh pair rather than toggling again.
    fn press(&mut self, now: Instant, window: Duration) -> bool {
        let is_double = self
            .last_press
            .is_some_and(|last| now.duration_since(last) <= window);
        self.last_press = if is_double { None } else { Some(now) };
        is_double
    }

    fn reset(&mut self) {
        self.last_press = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two presses inside the window complete a double-tap.
    #[test]
    fn double_tap_within_window_triggers() {
        let window = Duration::from_millis(300);
        let mut tap = DoubleTap::default();
        let start = Instant::now();

        assert!(!tap.press(start, window));
        assert!(tap.press(start + Duration::from_millis(100), window));
    }

    /// Two presses farther apart than the window do not.
    #[test]
    fn double_tap_outside_window_does_not_trigger() {
        let window = Duration::from_millis(300);
        let mut tap = DoubleTap::default();
        let start = Instant::now();

        assert!(!tap.press(start, window));
        assert!(!tap.press(start + Duration::from_millis(500), window));
    }

    /// A triple tap toggles once, then starts a fresh pair (not a second toggle).
    #[test]
    fn triple_tap_does_not_double_toggle() {
        let window = Duration::from_millis(300);
        let mut tap = DoubleTap::default();
        let start = Instant::now();

        assert!(!tap.press(start, window));
        assert!(tap.press(start + Duration::from_millis(50), window));
        assert!(!tap.press(start + Duration::from_millis(100), window));
    }

    /// A line-detent wheel event maps straight through to ticks.
    #[test]
    fn wheel_line_delta_is_ticks() {
        assert_eq!(wheel_ticks(&MouseScrollDelta::LineDelta(0.0, 2.0)), 2.0);
    }
}
