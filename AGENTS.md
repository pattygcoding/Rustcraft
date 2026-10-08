# AGENTS.md

Guidance for AI agents (and humans) working on **Rustcraft** — a lightweight,
from-scratch Minecraft clone written in Rust. Keep this file up to date when
you add crates, change the module layout, or finish a roadmap milestone.

## Goal

Build a small, fast voxel sandbox game in the spirit of Minecraft (not a 1:1
clone): a procedurally generated voxel world you can fly/walk around, with
block breaking/placing, in a single native window. "Lightweight" means we own
the rendering, world and physics code rather than pulling in a full game engine.

## Current status

**Superflat world.** A 256×256 block superflat world (16×16 chunks, centred on the
origin, 66 blocks tall): 1 bedrock, 59 stone, 5 dirt, 1 grass. It is meshed with
**hidden-face culling that also spans chunk borders**, into one mesh per chunk,
and drawn from individual PNGs in a `texture_2d_array` (no atlas). 256 chunks →
256 draw calls for ~199k visible faces. Viewed through a first-person fly camera
(`src/camera.rs`). See **Textures & blocks** and **Chunks & meshing**. Next: block
interaction, greedy meshing, noise terrain.

## Tech stack

| Concern | Crate | Version | Why |
| --- | --- | --- | --- |
| Window + input + event loop | `winit` | 0.30 | De-facto standard cross-platform windowing; gives us raw events to build controls on. |
| GPU rendering | `wgpu` | 30.0 | Safe, cross-platform WebGPU-style API (Vulkan/DX12/Metal/GL). Huge voxel-game ecosystem. |
| Math | `glam` | 0.34 | Fast SIMD-friendly `Vec3`/`Mat4` for cameras and chunk transforms. |
| GPU buffer casting | `bytemuck` | 1.25 | `#[derive(Pod)]` for safely uploading vertex/uniform structs to the GPU. |
| Async block-on | `pollster` | 1.0 | Block on wgpu's `async` init calls from a synchronous `main`. |
| Logging | `log` + `env_logger` | 0.4 / 0.11 | wgpu/winit log through the `log` facade; `env_logger` prints them, filtered by `RUST_LOG`. |
| Procedural noise | `noise` | 0.9 | Perlin/Simplex/OpenSimplex for terrain generation. (Not wired up yet.) |
| Textures | `image` | 0.25 | Load PNG block textures for the atlas. (Not wired up yet.) |

**Why not Bevy?** A full engine would hide the voxel-rendering and chunk-meshing
work that is the point of this project, and adds a large dependency tree. We want
direct control over the pipeline. Revisit only if the scope grows enormously.

## Project layout

```
src/
  main.rs          Entry point: logging, `winit` event loop, the `App` handler.
  camera.rs        `Camera`: first-person fly camera (view/projection matrices).
  gfx/
    mod.rs         Graphics module root + re-exports.
    context.rs     `GraphicsContext`: wgpu instance/device/queue/surface + resize.
    renderer.rs    `Renderer`: pipelines, uniform, depth buffer, draws the scene.
    mesh.rs        `Vertex` + `BlockFaces` + `Mesh` (textured block geometry).
    texture.rs     `TextureArray` + `BlockTextures` (one PNG per array layer).
  input.rs         `Input`: keyboard/mouse state -> per-frame `PlayerInput`.
  world/
    mod.rs         World module root + re-exports.
    block.rs       `Block` types and their per-face textures.
    chunk.rs       16×256×16 `Chunk` storage + simple terrain generation.
    mesher.rs      `mesh_chunk`: emits only visible faces (hidden-face culling).
shaders/
  block.wgsl       Textured-block vertex + fragment shader.
resources/
  assets/textures/ Individual block PNGs (one array layer each).
Cargo.toml         Dependencies (edition 2024).
AGENTS.md       This file.
README.md       Short human-facing overview.
```

Suggested growth (create these as the milestones land):

```
src/
  app.rs        (optional split) winit <-> game glue once `main.rs` grows.
  gfx/          Grow further: texture atlas, chunk buffers, render graph.
  world/        Chunk storage, block types, terrain generation.
  player.rs     Player physics/collision (gravity, grounded movement).
assets/         Textures and world data.
shaders/        More WGSL (`*.wgsl`) as pipelines are added.
```

## Textures & blocks

Block textures are **not** packed into an atlas. Each PNG under
`resources/assets/textures/` becomes its own **layer** of a single
`wgpu::Texture` (`dimension = D2`, `depth_or_array_layers = N`) — a
`texture_2d_array`. The shader selects a layer per vertex.

Why this beats an atlas:

* **Flat cost as textures grow** — every block type shares one bind group and one
  draw call, so adding textures adds no draw calls or bind switches.
* **No mip bleeding** — layers are independent, so (future) mipmaps never sample
  a neighbouring texture.
* **Cheap animation** — update one layer instead of re-uploading a whole atlas.
* **Mixed sizes are fine** — each source is resized to `TEXTURE_SIZE` (nearest).

Rules of the road:

* **Add a texture:** drop `my_block.png` into `resources/assets/textures/`. It is
  addressed by its file stem: `textures.layer("my_block")`. Layer indices are
  assigned in sorted file order, so they are stable between runs.
* **Multi-sided / entity blocks:** use `mesh::BlockFaces` —
  `BlockFaces::uniform(l)` (all six faces), `BlockFaces::column(top, side, bottom)`
  (grass), or `BlockFaces::new([+X, -X, +Y, -Y, +Z, -Z])` for anything else. Face
  order is `[+X, -X, +Y, -Y, +Z, -Z]`; each vertex carries a `layer` index that the
  shader samples.
* **Cutout blocks** (leaves, foliage, fences): the fragment shader `discard`s
  fragments with alpha < 0.5, so transparent texels vanish in the opaque pass with
  no blending.
* **Blended blocks** (glass, water): build them with
  `PlacedBlock { transparent: true, .. }`. They go into a second index range drawn
  after the opaque pass with alpha blending and depth writes off (see
  `renderer::render` and `create_pipeline`).
* **Animated blocks**: call `TextureArray::update_layer(queue, layer, image)` each
  frame (or every few frames) for just that layer — far cheaper than an atlas.

Current limits / next steps:

* `mip_level_count` is 1 (no mipmaps yet). When added, generate them per-layer so
  layers do not bleed.
* Transparent geometry is not yet sorted back-to-front (fine for convex blocks
  like glass; matters for e.g. stacked water).
* The transparent pipeline needs at least one `transparent: true` block in the
  scene to actually draw a second pass.

## Chunks & meshing

The world is divided into **chunks** of `16 × 256 × 16` blocks (`world::chunk`),
stored as one flat `Vec<Block>` indexed `[y][z][x]` (~64 KiB per chunk). `Block`
is a `#[repr(u8)]` enum, so each block is one byte.

Meshing (`world::mesher::mesh_chunk`) is where the efficiency comes from:

* For every solid block and each of its six faces, the face is emitted **only if
  the neighbouring block is not solid** (`Block::is_solid`). Interior faces — the
  majority in a filled chunk — produce no vertices, indices or triangles at all.
  (The current world meshes to ~199k visible faces across 256 chunks, vs. ~26M
  faces if every one of its ~4.3M solid blocks drew all six.)
* Neighbour lookups cross chunk borders via `World::block`, so faces shared by two
  chunks are culled too — only the world's outer shell is drawn. An isolated chunk
  reads air at its border and so draws its whole shell.
* A whole chunk becomes **one mesh and one draw call**, so per-frame cost is
  independent of how many blocks the chunk holds.

`World::superflat()` builds the world as `CHUNKS_PER_AXIS²` (16×16 = 256) chunks,
centred on the origin; each `Chunk::superflat()` places 1 bedrock, 59 stone, 5 dirt
and 1 grass (66 blocks tall). `World::block()` samples any world position so the
mesher can cull across chunk borders, and `World::iter_chunks()` yields each
chunk's world-space origin, which the mesher bakes into vertex positions — so all
chunks share one view-projection uniform and no per-chunk model matrix.

Still to come: dirty-chunk remeshing on edit, **greedy meshing** (merging coplanar
quads — a flat grass field would collapse to a handful of quads), and meshing on
background threads.

## Commands

Run everything from the repository root with PowerShell:

```powershell
cargo build                 # compile
cargo run                   # run the game (opens a window)
RUST_LOG=info cargo run     # same, with wgpu/winit logging enabled

cargo fmt                   # format
cargo clippy --all-targets -- -D warnings   # lint (must stay clean)
cargo test                  # unit/integration tests
```

Notes:

* There are no tests yet. Add them alongside new pure logic (world gen, meshing,
  raycasting, chunk maths) — those are the easy things to unit-test.
* `RUST_LOG` controls verbosity (e.g. `RUST_LOG=rustcraft=debug,wgpu=warn`).
  The app's own messages use the `rustcraft` target.

## Conventions

* **Rust edition 2024**, stable toolchain. Formatted with `rustfmt` defaults.
* **Lint-clean:** code must pass `cargo clippy --all-targets -- -D warnings`.
* **Logging, not `println!`:** use the `log` macros (`log::info!`, `log::debug!`,
  `log::warn!`, `log::error!`). Reserve `println!` for genuine stdout output only.
* **Error handling:** at startup, unrecoverable conditions may `panic!`/`expect`
  (e.g. no GPU adapter). Prefer returning `Result` / `Option` for anything that
  can fail at runtime, and avoid `unwrap()` in logic that can be defended against.
* **`main` returns `Result`** so `?` works for event-loop and setup errors.
* **Document public items** with `///` docs explaining the *why*, not just the
  *what*. Module files start with a `//!` module doc.
* **GPU labels:** give wgpu resources a `label` (`Some("...")`) to make GPU
  debugger output legible.
* **Scope for "lightweight":** prefer plain data structures and a hand-rolled
  ECS-lite over adding heavy frameworks. Justify any new dependency in this file.

## Controls

`src/input.rs` turns raw events into a per-frame `PlayerInput`. Current mapping
(the classic Minecraft layout):

| Input | Action |
| --- | --- |
| Mouse move | Look (yaw/pitch), from raw `DeviceEvent::MouseMotion` |
| `W` / `A` / `S` / `D` | Move forward / left / back / right (relative to view) |
| `Space` | Move up |
| `Shift` | Move down |
| `Space` ×2 (double-tap) | Toggle flight (faster movement) |
| `Ctrl` | Sprint (speed multiplier) |
| `Esc` | Release the cursor |
| Left click | Grab the cursor again |
| Left / right button | Break / place (captured, unused until implemented) |
| Scroll wheel | Hotbar selection (captured, unused until implemented) |

Held keys are tracked in a set and cleared when the window loses focus; mouse
look uses raw `DeviceEvent::MouseMotion` so it keeps working while the cursor is
grabbed; auto-repeat is ignored for double-tap detection. The cursor is grabbed
on startup — `Esc` frees it, a left click grabs it again.

`Space`/`Shift` currently map to **up/down** because there is no gravity or
ground yet; once the player/physics milestone lands they become jump/sneak and
the camera follows a grounded player. The input layer already exposes `jump`/
`sneak`, so only `src/camera.rs` / the future `player.rs` change.

## API notes (read before copying old tutorials!)

The pinned versions are *new* and their APIs differ from most blog posts and
older examples. Everything below is verified against the vendored sources.

### wgpu 30

* `Instance::new(desc: InstanceDescriptor)` takes the descriptor **by value**.
  `InstanceDescriptor` has no `Default`; start from
  `wgpu::InstanceDescriptor::new_without_display_handle()` and use struct-update
  syntax, e.g. `InstanceDescriptor { backends: Backends::all(), ..new_without_display_handle() }`.
* `instance.create_surface(target)` returns `Result<Surface, CreateSurfaceError>`.
  Pass an `Arc<Window>` to get a `Surface<'static>` (see the lifetime note below).
* `instance.request_adapter(&RequestAdapterOptions)` returns
  `Result<Adapter, RequestAdapterError>` (it used to return `Option`).
* `adapter.request_device(&DeviceDescriptor)` takes **one** argument and returns
  `Result<(Device, Queue), RequestDeviceError>` (the old `trace` argument is gone;
  tracing now lives in `DeviceDescriptor::trace`).
* Configure a surface with `surface.get_default_config(&adapter, width, height)
  -> Option<SurfaceConfiguration>` rather than building the config by hand.
* **Big one:** `surface.get_current_texture()` now returns a
  `wgpu::CurrentSurfaceTexture` **enum**, not `Result<SurfaceTexture, SurfaceError>`.
  Match `Success`/`Suboptimal`/`Timeout`/`Occluded`/`Outdated`/`Lost`/`Validation`.
* **Presentation moved:** after `queue.submit(...)` you must call
  `queue.present(surface_texture)` — `SurfaceTexture` no longer has a `present()`
  method. `present` consumes the `SurfaceTexture`.
* `RenderPassColorAttachment` has a `depth_slice: Option<u32>` field (set `None`).
* `RenderPassDescriptor` gained `timestamp_writes`, `occlusion_query_set` and
  `multiview_mask`; use `..Default::default()` for the ones you don't set.
* `DeviceDescriptor`, `RenderPassDescriptor`, `TextureViewDescriptor` derive
  `Default`; `CommandEncoderDescriptor` does not (use `::default()` or set `label`).
* **`Surface::configure` panics if a frame from `get_current_texture` is still
  alive.** For the `Suboptimal` case, draw + present the frame first and
  reconfigure *after* it is released (see `gfx::renderer`), never while holding it.
* Pipeline descriptors are `Option`-heavy in 30: `VertexState::buffers`,
  `FragmentState::targets` and `PipelineLayoutDescriptor::bind_group_layouts` are
  slices of `Option<..>`; `entry_point` is `Option<&str>`;
  `PipelineLayoutDescriptor` names the push-constant size `immediate_size`; and
  `DepthStencilState::{depth_write_enabled, depth_compare}` are `Option`s.

### glam 0.34

* Matrix constructors moved into `glam::camera`. Use
  `glam::camera::rh::proj::directx::perspective(..)` (Y-up, Z ∈ [0, 1] — matches
  wgpu) and `glam::camera::rh::view::look_at_mat4(..)`. The old
  `Mat4::perspective_rh` / `Mat4::look_at_rh` no longer exist.

### winit 0.30

* Uses the `ApplicationHandler` trait driven by `EventLoop::run_app(&mut app)`.
  The required methods are `resumed(&mut self, &ActiveEventLoop)` and
  `window_event(&mut self, &ActiveEventLoop, WindowId, WindowEvent)`.
* **Create the window in `resumed`**, not before `run_app` — several platforms
  forbid creating a surface earlier. Guard against redundant `Resumed` events.
* Create windows with `event_loop.create_window(Window::default_attributes()...)`.
* Exit with `event_loop.exit()`.

### Avoiding a self-referential borrow

`wgpu::Surface<'window>` borrows its window, but `winit::Window` is not `'static`.
Store the window as `Arc<Window>` and pass the `Arc` to
`instance.create_surface(...)`; `Arc<Window>` satisfies wgpu's `SurfaceTarget`
bound, yielding a `Surface<'static>`. This lets `App` hold both the window and
the renderer without a lifetime tangle.

## Roadmap

Work milestones in order; update the **Current status** section as they land.

0. **Hello world** — window + clear colour. ✅ done.
1. **First geometry** — a spinning cube via a `wgpu::RenderPipeline`, vertex +
   index buffers, a uniform transform, a WGSL shader and a depth buffer. ✅ done.
2. **Camera** — `glam` view/projection matrices in a uniform buffer; first-person
   fly camera with mouse-look and cursor grab; depth buffer. ✅ done.
3. **Chunks** — 16×256×16 chunks, a hidden-face-culling mesher that also culls
   across chunk borders, and a 16×16 grid forming a 256×256 world. ✅ done (greedy
   meshing + streaming still to come).
4. **Terrain** — fill chunks from `noise` (e.g. fBm) to get rolling hills.
5. **Blocks & textures** — ✅ individual PNGs in a `texture_2d_array` (no atlas),
   per-face textures, cutout + transparent passes. Remaining: animated textures,
   then greedy meshing (which lands with chunks).
6. **Player** — WASD + mouse-look, gravity and AABB collision, jumping.
7. **Interaction** — voxel raycast for break/place; rebuild affected chunk mesh.
8. **Streaming** — generate/mesh chunks around the player on background threads;
   unload distant chunks.
9. **Persistence** — save/load the world (e.g. a simple binary or region format).

## Notes on running

* The app needs a working GPU driver. On this machine it selects the discrete
  GPU (Vulkan). A harmless `wgpu_hal::vulkan::instance` warning about layer
  manifest registry lookups is normal on Windows without the Vulkan SDK.
* `cargo run` opens a real window and blocks until you close it.
* `Cargo.lock` is committed (application, not library) so builds are reproducible.



