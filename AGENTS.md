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

**Infinite world you can edit.** A **noise-generated** landscape — rolling hills,
cliffs, the odd overhang, and **translucent oceans** flooded up to `SEA_LEVEL`
(y = 62) — from a
3D density field of combined multi-octave Perlin noise (bedrock → stone → dirt →
grass) **streams around the player**: only
chunks within `RENDER_RADIUS` are kept loaded; as the player moves, far chunks are
dropped and near ones generated, so the world extends forever while memory stays
bounded. Chunks are meshed with **cross-chunk hidden-face culling** into one mesh
(one draw call) each, drawn from individual PNGs in a `texture_2d_array` (no atlas).
A first-person fly camera; **right-click breaks** and **left-click places** the
block the camera is looking at (a voxel raycast), chosen from a **hotbar HUD** you
scroll through. **Minecraft-style sky lighting (levels 0–15)** darkens overhangs and
caves, and shades each face by which way it points. See **Textures & blocks** (incl.
**Lighting** and **Transparency**), **Terrain** and **Chunks & meshing**. Next:
greedy meshing, player physics, biomes.

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
  hotbar.rs        `Hotbar`: placeable blocks + selection (+ HUD geometry).
  gfx/
    mod.rs         Graphics module root + re-exports.
    context.rs     `GraphicsContext`: wgpu instance/device/queue/surface + resize.
    renderer.rs    `Renderer`: pipelines, uniform, depth buffer, draws the scene.
    mesh.rs        `Vertex`, `BlockFaces`, `MeshData`/`Mesh` (opaque + translucent).
    texture.rs     `TextureArray` + `BlockTextures` (one PNG per array layer).
  input.rs         `Input`: keyboard/mouse state -> per-frame `PlayerInput`.
  world/
    mod.rs         `World`: streaming set of loaded chunks + world-space lookups.
    block.rs       `Block` types and their per-face textures.
    chunk.rs       16×256×16 `Chunk`: blocks, plus 0–15 sky light per cell.
    light.rs       `sky_light`: sunlight columns + a flood-fill spread.
    terrain.rs     `Terrain`: 3D `Fbm<Perlin>` density field + chunk generation.
    mesher.rs      `mesh_chunk`: emits only visible faces (hidden-face culling).
shaders/
  block.wgsl       Textured-block vertex + fragment shader.
  ui.wgsl          Screen-space HUD shader (the hotbar).
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
  shader samples, plus the packed lighting for that face (see **Lighting** below).
* **Greyscale textures** (water): do not bake the colour into the image. Give the
  block a colour in `Block::tint()` (`0xAARRGGBB`); the mesher writes it per vertex
  and the shader multiplies it over the sample. Water uses Minecraft's blue
  `0x3F76E4` with a translucent alpha; untinted blocks send opaque white, which
  leaves the texture alone.
* **Cutout blocks** (leaves, foliage, fences): the fragment shader `discard`s
  fragments with alpha < 0.5, so transparent texels vanish in the opaque pass with
  no blending.
* **Blended blocks** (water): report them from `Block::is_translucent()`, and the
  mesher puts their faces in the **translucent** set instead of the opaque one. The
  renderer draws that set after the opaque geometry, farthest chunk first, with
  alpha blending and depth writes off — see **Transparency** below.
* **Animated blocks**: call `TextureArray::update_layer(queue, layer, image)` each
  frame (or every few frames) for just that layer — far cheaper than an atlas.

Current limits / next steps:

* `mip_level_count` is 1 (no mipmaps yet). When added, generate them per-layer so
  layers do not bleed.
* Transparent geometry is not yet sorted back-to-front (fine for convex blocks
  like glass; matters for e.g. stacked water).
* The transparent pipeline needs at least one translucent block in the scene to
  actually draw a second pass (water supplies that).

### Lighting

Light lives in the chunk (`Chunk::light`, one `u8` per cell) and is recomputed by
`world::light` whenever a chunk is generated or a block is edited. It follows
Minecraft's rules, at **levels 0–15**:

* **Sunlight** falls straight down at full strength, so every cell with open sky
  above it is **15**.
* From there light spreads one block at a time in all six directions, **losing a
  level each step**, and solid blocks stop it dead.
* So the world is bright in the open and fades to black under overhangs and deep
  inside caves. (Light does not cross chunk borders; sunlight is vertical, so all
  that is lost is the sideways bleed where a shadow straddles a border.)

The mesher bakes, per face, the light of the open cell that face looks into, plus
the face index — packed into one `u32` by `mesh::pack_light` (sky light in bits 0–3,
block light 4–7, face 8–10; both channels are 0–15). The fragment shader then:

1. takes `max(sky_light, block_light)` for that cell — block light is 0 for now,
   since nothing emits light yet, and it is always daytime so there is no day/night
   scaling;
2. runs it through **Minecraft's lightmap curve** (`15 → 1.0`, `8 → 0.22`, `0 → 0`),
   so shadow deepens quickly with distance from the sky;
3. multiplies by a **per-face shade** — **top 1.0, north/south 0.8, east/west 0.6,
   bottom 0.5**. That is the trick that makes a block's sides read as solid: without
   it every face of a flat-coloured cube looks identical.

The multiply happens in linear space (textures and the render target are sRGB), so
the shader raises the factor to `2.2` to land the shading on the gamma-encoded
texture the way Minecraft does, and floors it so unlit caves are very dark grey
rather than pure black.

Known limits: lighting is per **face**, not per vertex corner, so there is no
smooth/ambient-occlusion darkening in corners yet; and nothing emits block light.

### Transparency

Water is **translucent**, so it cannot be drawn in the opaque pass. Each chunk's
mesh is therefore split in two (`gfx::mesh::MeshData`): an **opaque** set
(depth-writing, unblended) and a **translucent** set (blended, depth writes off).
They are separate vertex *and* index buffers, so each pass binds exactly what it
needs with no index offsets to reason about, and a chunk with no water allocates
nothing for the blended pass.

The renderer draws them in the only order that is correct:

1. **Opaque geometry**, nearest chunk first, depth writes on. Front-to-back is an
   optimisation: the depth buffer then rejects hidden fragments as early as it can.
2. **Translucent geometry**, farthest chunk first, blended, testing depth but not
   writing it. Depth writes must stay off — a translucent surface must not hide
   what is blended behind it — which is exactly why the order has to be back to
   front: nearer water blends *over* the water behind it.

`Renderer::order_chunks` sorts the loaded chunks by distance once per frame into a
reused `Vec` (no per-frame allocation); the opaque pass walks it forwards and the
blended pass walks it backwards, so one sort serves both.

Face culling follows the same split (`Block::hides_face_of`): an opaque neighbour
hides a face, a translucent one hides only faces of its own kind. So the sea has no
faces between its own cells, while the seabed *is* drawn — and seen — through the
water.

Water is also see-through to gameplay: `World::raycast` stops at the first **opaque**
block, so looking at the sea targets the ground beneath it and blocks can be placed
into water.

Still to come: an underwater fog/overlay (`water_overlay.png`), and sorting the
translucent *faces within* a chunk — with one flat sea at `SEA_LEVEL` at most one
translucent surface lies along any view ray, so chunk ordering is enough for now.

## Chunks & meshing

The world is divided into **chunks** of `16 × 256 × 16` blocks (`world::chunk`),
stored as one flat `Vec<Block>` indexed `[y][z][x]` (~64 KiB per chunk). `Block`
is a `#[repr(u8)]` enum, so each block is one byte.

Meshing (`world::mesher::mesh_chunk`) is where the efficiency comes from:

* For every solid block and each of its six faces, the face is emitted **only if
  the neighbouring block is not solid** (`Block::is_solid`). Interior faces — the
  majority in a filled chunk — produce no vertices, indices or triangles at all.
  (The loaded region meshes to roughly ~140k visible faces across ~169 chunks,
  vs. millions if every block face were drawn.)
* Neighbour lookups cross chunk borders via `World::block`, so faces shared by two
  chunks are culled too — only the world's outer shell is drawn. An isolated chunk
  reads air at its border and so draws its whole shell.
* A whole chunk becomes **one mesh and one draw call**, so per-frame cost is
  independent of how many blocks the chunk holds.

`Terrain::generate_chunk` fills each column: bedrock at the bottom, stone, a few
blocks of dirt, then grass on top. `World` keeps a `HashMap<ChunkPos, Chunk>` of
the chunks near the player; `World::update(player_x, player_z)` streams them —
dropping out-of-range chunks, generating in-range ones — and marks a **dirty set**
of chunks that need (re)meshing. Because a chunk's mesh depends on whether its
*neighbours* are loaded (culling), both generated chunks and their neighbours are
marked dirty.

### Terrain

`src/world/terrain.rs` shapes the land the way modern Minecraft does: as a **3D
noise field**, not a heightmap. Each point gets a *density*, and a block is solid
where it is positive:

    density(x, y, z) = (surface_target(x, z) - y) + DETAIL * noise_at(x, y, z)

* `surface_target` is a 2D field: a low-frequency **continent** layer (wavelength
  ~180 blocks) scaled by a **relief** layer, so the world has open plains and
  mountain ranges.
* `noise_at` is the **combined multi-octave 3D Perlin** field in **`[-1, 1]`** — two
  `Fbm<Perlin>` layers (a coarse *rough* one and a finer *detail* one) blended with
  weights that sum to 1. Its `y` is stretched (`Y_STRETCH`) so it varies faster
  vertically than horizontally; that vertical variation is what grows **overhangs,
  cliffs and caves** rather than a smooth sheet (~3% of columns end up with air
  pocketed beneath their surface).
* `DETAIL` (±35 blocks) is how far the 3D noise can push the surface.

`(surface_target - y)` falls by one per block going up, so more than `WINDOW` blocks
either side of the target the answer is certain — solid below, air above — and only
that narrow band is ever sampled. Within it the field is read every `STEP` (4) blocks
down the column and **linearly interpolated** in between, exactly like Minecraft's
density noise. Sampling is the current hot spot (~2 s for the initial 169 chunks in
a debug build); background-thread generation is on the roadmap.

Everything is a pure function of `(seed, x, y, z)`, so chunks agree at their borders
and the world is reproducible. The seed comes from `RUSTCRAFT_SEED`, or `DEFAULT_SEED`
if unset. The camera spawns standing on the surface.

### Water

`SEA_LEVEL` is **62**: once a column's ground is laid down, any open space left
between it and the sea is filled with `Block::Water`, and the surface block is
**grass above the waterline but bare dirt below it** — a drowned lawn would look
wrong. Only the space *above* a column's surface floods, so the noise's caves stay
dry. The camera stands on the sea when its spawn column is under it.

Note the numbers: `BASE_HEIGHT` is 64 and `SEA_LEVEL` is 62, so a little under half
the world ends up as ocean. Both are single constants to taste.

### Chunk streaming

`World::update` is called every frame but only does work when the player crosses a
chunk boundary. `Renderer::update_chunks` drops meshes for unloaded chunks and
(re)meshes up to `MESH_BUDGET` dirty chunks **per frame, nearest first**, so
streaming never stalls a frame — far chunks are replaced by near ones a few at a
time. `World::block()` samples any world position so the mesher culls across chunk
borders, and the mesher bakes each chunk's world origin into its vertex positions,
so all chunks share one view-projection uniform (no per-chunk model matrix).

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
| Left click | Place the selected block (or grab the cursor when it is free) |
| Right click | Break the block the camera is looking at |
| Scroll wheel | Change the selected block in the hotbar |

Held keys are tracked in a set and cleared when the window loses focus; mouse
look uses raw `DeviceEvent::MouseMotion` so it keeps working while the cursor is
grabbed; auto-repeat is ignored for double-tap detection. The cursor is grabbed
on startup — `Esc` frees it, a left click grabs it again.

**Block editing** raycasts from the camera through the voxel grid
(`World::raycast`, an Amanatides–Woo DDA) up to `REACH` (6) blocks, returning the
block hit *and* the face it was entered through. **Right-click breaks** it;
**left-click places** the selected block in the empty cell against that face. Edits
go through `World::set_block`, which marks the chunk — and any touched border
neighbour — dirty for re-meshing. Bedrock is unbreakable, so you cannot mine
through the bottom.

**Hotbar**: `src/hotbar.rs` holds the placeable blocks and the selection; the mouse
wheel changes it. The renderer draws it as a screen-space HUD (`shaders/ui.wgsl` —
a 2D pipeline that reuses the block texture array and bind group), with the selected
slot enlarged so you can see what you will place.

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
4. **Terrain** — ✅ a 3D density field of combined multi-octave `Fbm<Perlin>` noise
   (blended layers, linearly interpolated down each column, like Minecraft), giving
   hills, cliffs and overhangs, plus **oceans filled to `SEA_LEVEL`** and drawn
   translucently (see **Transparency**). Still to come: biomes and flowing water.
5. **Blocks & textures** — ✅ individual PNGs in a `texture_2d_array` (no atlas),
   per-face textures, cutout + transparent passes. Remaining: animated textures,
   then greedy meshing (which lands with chunks).
6. **Lighting** — ✅ Minecraft-style sky light at levels 0–15 (vertical sunlight +
   a flood-fill spread), baked per face along with Minecraft's directional face
   shading. Still to come: block light from torches, cross-chunk propagation, and
   smooth per-vertex ambient occlusion.
7. **Player** — WASD + mouse-look, gravity and AABB collision, jumping.
8. **Interaction** — ✅ voxel raycast break (right-click) and place (left-click),
   with a scrollable hotbar HUD; affected chunks re-mesh automatically.
9. **Streaming** — ✅ chunks stream around the player (load/unload with a per-frame
   mesh budget, nearest first). Still to come: generating/meshing on background
   threads.
10. **Persistence** — save/load the world (e.g. a simple binary or region format).

## Notes on running

* The app needs a working GPU driver. On this machine it selects the discrete
  GPU (Vulkan). A harmless `wgpu_hal::vulkan::instance` warning about layer
  manifest registry lookups is normal on Windows without the Vulkan SDK.
* `cargo run` opens a real window and blocks until you close it.
* `Cargo.lock` is committed (application, not library) so builds are reproducible.



