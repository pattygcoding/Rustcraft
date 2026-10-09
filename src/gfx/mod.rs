//! Graphics layer.
//!
//! The rendering boilerplate lives here, kept deliberately separate from game
//! logic so the two can evolve independently:
//!
//! * [`context::GraphicsContext`] — one-time GPU setup (instance, device, queue
//!   and the window's presentation surface) plus swap-chain reconfiguration.
//! * [`renderer::Renderer`] — turns state into GPU draw calls: it fills a G-buffer
//!   from the world's geometry, lights it in one fullscreen pass, and draws the
//!   translucent geometry and the HUD on top.
//! * [`shadow::Sun`] — the sun's direction and the matrix that renders the world
//!   from its point of view, which is what puts real shadows in the lighting pass.

pub mod context;
pub mod mesh;
pub mod renderer;
pub mod shadow;
pub mod texture;

pub use renderer::Renderer;
