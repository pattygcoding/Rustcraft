//! The player's view into the world.
//!
//! A first-person **fly camera**: yaw/pitch come from the mouse (see
//! [`crate::input`]), WASD moves along the ground plane relative to where you
//! look, and Space/Shift move up and down. There is no gravity or collision yet —
//! that arrives with the player/physics milestone — so this is effectively a
//! free-fly camera. Enabling flight (`input.flying`) simply moves it faster.

use glam::camera::rh::proj::directx;
use glam::camera::rh::view::look_at_mat4;
use glam::{Mat4, Vec2, Vec3};

use crate::input::PlayerInput;

/// How close to straight up/down the camera may look, in radians (~89°).
/// Stopping short of ±90° avoids a degenerate view matrix (looking parallel to up).
const PITCH_LIMIT: f32 = 1.553_343;

/// A first-person camera.
pub struct Camera {
    /// World-space position, in "blocks".
    pub position: Vec3,
    /// Rotation about the world Y axis, in radians. `0` looks toward `-Z`.
    pub yaw: f32,
    /// Vertical look angle, in radians. Positive looks up.
    pub pitch: f32,
    /// Radians of rotation per unit of mouse motion.
    pub sensitivity: f32,
    /// Walking speed, in blocks per second.
    pub walk_speed: f32,
    /// Flying speed, in blocks per second.
    pub fly_speed: f32,
    /// Speed multiplier applied while sprinting.
    pub sprint_multiplier: f32,
    /// Vertical field of view, in radians.
    pub fov_y: f32,
    /// Near clip distance.
    pub z_near: f32,
    /// Far clip distance.
    pub z_far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        // A vantage overlooking the superflat world (which now streams around you).
        let mut camera = Self::new(Vec3::new(0.0, 90.0, 80.0));
        camera.pitch = -0.4;
        camera
    }
}

impl Camera {
    /// Create a camera at `position`, looking down `-Z` with no pitch.
    pub fn new(position: Vec3) -> Self {
        Self {
            position,
            yaw: 0.0,
            pitch: 0.0,
            sensitivity: 0.0022,
            walk_speed: 5.0,
            fly_speed: 14.0,
            sprint_multiplier: 2.0,
            fov_y: 60f32.to_radians(),
            z_near: 0.05,
            z_far: 1000.0,
        }
    }

    /// Apply one frame of mouse-look and movement from `input`.
    ///
    /// `dt` is the time since the previous frame, in seconds.
    pub fn update(&mut self, input: &PlayerInput, dt: f32) {
        self.look(input.look);
        self.apply_movement(input, dt);
    }

    /// Rotate the view by a raw mouse delta, in pixels.
    fn look(&mut self, delta: Vec2) {
        // Moving the mouse right turns right; moving it up looks up.
        self.yaw += delta.x * self.sensitivity;
        self.pitch -= delta.y * self.sensitivity;
        self.pitch = self.pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Translate the camera according to the movement keys.
    fn apply_movement(&mut self, input: &PlayerInput, dt: f32) {
        // Horizontal basis derived from yaw only, so looking up/down does not
        // change which way WASD moves.
        let forward = Vec3::new(self.yaw.sin(), 0.0, -self.yaw.cos());
        let right = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin());

        let mut direction = forward * input.movement.y + right * input.movement.x;
        direction.y += f32::from(input.jump) - f32::from(input.sneak);

        if direction == Vec3::ZERO {
            return;
        }

        let mut speed = if input.flying {
            self.fly_speed
        } else {
            self.walk_speed
        };
        if input.sprint {
            speed *= self.sprint_multiplier;
        }

        self.position += direction.normalize() * speed * dt;
    }

    /// The unit vector the camera is looking along.
    pub fn forward(&self) -> Vec3 {
        Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            -self.yaw.cos() * self.pitch.cos(),
        )
    }

    /// The view matrix (world space -> view space).
    pub fn view_matrix(&self) -> Mat4 {
        look_at_mat4(self.position, self.position + self.forward(), Vec3::Y)
    }

    /// The projection matrix for the given viewport aspect ratio.
    pub fn projection_matrix(&self, aspect: f32) -> Mat4 {
        // `directx::perspective` matches wgpu's NDC (Y-up, Z in [0, 1]).
        directx::perspective(self.fov_y, aspect, self.z_near, self.z_far)
    }

    /// The combined view * projection matrix for the given aspect ratio.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection_matrix(aspect) * self.view_matrix()
    }
}
