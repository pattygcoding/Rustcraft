//! Graphics layer.
//!
//! The rendering boilerplate lives here, kept deliberately separate from game
//! logic so the two can evolve independently:
//!
//! * [`context::GraphicsContext`] — one-time GPU setup (instance, device, queue
//!   and the window's presentation surface) plus swap-chain reconfiguration.
//! * [`renderer::Renderer`] — turns state into GPU draw calls. Right now it only
//!   clears the screen, leaving a window with nothing in it.

pub mod context;
pub mod mesh;
pub mod renderer;
pub mod texture;

pub use renderer::Renderer;
