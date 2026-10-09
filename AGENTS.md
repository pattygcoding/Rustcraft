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
bounded. Generating a chunk is the expensive part, so it happens on **worker threads**
(see **Chunk streaming**): the frame loop only ever collects what they have finished, and
it opens on the spawn — the chunks around it, in under a second, instead of a
three-second freeze — with the rest of the patch filling in around you. Dry grass open to
the sky has a 5-in-1000 chance of a short **oak** on it.
Then the ground is **carved** (see **Caves & ravines**): Minecraft Beta's own carvers walk
**worms** of ellipsoids through the rock — winding **caves** that fork into networks, with the odd
room — and, one chunk in fifty, a long **ravine**: a gash with walls, stretched vertically, that
cuts down as it runs. Every cell still open at `LAVA_LEVEL` (y = 11) is a **lava sea**, so a shaft
deep enough ends in a glowing pool with caves around it to explore. The carvers are pure functions
of `(seed, position)` like the terrain, so caves cross chunk borders seamlessly even though the
chunks either side arrive from different threads.
Chunks are meshed with **cross-chunk hidden-face culling** into one mesh
(one draw call) each, drawn from individual PNGs in a `texture_2d_array` (no atlas).
A first-person fly camera; **right-click breaks** and **left-click uses what is in hand** on the
block the camera is looking at (a voxel raycast), held in **nine inventory slots**
chosen with `1`–`9` or the wheel — or picked straight off the world with the **wheel button**,
which makes what you are aimed at the thing in hand. A **crosshair** marks the middle of the
screen — where a click's raycast starts, so it is the block being broken, used or picked — and
hides while a screen
stands between the player and the world. `E` opens a **creative inventory** where everything you
can hold — grass, dirt, stone, oak log, oak planks, oak leaves, poppy, dandelion, glass,
**bedrock** and three **buckets** — is drawn as a little 3D cube, or as its own sprite in the
case of a bucket: click one to pick it up, click a slot to
drop it in. **Buckets** are how the fluids move: a bucket of water or lava pours it into the
world (and is left empty), and an empty bucket scoops a fluid back out — so water and lava are
never held as blocks at all (see **Buckets**). A **caption** above the row names what you are
holding for a moment whenever it changes, and the creative screen's tooltip names what the cursor
is over — both of them reading the labels out of `resources/assets/lang/en.json`, the one language
file (see **The language**). `Esc` opens a **pause menu** — **Continue** or **Exit** — which freezes the world
and takes the cursor while it is up, and whose labels come from a hand-rolled 5×7 **bitmap
font** drawn as flat quads (see **Controls**). The creative screen is a card with a bevelled title
bar and slots, and a tooltip naming whatever the cursor is over; see the HUD paragraph in
**Interaction**. The renderer is **deferred** (see **Rendering**): the world is drawn into a
G-buffer and lit in one fullscreen pass, in **two light channels** (see
**Emissive blocks & block light**): **Minecraft-style sky lighting (levels 0–15)**
— which darkens overhangs and caves, and is *all* the light in a cave — is the *ambient* half,
baked **per
corner** with Minecraft's **smooth lighting** and ambient occlusion, so a shadow's edge is a
gradient and a wall meets the floor in a soft dark crease rather than a step; the **block
light** lava emits is the other, which no shadow may take away, so *a lava pool lights the
cave around it* — the picture that shows the two channels earning their keep. The sky's other
half is a **shadow map**: the same chunk meshes drawn once from the sun's point of view, so
trees, cliffs and walls cast real shadows, thrown west because the sun sits high in the east
(see **Shadows**). Holding **`N`** lights the whole world — caves and overhangs included — as if
every cell held level 15, a *view* option for looking around in the dark (see **Lighting**). The sea is the one surface with a shader of its own: its ripples are a normal
built from the water's own texture read as a height map, plus drifting **fBm**, and the sun catches
it — a highlight and a sheen of sky (see **Transparency**). See **Textures & blocks** (incl.
**Lighting** and **Transparency**),
**Terrain** (incl. **Caves & ravines**) and **Chunks & meshing**. Next: greedy meshing, player
physics, biomes.

## Tech stack

| Concern | Crate | Version | Why |
| --- | --- | --- | --- |
| Window + input + event loop | `winit` | 0.30 | De-facto standard cross-platform windowing; gives us raw events to build controls on. |
| GPU rendering | `wgpu` | 30.0 | Safe, cross-platform WebGPU-style API (Vulkan/DX12/Metal/GL). Huge voxel-game ecosystem. |
| Math | `glam` | 0.34 | Fast SIMD-friendly `Vec3`/`Mat4` for cameras and chunk transforms. |
| GPU buffer casting | `bytemuck` | 1.25 | `#[derive(Pod)]` for safely uploading vertex/uniform structs to the GPU. |
| Async block-on | `pollster` | 1.0 | Block on wgpu's `async` init calls from a synchronous `main`. |
| Logging | `log` + `env_logger` | 0.4 / 0.11 | wgpu/winit log through the `log` facade; `env_logger` prints them, filtered by `RUST_LOG`. |
| Procedural noise | `noise` | 0.9 | Perlin/Simplex/OpenSimplex; the terrain is a `Fbm<Perlin>` field. |
| Textures | `image` | 0.25 | Load the PNG block textures (one `texture_2d_array` layer each). |
| Shader validation | `naga` (dev) | 30 | wgpu's own WGSL front end, for a test that parses and validates every shader with no GPU. Already in the tree as wgpu's dependency, so it costs nothing to build and can never disagree with the validator the renderer uses. |

**Why not Bevy?** A full engine would hide the voxel-rendering and chunk-meshing
work that is the point of this project, and adds a large dependency tree. We want
direct control over the pipeline. Revisit only if the scope grows enormously.

## Project layout

```
src/
  main.rs          Entry point: logging, `winit` event loop, the `App` handler.
  camera.rs        `Camera`: first-person fly camera (view/projection matrices).
  inventory.rs     `Inventory`: nine slots, the creative screen (`E`), HUD geometry.
  item.rs          `Item`: what a slot holds — a block, or a bucket.
  lang.rs          `Lang`: the labels, read from `assets/lang/en.json`.
  pause.rs         `PauseMenu`: the screen `Esc` opens — Continue or Exit.
  font.rs          A 5×7 bitmap font, drawn as flat quads (the menu's labels).
  ui.rs            `Rect`, `push_rect` and `push_bevel`: the primitives every HUD screen shares.
  gfx/
    mod.rs         Graphics module root + re-exports.
    context.rs     `GraphicsContext`: wgpu instance/device/queue/surface + resize.
    renderer.rs    `Renderer`: the four passes, pipelines, uniforms, chunk meshes.
    mesh.rs        `Vertex`, `BlockFaces`, `BlockIcon`, `MeshData`/`Mesh` (2 passes).
    shadow.rs      `Sun`/`ShadowMap`: the sun's direction, box and depth buffer.
    texture.rs     `TextureArray` + `BlockTextures` (one PNG per array layer).
  input.rs         `Input`: keyboard/mouse state -> per-frame `PlayerInput`.
  world/
    mod.rs         `World`: the streaming set of chunks + world-space lookups.
    streaming.rs   `ChunkPool`: the worker threads that generate chunks off-thread.
    block.rs       `Block` types and their per-face textures.
    carve.rs       `carve`: the cave and ravine carvers that cut the rock.
    chunk.rs       16×256×16 `Chunk`: blocks, plus 0–15 sky *and* block light per cell.
    light.rs       `sky_light`/`block_light`: flood-fill spreads, and the light curve.
    random.rs      `JavaRandom`: `java.util.Random`, ported for the carvers' dice.
    terrain.rs     `Terrain`: 3D `Fbm<Perlin>` density field + chunk generation.
    mesher.rs      `mesh_chunk`: emits only visible faces (hidden-face culling).
shaders/
  sun.wgsl         Shared prelude: face shade, the sun/ambient mix, tint maths.
  water.wgsl       Water surface: fBm ripples and a normal from the height map.
  block.wgsl       Block geometry (into the G-buffer), plus the blended water pass.
  lighting.wgsl    Deferred pass: lights the G-buffer in one fullscreen triangle.
  shadow.wgsl      The shadow pass: depth only, and no fragment shader at all.
  ui.wgsl          Screen-space HUD shader (the crosshair, the hotbar and the screens over it).
resources/
  assets/textures/blocks Individual block PNGs (one array layer each).
  assets/textures/items  Item sprites (buckets), in the same array.
  assets/lang/en.json    What the HUD calls every block and item.
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

## Rendering

The frame is **deferred**. Instead of shading each fragment as it is drawn, the world is drawn
once into a **G-buffer** that records *what* each pixel shows — its colour, the way it faces,
how far away it is, and how bright it is — and a single fullscreen pass
then reads that back and puts the light on it. The expensive part, working out whether the sun
reaches a given point, therefore runs once per *pixel on screen* rather than once per fragment
drawn, however many surfaces overlap that pixel. (Honest accounting: for **one** directional
light that is close to a wash — a shadow map alone would do — but a second light then costs a
multiply per pixel instead of another draw of the whole world.)

`Renderer::render` runs four passes:

1. **Shadow** — the chunk meshes again, from the sun's point of view, keeping only depth (see
   **Shadows**).
2. **Geometry** — the opaque and cutout world into the G-buffer. No lighting at all: the
   fragment shader writes what the surface *is* and nothing else.
3. **Lighting** — one fullscreen triangle (no vertex buffer, no attributes: `vs_fullscreen`
   builds it from `vertex_index`) that lights every pixel of the G-buffer onto the window.
4. **Blend** — water, then the HUD, over the lit picture. Translucency *cannot* be deferred —
   it has to blend in order — so water shades itself and is drawn last.

The G-buffer is three textures, one texel per pixel of the window, so all of it is rebuilt on
resize:

| Target | Format | Holds |
| --- | --- | --- |
| albedo | `Rgba8UnormSrgb` | the block's colour in `rgb`, the surface's **sky** brightness in `a` |
| normal | `Rgba8Unorm` | the face normal, as `n * 0.5 + 0.5`, and the surface's **block** brightness in `a` |
| depth | `Depth32Float` | scene depth, and what pass 4 tests against |

Two details there are easy to get wrong:

* **The brightness rides in an alpha channel.** Each is a 0–1 number — the light curve's output,
  averaged per corner and interpolated across the face by the rasterizer (see **Lighting**) — and
  alpha is *not* sRGB-encoded, so it survives the trip through the G-buffer untouched. A whole
  byte is all it needs, because a byte is all a vertex carries. There are two of them because
  sky light and block light have to reach the lighting pass *apart*: the shadow map may take one
  away and must not touch the other (see **Emissive blocks & block light**). The normal's alpha
  was the one channel going spare, and it is where the second one lives.
* **The albedo is sRGB on purpose.** The geometry pass writes linear colour and the hardware
  encodes it; the lighting pass decodes it again (a `textureLoad` from an sRGB texture still
  converts). Both sides then multiply in linear space and agree about what half brightness
  means.
* The normal target is *not* sRGB, and neither brightness is a colour: an alpha value is a
  plain 0–1 number on both sides of the G-buffer, which is exactly why it is the right place
  for a light.

The G-buffer costs eight bytes a pixel. In exchange, lighting leaves the geometry pass
altogether, which is what made a shadow map cheap to add — and a second light channel cost one
of those bytes rather than another pass.

`shaders/sun.wgsl` is a **prelude**, not a shader: WGSL has no `#include`, so `Renderer::compose`
pastes it onto the front of `block.wgsl`, `lighting.wgsl` and `ui.wgsl`. That is how the per-face
shade, the sun/ambient mix, the two-channel mix (`lit_channels`) and the tint maths stay one
definition instead of three that drift.
The light *curve* is deliberately not in there: smooth lighting has to average the light of the
cells around a corner in brightness rather than in levels, so the curve is applied in Rust
(`world::light::brightness`) where the averaging happens, and the shaders only multiply.

## Textures & blocks

Block textures are **not** packed into an atlas. Each PNG under
`resources/assets/textures/blocks/` becomes its own **layer** of a single
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

* **Add a texture:** drop `my_block.png` into `resources/assets/textures/blocks`. It is
  addressed by its file stem: `textures.layer("my_block")`. Layer indices are
  assigned in sorted file order, so they are stable between runs. **Item sprites go in
  `resources/assets/textures/items`** and become layers of the *same* array — a bucket is drawn
  by the same pipeline as a block face, so it needs no array of its own.
* **Animation strips load their first frame.** A PNG taller than it is wide by a whole number of
  times is a sequence of frames stacked one above the other — `water_still.png` is 16×512, which
  is thirty-two 16×16 frames — and `texture::frame_size` takes the top square of it. Without that
  a 16×512 sheet is *squashed* into one 16×16 tile, which is not a smaller version of the picture
  but a different one: fine banding, and — since the water takes its ripples from that texture's
  height — noise instead of ripples. Moving between the frames is what
  `TextureArray::update_layer` is for, and is still to come.
* **Mipmaps are automatic:** each layer is uploaded as its own mip pyramid
  (`texture::downsample`, a box filter that averages *alpha* as well as colour), and the
  sampler filters nearest *within* a level and linear *between* them. A distant block
  therefore samples a pre-averaged texel instead of aliasing, and because the pyramid sits
  inside one layer it can never bleed into a neighbouring texture.
* **Multi-sided / entity blocks:** use `mesh::BlockFaces` —
  `BlockFaces::uniform(l)` (all six faces), `BlockFaces::column(top, side, bottom)`
  (grass), or `BlockFaces::new([+X, -X, +Y, -Y, +Z, -Z])` for anything else. Face
  order is `[+X, -X, +Y, -Y, +Z, -Z]`; each vertex carries a `layer` index that the
  shader samples, plus the packed shading for that corner (see **Lighting** below).
* **Greyscale textures** (water, oak leaves): do not bake the colour into the image.
  Give the block a colour in `Block::tint()` (`0xAARRGGBB`); the mesher writes it per
  vertex and the shader multiplies it over the sample. Water takes Minecraft's blue
  `0x3F76E4` at a blended alpha, oak leaves its green `0x77AB2F`; untinted blocks
  send opaque white, which leaves the texture alone. `shaders/ui.wgsl` applies the same
  tint to the hotbar icons (minus the lighting), so a grey texture reads the same in the
  HUD as it does in the world — do not forget it when adding a tinted block.
* **Cutout blocks** (oak leaves, flowers): texels below the cutoff are `discard`ed, so
  they vanish in the opaque pass with no blending — and because that pass writes depth, a
  canopy occludes what is behind it. Nothing is needed in code beyond *not* reporting the
  block from `Block::is_blended()`. Note the cutoff is **0.1**, not 0.5: textures are
  sampled *mipmapped* (see below), and with a high cutoff a shrinking leaf would nibble
  away until a distant canopy dissolved into holes. Minecraft's leaves are exactly this,
  its `cutout_mipped` render type.
* **Blended blocks** (water, glass): report them from `Block::is_blended()`, and the
  mesher puts their faces in the **translucent** set instead of the opaque one. The
  renderer draws that set after the opaque geometry, farthest chunk first, with alpha
  blending and depth writes off — see **Transparency** below. This is Minecraft's
  *translucent* render type: there is no alpha test, so a texture's own alpha blends.
* **Cross sprites** (flowers): report them from `Block::is_cross()`, and the mesher
  draws `gfx::mesh::CROSS_PLANES` — two quads crossed through the middle of the cell —
  instead of a cube, lit by the light of their *own* cell. They are exempt from face
  culling entirely, because their planes lie flush against nothing.
* **Animated blocks**: call `TextureArray::update_layer(queue, layer, image)` each
  frame (or every few frames) for just that layer — far cheaper than an atlas.

Current limits / next steps:

* Blended geometry is not sorted back-to-front *within* a chunk, only between chunks
  (fine for one flat sea or a plain pane; it can mis-order water against glass in the
  same chunk).
* The transparent pipeline needs at least one translucent block in the scene to
  actually draw a second pass (water supplies that).

### Lighting

Light lives in the chunk, **one `u8` per cell per channel** (`Chunk::sky_light` and
`Chunk::block_light`), and is recomputed together by `Chunk::relight` whenever a chunk is
generated or a block is edited. Both channels follow Minecraft's rules, at **levels 0–15**:

* **Sunlight** falls straight down at full strength, so every cell with open sky
  above it is **15**.
* From there light spreads one block at a time in all six directions, **losing a
  level each step**, and solid blocks stop it dead.
* Two blocks soften those rules (`Block::lets_light_through`, `Block::blocks_sky`).
  **Water and leaves** let light spread *into* them, losing a level per block — which
  is why a canopy dapples the ground under it. **Glass** is transparent to the sun
  outright: it does not even stop the vertical sunlight, so a glass roof is as bright
  as open sky.
* So the world is bright in the open and fades to black under overhangs and deep
  inside caves. (Light does not cross chunk borders; sunlight is vertical, so all
  that is lost is the sideways bleed where a shadow straddles a border.)
* The second channel, **block light**, is that same spread started from the blocks that
  *emit* light instead of from the sky — see **Emissive blocks & block light** below.

The mesher bakes the light **per corner**, not per face, and that is the whole of **smooth
lighting**. For each of a face's four corners it averages the *brightness* — the light curve's
output, not the level — of the four cells that meet around it: the open cell the face looks
into, the two beside it, and the one diagonally between them. Averaging levels would step a
whole level at a time; averaging brightness is what gives the gradient that softens a shadow
instead of banding it. On top of that sits **ambient occlusion**: a solid cell in those three
shades the corner, and two beside each other — a wall meeting a wall — shade it hardest
(`AO_SHADE`, 0.55–1.0), which is the soft dark crease along the bottom of a wall. A solid cell
contributes the *front* cell's brightness rather than its own, so a wall beside sunlit ground
darkens the crease without dimming the whole face.

Both halves live in `world::mesher::corner_light`, and the light curve lives in Rust
(`world::light::brightness`) rather than in a shader for exactly that reason: the average has to
be taken after the curve. The two channels are averaged by the *same* arithmetic —
`corner_light` carries a `Brightness` pair and scales both by the one ambient-occlusion factor,
because a crease in a lava-lit room is a crease in whatever light is shining — and are parted
again only when the vertex is packed. The mesher packs each corner's **two** brightness bytes
and the face index into one `u32` (`mesh::pack_light`: sky in bits 0–7, block in 8–15, face in
16–18), the geometry pass writes the *interpolated* sky brightness into the albedo's alpha and
the block brightness into the normal's alpha, and `lighting.wgsl` then has only a multiply left:

1. the interpolated brightness — the *ambient* half of the light;
2. times a **per-face shade** — **top 1.0, north/south 0.8, east/west 0.6, bottom 0.5**. That is
   the trick that makes a block's sides read as solid: without it every face of a flat-coloured
   cube looks identical;
3. with the split between **sunlight and shade** applied by the shadow map (see **Shadows**),
   which decides how much *direct* sun reaches the pixel.

Worth being explicit about what that preserves: where the light is uniform — flat ground in the
open, every cell at 15 — all four corners come out at full brightness, the very value the mesher
used to pack for the whole face. Smooth lighting is therefore a change to *shaded* faces rather
than a rewrite of the picture, and a test pins it. Water gets the same treatment through the
blended pass, so a shore dims into the sea and a shadow on the bed shades the surface above it.

The multiply happens in linear space (textures and the render target are sRGB), so
the shader raises the factor to `2.2` to land the shading on the gamma-encoded
texture the way Minecraft does, and floors it so unlit caves are very dark grey
rather than pure black.

The G-buffer remembers the face *index*, but `block.wgsl` also writes the face's real
normal, as `n * 0.5 + 0.5` — because the lighting pass has to push a pixel along its own
normal before asking the shadow map. A voxel world only has six of them, and the pass
recovers the index from the largest component (`face_of_normal`), so the two always agree.
The normal's alpha was the one truly spare channel in the G-buffer, and is where the block
brightness now rides — which is a small piece of the deferred pipeline earning its keep: a
second light costs one more byte carried through the buffers, not another pass.

A flower is lit flat by its own cell, having no corners that meet anything; and light does not
cross chunk borders, so a smoothed gradient stops at a chunk seam where a shadow straddles it.

**Full bright** — the `N` key, held down — is the one thing that skips all of that. While it is
held, every cell is lit as if it held **level 15**: a cave, an overhang's underside and open
ground all come out at the brightness the mesher would have baked for a cell in the sun. It is a
*view* option and nothing more — it changes no block, no chunk and no baked light, only the number
the picture is drawn from — which is why it lives in the per-frame uniform (`Globals.full_bright`)
rather than in the mesher or the vertex data: the light is still in the vertices, and the lighting
pass simply declines to read it.

Two things go with it, and both are deliberate:

* **The sun's shadow goes too.** A shadow is the one thing left that could darken a fully-lit
  cell, and the mode exists to *read* a cave, so the lighting pass skips the shadow lookup
  entirely — which is also why it can return early, without even reconstructing a world position
  from the depth (that exists only to ask the shadow map).
* **The face shade stays.** Top 1.0, sides 0.6–0.8, bottom 0.5 is what makes a cube read as a cube
  rather than a flat sheet of its own texture, and level 15 does not take it away: the pass is
  handed `shade` twice, once per channel, instead of the baked brightness the corners carry.

Water is lit by the *blend* pass rather than this one, so it has to ask for the same thing
separately (`fs_blend` in `block.wgsl`) — otherwise the sea in a cave would be the one dark thing
left in a full-bright picture. The HUD needs nothing: it was never lit.

### Emissive blocks & block light

Some blocks *make* light rather than merely letting it through, and that light has to be kept
apart from the sky's — because the two are not equivalent further down the pipeline.
`Block::light()` is the hook: **lava returns 15**, everything else 0.
`world::light::block_light` seeds a flood fill at every such cell and spreads it exactly as the
sun spreads, one level per block and stopped dead by anything opaque, so a pool is a bright
source with a falloff rather than a flat glow. The result lives in `Chunk::block_light`, and
`Chunk::relight` recomputes both channels, so placing a lava block relights its chunk the way
digging a hole already did.

Why a second channel earns its keep:

* **A shadow may not take block light away.** The sun's shadow map says whether the *sun*
  reaches a point; it has nothing to say about a torch, and neither does the baked sky light —
  which is why lava's 15 fed into the sky channel alone would have rendered at about 15% of full
  brightness (`lit(1.0, 0.0)` — the ambient half alone, with no direct sun), as if it were
  sunlight in the shade. The lighting pass combines the two as

      light = max(lit(sky * shade, shadow), lit(block * shade, 1.0))

  so either one alone can light a surface: out in the open the sky term is what it always was
  and the block term is nothing, while in a cave beside a lava pool it is the other way round.
  The block term is deliberately lit *as if the sun reached it* (`shadow = 1.0`) — that is the
  "unshadowed" half of the equation — and the `max` is what stops the two stacking into
  something brighter than either.
* **No emitters means no change.** With `block` zero everywhere, `lit(0, 1.0)` is the light
  curve's own floor — the same 0.004 `lit` already clamps to — so the `max` returns exactly what
  `lit` on the sky alone returned before there was a second channel, and the mesher writes a
  block byte of 0 for every vertex. Two tests pin that property: a roofed room with no source in
  it bakes no block brightness at all, and a chunk with nothing emitting reads its sky light
  unchanged. Adding the channel to a world with no lava in it is therefore a change to the
  picture only where a lava block stands.
* **It is the general hook, not a lava special case.** A torch is the same `Block::light()` with a
  smaller number and a different texture; nothing else in the pipeline would change.

Two things to be careful of when adding a block like this. *Emitting* and *letting light through*
are different questions: lava is **opaque** (`is_opaque`), so neither channel passes through it —
light pours *out* of it instead. And a fluid is not
automatically a source: water lets block light through while emitting none of its own, and lava
emits while letting none through. Being opaque to light is not the same as being opaque *to face
culling*, either: lava does not fill its cell, so it hides only faces of its own kind (see
**Transparency**).

Known limits: lava is the only emitter (the whole palette is in the creative screen's, and the
world's only block light is the lava sea at the bottom of it — see **Caves & ravines**), and none of
it flows; block light does not cross chunk borders, so a pool at a chunk seam lights only as far as
the seam; and the light curve is the same one the sky uses, which makes a lava-lit room dimmer than
Minecraft's, whose falloff is gentler.

### Shadows

The sun is a **shadow map**: a depth buffer of what the sun can see (see `gfx::shadow`),
drawn once per frame when it needs drawing, and sampled by the lighting pass. It costs one
extra draw of geometry the renderer already has — a chunk's mesh *is* the world's outer
shell, because the mesher culls only the faces between two solid blocks, so nothing new has
to be built for shadows at all.

* `Sun` is a fixed direction (`TILT` — high, and to the east) plus an orthographic box wide
  enough to cover the whole streamed region: `RENDER_RADIUS + 1` chunks each way, which works
  out at about **11 cm per texel** over a 2048² map. Fine enough that a tree casts a
  tree-shaped shadow, coarse enough to redraw every frame.
* The box follows the player, but its **texel grid is held still**: the matrix only changes
  when the player crosses a whole texel along the light's own axes (`shadow::snap`). Without
  that, every shadow edge would crawl a texel per frame as you walked. It is rebuilt from the
  lattice *indices* rather than nudged from the camera, so one cell always yields one matrix,
  bit for bit — otherwise the renderer could never tell that nothing had moved.
* **The pass can be skipped.** `render` redraws the map only when the box moved a texel or a
  chunk was remeshed, which saves a whole pass on most frames while you stand still.
  `shadow_stale` also *starts* true: an untouched depth texture reads as 0, which would
  shadow the entire world.
* A pixel is pushed a fraction of a texel along its **own normal** before the lookup. That is
  the defence against a surface shadowing itself at a grazing angle, and the reason the
  G-buffer stores a real normal. A small depth bias on the pipeline catches the steepest
  angles.
* A **3×3 spread of `textureSampleCompareLevel` taps** softens the edge. The sampler is a
  *comparison* sampler with `Linear` filtering, so the depth test happens inside the sampler
  and each tap averages four stored depths: percentage-closer filtering out of nine lookups.
  (`...Level` rather than plain `textureSampleCompare`, because the call sits inside a branch
  and a sampling call that is free to choose its own mip level may not.)
* **Outside the box counts as lit.** The map knows nothing there, and its clamped edge texel
  would otherwise cast a shadow that is not there.

The light then splits in two, with `AMBIENT + DIRECT = 1.0`:

    light = curve(voxel) * face_shade * (AMBIENT + DIRECT * shadow)

That is deliberately arranged so that in the open, where `shadow` is 1.0, lit ground comes out
**exactly as bright as it was before there was a sun at all** — and a shadow takes it down to
`AMBIENT` (0.42). Adding shadows is then a change to the picture rather than a rewrite of it,
which is also why water, which has no shadow map bound in the blend pass, can go on lighting
itself with `shadow = 1.0` — and why the HUD, which is not lit at all, still shades its icons
with the plain `lit(shade, 1.0)` it always used.

Two things about that equation are worth being explicit about:

* **The sun's direction does not affect how bright a face is** — there is no `dot(N, L)`
  anywhere. Today the sun acts *only* through its shadow: a wall facing away from it is as
  bright as one facing toward it, until something blocks the light. Adding `dot(N, L)` means
  putting it in the *direct* term alone and *replacing* `face_shade` there, never stacking the
  two.
* **The baked brightness is the ambient term**, so a canopy that darkens the ground beneath it
  still does, and the shadow map only takes away the direct sun on top of that. That is why a
  cave stays dark even with the sun's contribution switched off.
* **Block light is not sunlight, so the map has nothing to say about it.** A lava pool's light
  is combined with the sky's *after* the shadow lookup, on the `shadow = 1.0` side of the
  equation, so a shadow across a cave floor darkens the daylight that is not there and leaves
  the glow alone (see **Emissive blocks & block light**).

Known limits:

* One map and no cascades, so the ~11 cm a texel is spent on the whole render distance at once.
* A canopy casts a **solid** shadow rather than a dappled one — the shadow pass has no fragment
  shader at all, so it cannot alpha-test leaves. Giving it the block textures and a `discard`
  on alpha, exactly as `fs_cutout` does, is the fix.
* **Water receives no shadow** (and neither does the HUD): both are drawn in the blend pass,
  which has no shadow map bound. Binding the sun's group there and calling the same PCF loop
  would do it.
* A flower casts a small solid shadow rather than a cross, and terrain further off than the box
  casts none — which at this draw distance you cannot see.

### Transparency

**Water and glass blend** — Minecraft's *translucent* render type — so they cannot be
drawn in the opaque pass. Each chunk's mesh is therefore split in two
(`gfx::mesh::MeshData`): an **opaque** set (depth-writing, unblended) and a
**translucent** set (blended, depth writes off). They are separate vertex *and* index
buffers, so each pass binds exactly what it needs with no index offsets to reason about,
and a chunk with no water or glass allocates nothing for the blended pass. **Lava is the
counter-example that keeps the split honest**: it is a fluid — shaped like water, sitting a
little low in its cell — but it is *opaque*, so it belongs to the opaque pass like stone.
Report it from `Block::is_blended()` and nowhere else in the renderer changes.

The two kinds of see-through block are separated by `Block::is_blended`, and
`shaders/block.wgsl` has a fragment entry point for each: the **opaque** pass *cuts out*
texels below half opacity (`fs_cutout`), which is what leaves and flowers want, while the
**blended** pass deliberately keeps them (`fs_blend`) so a texture can carry real alpha —
a faintly tinted pane stays faint rather than vanishing, just like Minecraft's translucent
pass.

The renderer draws them in the only order that is correct — passes 2 to 4 of the frame:

1. **Opaque geometry**, nearest chunk first, depth writes on. Front-to-back is an
   optimisation: the depth buffer then rejects hidden fragments as early as it can.
2. **The lighting pass** over the whole G-buffer (see **Rendering**), which is what actually
   leaves a finished picture on the screen for the last pass to blend over.
3. **Translucent geometry**, farthest chunk first, blended, testing depth but not
   writing it. Depth writes must stay off — a translucent surface must not hide
   what is blended behind it — which is exactly why the order has to be back to
   front: nearer water blends *over* the water behind it. The depth it tests is the scene's
   own, out of the G-buffer: loaded and never written, and discarded when the pass ends.

`Renderer::order_chunks` sorts the loaded chunks by distance once per frame into a
reused `Vec` (no per-frame allocation); the opaque pass walks it forwards and the
blended pass walks it backwards, so one sort serves both.

Face culling follows the same split (`Block::hides_face_of`): an opaque neighbour
hides a face, a see-through one hides only faces of its own kind. So the sea has no
faces between its own cells, while the seabed *is* drawn — and seen — through the
water; a wall of glass has no seam through it either. Leaves hide nothing at all: a
leaf face is kept even against another leaf, because culling those would let you see
straight through the canopy. **The fluids hide only their own kind** — a pool has no faces
inside it — and the *rock* beside a pool, and the rock over it, keep their faces rather than
being culled by it. That last rule is what keeps a pool's waterline honest: see below.

Cross sprites are exempt outright: `hides_face_of` only hides a face when the block
being drawn *fills its cell* (`Block::is_full_cube`), and a flower's planes lie flush
against nothing — so nothing may erase one, not even the wall it grows against.

Air **and the fluids** are see-through to gameplay as well as to the eye: `World::raycast`
stops at the first block a ray can *hit* (`Block::is_targetable` — everything but air
and the fluids), so looking at the sea targets the ground beneath it, looking at a lava pool
targets the rock behind it, while glass and leaves are aimed at directly. A placed block
meanwhile has to land in `Block::is_replaceable()` space — air, or a fluid you are building
over — so you can build into the sea, and bury a pool of lava the same way.

Water and lava are both drawn a **fraction of a block short**: their surface reaches 14/16 of
the way up their cell (`Block::surface_height`, one constant for both), so a shoreline shows a
step *down* into the water — and a lava pool a step down into the lava. `mesher::top_height`
applies that only where the fluid is exposed — buried under more fluid it keeps the full
height, so columns of sea
have no seam in them — and `push_face` shortens only a face's *upper* edge, so water's
floor still rests on the block below. A face's texture, light, tint and height travel
together as a `FaceStyle`.

That shortened surface is also why a **fluid hides nothing but its own kind** (see the face
culling above). The slice above the surface is empty — nothing is drawn there — so a face a
fluid had culled would leave a two-pixel slit along the top of the pool with no face across it
at all, and the neighbour's *missing* face would show through it: the rock at the waterline, or
the rock a pool sits under, would read as transparent. A fluid therefore hides only its own
kind, which stops short in exactly the same way and so still lies flush against it, and every
other face around a pool stands. Lava alone used to be the exception — opaque, and so culled
like stone — and that is precisely the case that showed the seam. The price is triangles rather
than pixels: the rock around a lava sea keeps faces that a pool's own geometry then hides, about
**+12%** of the patch's faces measured over six chunks, all of them occluded.

### The water surface

Water is the one surface in the world that is not flat, and the one thing the renderer cannot
fake with geometry: a sea is one quad per block, a hair short of its cell, so *every* ripple has
to be made up in the shading. `shaders/water.wgsl` (pasted into `block.wgsl`, since `fs_blend` is
where water is drawn) does exactly that — it builds a normal for the surface, and the light
sends it back as a glint.

The normal is the facet's own, tilted by two slopes added together:

* **The water height map**, which is the water texture itself. It is greyscale and takes its
  colour from the block's tint, so its brightness is a height and a central difference across a
  texel is the slope of the surface — a bump map, in other words, sampled at mip level 0 because
  the finest ripples are the point, and wrapped by the sampler's `Repeat` rather than flattening
  at the tile's edge. **The ripples you can see drawn on the water are therefore the ripples that
  catch the light.** The strength is calibrated to that texture, and to a measurement: its slopes
  are gentle (a texel differs from its neighbour by 0.04 of the range on average), so
  `BUMP_STRENGTH` is 8 — at 2 the tilt would be under five degrees and invisible.
* **fBm**, four octaves of value noise over a hash, sampled in *world* space and scrolled with
  time. World space because the pattern should belong to the world rather than to each block —
  otherwise the sea would visibly repeat every block — and scrolled because water is never still.
  The hash works on the *bits* of the coordinates, not on their values, so the pattern is the same
  on every GPU: a `sin`-based one is only as good as the driver's sine, and a ripple pattern that
  differed between drivers would be a bug nobody could chase.

Both fade out with distance (`DETAIL_FADE`), which is not only taste: a 16-texel height map spread
over a whole sea aliases into shimmer, and what a sea looks like from far off is a sheet of
reflected sky anyway.

The tilted normal then earns its keep twice, and neither is a shadow — there is no shadow map in
this pass:

* a **highlight** where the sun's reflection lines up with the eye (a tight `pow(dot(N, H), 48)`,
  so it lands as sparkles where a ripple happens to catch the sun rather than as a sheen over
  everything), scaled by the *sky* half of the light, because a pool lit by lava in a cave should
  not catch a sunbeam;
* a **sheen** of sky at grazing angles — `SHEEN_COLOR` is the colour the renderer clears to, and
  this is the part that makes a sea seen from a low angle read as sky rather than as water.

That needed three things in the uniform the passes share (`Globals`): `sun_direction`
(`shadow::direction`, the same vector the shadow box is built from), `eye_position`, and `time`.
The same uniform carries the `full_bright` switch the `N` key sets — every pass that lights
something reads it (see **Lighting**).
It is also why binding 0 is `VERTEX_FRAGMENT` rather than vertex-only, and why the layout is
pinned by a test — a uniform whose fields moved on one side would not fail to compile, it would
quietly light the water with the wrong numbers. Worth being explicit about the scope: **this is
the only `dot(N, L)` in the renderer.** The rest of the world is lit without asking which way the
sun faces (see **Shadows**), and the water does not change that.

The other half of the water's look is which blocks ripple. Water and glass share the blended pass,
and a pane is flat, so the surface has to say which it is: the mesher marks a water face with
[`mesh::WAVY`] — bit 19 of the packed light word, beside the two brightness bytes and the face
index — and `fs_blend` ripples only what carries it. One bit, and `Block::ripples()` is the one
place that decides who gets it.

Known limits: the water's side faces ripple too (they are the waterline, and it reads as a lip of
water rather than as a mistake); the highlight cannot be shadowed, for the same reason everything
else in this pass cannot (the map is not bound here); and the ripples are shading only — the
surface itself never moves.

Still to come: an underwater fog/overlay (`water_overlay.png`), the water texture *animating*
between the frames of its strip (see **Textures & blocks**), and sorting the
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
blocks of dirt, then grass on top, and above the grass the odd **oak** — trunk and
canopy alike (see **Terrain**) — and then cuts the **caves and ravines** and floods the
bottom of the world with lava (see **Caves & ravines**).
`World` keeps a `HashMap<ChunkPos, Chunk>` of
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
density noise. Sampling is the hot spot — the terrain pass is ~18 ms of a chunk and the carvers
add ~7 ms to that in a debug build (see **Caves & ravines**) — which is
why generation runs on worker threads rather than in the frame loop (see **Chunk
streaming**).

Everything is a pure function of `(seed, x, y, z)`, so chunks agree at their borders
and the world is reproducible. The seed comes from `RUSTCRAFT_SEED`, or `DEFAULT_SEED`
if unset. The camera spawns standing on the surface.

### Caves & ravines

The density field gives the *shape* of the land, but it is a smooth sheet: air pocketed
under the surface by the noise's vertical variation is where a cave can happen, not a cave.
The caves and ravines are cut into the rock afterwards, by **carvers** — Minecraft Beta's
approach, and deliberately not another threshold on the density field. Rather than giving every
cell a chance of being hollow, a handful of deterministic **worms** walk through the rock the
terrain pass has already laid down, and each step of the walk cuts an ellipsoid out of it:

* **Caves** (`world::carve`, one origin in `CAVE_GATE` (15)) — winding tunnels. The heading
  drifts a little at every step and the radius swells in the middle and tapers at both ends, so a
  tunnel has a mouth rather than a wall. Part way along, a fat worm forks into two thinner ones at
  right angles, and *that* is what makes a cave a **network** rather than a corridor; a start in
  four is a **room** instead, a worm that barely steers and so mills about in one place, hollowing
  a chamber. Threading, not hollowing: measured, caves cut about 7% of the underground.
* **Ravines** (one origin in `RAVINE_GATE` (50)) — rare, long and nearly straight, with a heading
  close to horizontal but a carve stretched **vertically** (`RAVINE_STRETCH`, 2). That one number
  is the whole difference between the two: the same worm flattened makes a tunnel, stretched makes
  a gash. The two legs it forks into at the end open it into a `Y`, and because it cuts *down* as
  it runs (`RAVINE_DESCENT`) a deep one reaches the lava at the bottom of the world.

`Terrain::generate_chunk` runs them in Minecraft's own order — columns, then carvers, then the
lava sea, then canopies — and `LAVA_LEVEL` (**11**) is the last of those: every cell still open at
or below it becomes `Block::Lava`. A pass of its own, so that neither carver has to know lava
exists (they cut *emptiness*), and so that a cave which reaches the bottom of the world ends in a
pool rather than a pit. That is where the world's block light comes from: the lava sea is lit at
15 and `Chunk::relight` spreads it up the shaft (see **Emissive blocks & block light**).

What a carver may cut is short by design — `Block::is_carveable` is **stone and soil and nothing
else**. Bedrock keeps the floor of the world solid however deep a worm goes — *carving* may not cut
it, though the player may still break it (see **Interaction**); a cave opening into
the sea does not drain it (nothing flows), and lava is a source rather than something to tunnel
through; and wood is not rock, so a ravine cuts *around* an oak and a tree whose ground a cave
opens keeps standing. (Trees are planted with the columns, *before* the carvers, precisely so that
a canopy always has its trunk: planting them afterwards would mean a chunk deciding whether to
draw a neighbour's canopy without being able to see whether that neighbour's ground was carved.)

**Why carving is seam-free** is the whole design, since chunks are generated by different threads
at different times and a cave that disagreed about the cells at a border would leave a wall
standing in the middle of a tunnel. Two rules make it impossible:

* **A worm's dice come from its own origin and nothing else.** The origin chunk's *absolute*
  coordinates seed a `JavaRandom` stream (see **`world::random`**), so the worm an origin grows is
  the same worm whichever chunk asks for it. (The one place we deliberately part company with the
  snippet this was ported from, which mixes in the offset from the chunk being carved — fine for
  Minecraft, where a chunk is carved in one pass, but not for a streaming world.)
* **A worm's reach is bounded by `RANGE` (8 chunks).** A cave gives up once it is further from its
  origin than the steps it has left could carry it — Minecraft's own test, and what makes eight
  the right number — and a ravine is shorter than that outright. So a chunk finds every worm that
  touches it by asking only the origins within `RANGE` of it (17 × 17 of them, Minecraft's
  `MapGenBase`), and nothing outside that box can print so much as one cell inside it.
  `caves_and_ravines_stay_within_reach` pins both halves of that, and
  `a_worm_carves_the_same_cells_from_either_side_of_a_border` pins the seam itself.

The cost is real and measured: a chunk's generation went from **18.7 ms to 25.8 ms** in a debug
build (worst chunk 38 ms), because a chunk is affected by a *neighbourhood* of origins rather than
one — the walk is what costs, not the cell-by-cell cut (the ellipsoid test is scaled rather than
divided, and the box is clipped to the chunk before a cell is looked at, and neither was what the
time was going to). It is spent on four worker threads, off the frame, so what a player sees is
unchanged: the spawn patch still opens in under a second and streaming runs at ~166 fps. The lever
if it ever needs to be cheaper is `[profile.dev] opt-level = 1` (a debug build leaves even the
noise unoptimised), not fewer caves.

Known limits: carvers and nothing else — no ore veins, no dungeons, no mineshafts; nothing flows,
so a ravine that cuts the seabed leaves a dry hole under the sea; and the world is reproducible per
machine but **not across platforms** (`sin`/`cos` are only exact to the platform's libm), so the
persistence milestone must store blocks rather than re-derive them.

**Oaks** sit on top: a grass column open to the sky rolls a hash of `(seed, x, z)`,
and 5 columns in 1000 come up a tree — a **straight trunk of 4–6 `OakLog`** (vanilla
oak's `StraightTrunkPlacer(4, 2, 0)`) planted from the grass straight up, wearing
Minecraft's **oak leaf arrangement** (the one birch uses): a `+` of five one block above
the log, the log's four sides and 1–3 diagonals around it, then two `5 × 5` rows with
the corners cut, the corners themselves filled only now and then. Being a pure function
of the seed, a tree never flickers as chunks stream back in. A trunk is one block wide,
so it never crosses a chunk border — but leaves reach two blocks out, so a chunk also
looks two blocks *outside* itself for trunks whose canopy spills in.

### Water

`SEA_LEVEL` is **62**: once a column's ground is laid down, any open space left
between it and the sea is filled with `Block::Water`, and the surface block is
**grass above the waterline but bare dirt below it** — a drowned lawn would look
wrong. Only the space *above* a column's surface floods, so the noise's caves stay
dry. The camera stands on the sea when its spawn column is under it.

**Lava is generated as the lava sea** — every cell still open at or below `LAVA_LEVEL` (11) — so
digging deep enough (or finding a ravine that reaches the bottom of the world) ends in a pool, and
the block light that comes with it is the only light down there (see **Caves & ravines** and
**Emissive blocks & block light**). It is not *placed* as veins or lakes in the terrain, and none
of it flows: a lava pool is a floor to walk over, not a trap that spreads.

Note the numbers: `BASE_HEIGHT` is 64 and `SEA_LEVEL` is 62, so a little under half
the world ends up as ocean; `LAVA_LEVEL` is 11, which is deliberately low — a cave has to go deep
before it glows, so the glow is something you find rather than something under your feet. All three
are single constants to taste.

### Chunk streaming

`World::update` is called every frame and has two jobs. It **collects** whatever the generator
threads have finished — a non-blocking drain of the pool's channel — and, only when the player has
crossed a chunk boundary, it re-centres the patch: out-of-range chunks are dropped, work still
queued for them is cancelled, and the missing positions (`missing_chunks`) are asked for **nearest
first**, so the world fills inwards from the player. A chunk that arrives after the player has
moved on is thrown away rather than filed, so nothing has to be un-done afterwards.

Generation itself waits on nothing in the frame: both halves of a job (`Terrain::generate_chunk`
and `Chunk::relight`) are *pure* functions of the position and the seed, so they run on
`world/streaming.rs`: `available_parallelism() - 1` worker threads, capped at 4, popping positions
off a shared queue behind a `Mutex`/`Condvar` and sending finished chunks back down an `mpsc`
channel. No new dependency for it, and no thread outlives the world — `Drop` sets the stop flag,
wakes the idle workers and joins them. The world keeps its own book of what is **in flight**
(`pending`), which is both what stops a position being asked for twice and what
`World::flush_around` waits on at startup: the three-by-three around the camera, so it can be stood
on real ground in under a second, with the remaining ~160 chunks streaming in over the first
frames. Waiting for the whole patch before the first frame is exactly the three-second freeze this
replaced.

`World::block()` samples any world position so the mesher culls across chunk borders, and the
mesher bakes each chunk's world origin into its vertex positions, so all chunks share one
view-projection uniform (no per-chunk model matrix).

Meshing stayed on the render thread, so it is **budgeted by time** instead: `update_chunks`
(re)meshes dirty chunks nearest-first until `MESH_BUDGET` (3 ms) is spent, always doing **at least
one** so a backlog cannot stall. The loop asks the world for one chunk at a time — it can only know
what the next chunk would cost by meshing it — and that per-chunk sort is microseconds against the
milliseconds a chunk takes, which in a debug build is *more than the budget*: 10–16 ms measured,
smooth lighting having roughly tripled the cost of a mesh since the first cut. So a stream burst
meshes exactly one chunk per frame and every frame of it pays that chunk's cost — ~25 ms frames
instead of the ~220 ms freezes of generating thirteen chunks in one — and a release build, where a
chunk meshes in a fraction of that, fits several into the budget.

Two things keep the picture honest while that happens:

* **A chunk is meshed only once its in-range neighbours have arrived** (`can_mesh`, which
  `take_dirty` filters on). A face between two chunks is culled, so meshing against a neighbour
  that is still generating would draw a wall where its blocks are about to be; waiting a frame is
  cheaper than drawing a seam that then has to be meshed away. A neighbour *outside*
  `RENDER_RADIUS` is never generated at all, so it is never waited for — the patch's edge is meant
  to show its faces, exactly as it always did.
* **Nothing gameplay-facing changed.** A chunk that has not arrived reads as air and full daylight,
  which is what an unloaded chunk already read as, so `block`, `light`, `raycast` and `set_block`
  needed no changes at all, and an edit still re-meshes its chunk on the next frame.

Frame timing has instrumentation of its own: `rustcraft=debug` logs how long a frame's meshing
took, how many chunks the threads have generated and what one cost, and **warns about any frame over
33 ms** with how much of it was meshing. That is what turned "chunk loading feels janky" into
numbers, and where a regression will show up first. What is left of the jank is one chunk's mesh
cost, so the next steps are `[profile.dev] opt-level = 1` (a debug build leaves even the noise
unoptimised) and then meshing on a worker thread as well.

Still to come: **greedy meshing** (merging coplanar quads — a flat grass field would
collapse to a handful of quads), and meshing off the render thread.

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

* Tests live beside the code they cover (`mod tests`), and there are ~145 of them — mostly pure
  logic (world gen, the carvers, meshing, lighting, raycasting, chunk maths, the HUD's layout and
  hit tests). Add them alongside new pure logic; the streaming decisions are pure functions
  (`missing_chunks`, `can_mesh`) precisely so they can be tested without threads, and the carvers
  are pure functions of `(seed, origin)` for the same reason — which is what lets a test carve the
  same worm from either side of a chunk border and compare the cells.
* **Shaders are tested too.** WGSL is a string as far as `rustc` is concerned, so a typo in one is
  otherwise only found when the game opens a window: `renderer::tests` hands every composed shader
  to `naga` — wgpu's own front end, already in the tree as its dependency — to be parsed and
  validated, and pins the `Globals` layout the CPU and GPU halves have to agree on. A new shader or
  a new uniform field belongs in those two tests.
* `RUST_LOG` controls verbosity (e.g. `RUST_LOG=rustcraft=debug,wgpu=warn`).
  The app's own messages use the `rustcraft` target. `rustcraft=debug` also prints the chunk
  streaming numbers — mesh time per frame, chunks generated, and any frame over 33 ms.

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
| `N` (held) | **Full bright**: light every cell as if it held level 15, so caves and overhangs are as visible as open ground |
| `Esc` | Open/close the pause menu (Continue / Exit) |
| Left click | Use what is in hand: place a block, or pour/fill a bucket (or grab the cursor when it is free) |
| Right click | Break the block the camera is looking at |
| Scroll wheel | Cycle the nine inventory slots |
| Middle click (the wheel pressed) | **Pick block**: what is being looked at becomes the thing in hand — a fluid being picked as its bucket |
| `1`–`9` | Choose a slot — or, on the creative screen, stock that slot with the block under the cursor |
| `E` | Open/close the creative inventory (and hand the cursor over with it) |

Held keys are tracked in a set and cleared when the window loses focus; mouse
look uses raw `DeviceEvent::MouseMotion` so it keeps working while the cursor is
grabbed; auto-repeat is ignored for double-tap detection. The cursor is grabbed
on startup — `Esc` opens the pause menu and so frees it, and *Continue* (or `Esc`
again) grabs it back.

**Block editing** raycasts from the camera through the voxel grid
(`World::raycast`, an Amanatides–Woo DDA) up to `REACH` (6) blocks, returning the
block hit *and* the face it was entered through — the first block the ray can actually
*hit*, the fluids being transparent to it. **Right-click breaks** it; **left-click uses what
is in hand** — see **Buckets** below; **the wheel button picks what is being looked at**.
Edits go through `World::set_block`, which marks the chunk — and any touched border neighbour —
dirty for re-meshing. **Bedrock can be broken like anything else** — there is no survival mode to
be kept out of the bottom of the world with, so the floor is the player's to take apart (breaking
and *carving* are different questions: the carvers still leave it alone, see **Caves & ravines**) —
while neither fluid can be broken: you build over water, and you bury lava — which is also the one
edit that turns light *on* rather than off, since `set_block` relights the chunk (see
**Emissive blocks & block light**).

### Buckets

A slot does not hold a [`Block`] — it holds an **`Item`** (`src/item.rs`), which is either a
block, drawn as a little cube and placed as a block, or one of the three buckets, drawn as a
flat sprite and *used* on the world. Buckets are the reason that distinction exists: **water and
lava are not blocks you can hold.** They are not on the creative palette; they live in the two
buckets, and a bucket is the only thing that can put one into the world or take one out of it.

- **A full bucket** pours. Left-click, and the fluid lands in the cell the ray enters — the same
  cell a block would be placed in — and the slot is left holding the **empty bucket** it came
  from. One pour per bucket.
- **An empty bucket** fills. Left-click at water or lava and the fluid is taken *out of the
  world*, with the full bucket left in hand. It is the exact inverse of pouring, and
  `Item::pours`/`Item::bucket_of` are the two halves of the one mapping, so they cannot drift.
- **A bucket is not picky about which fluid**, and nothing flows here: every fluid cell is a
  *source*, so there is no half-empty water to refuse. That is why filling needs no test beyond
  "is it a fluid".

Filling needed the second raycast. `World::raycast` looks *through* fluids — that is what makes
right-click break the ground under the sea rather than the sea — so a bucket would never find the
water it was held over. `World::raycast_anything` is the same DDA with a wider stop: it takes the
**nearest thing of either kind**, a block or a fluid, which is exactly what a bucket should act
on. It also means nothing can be scooped through a wall, and **pick block** uses it too: pick a
fluid and you get its bucket, the way Minecraft's pick does, because what the crosshair is on is
the water and not the sand beneath it.

**Inventory**: `src/inventory.rs` holds nine quick-access slots and the creative
screen. Slots are chosen with `1`–`9` or the wheel; `E` opens the screen, which shows
everything you can hold (grass, dirt, stone, oak log, oak planks, oak leaves, poppy,
dandelion, glass, the three buckets, bedrock). Clicking a palette entry picks it up, clicking a
slot drops it in, and hovering one and pressing `1`–`9` stocks that slot directly —
the way Minecraft does it. **`Inventory::pick_item`** is the other way in: a slot that already
holds the item being looked at is *selected*, and otherwise the item replaces whatever the
selected slot held — so the row is a set of things you have, reaching for one of them does not
disturb the others, and what is in hand is always what you were just looking at. Only buckets
are consumed, and they come straight back as their other half, so this is otherwise a creative
inventory. The screen takes the cursor while it is open
(`App` grabs and releases it with the toggle), and the world ignores mouse-look, clicks and the
wheel until it closes.

The HUD is drawn by the renderer as screen-space geometry (`shaders/ui.wgsl` — a 2D
pipeline that reuses the block texture array and bind group). A slot's **block** is drawn
by `gfx::mesh::BlockIcon`: a fixed 45°-yaw, 30°-tilt axonometric projection of the unit
cube, which hands the top and its two visible sides to the *existing* `CUBE_FACES`
corners — so each face keeps its real texture (the log shows rings on top and bark on
the sides) and its real directional shading, because `ui.wgsl` reads the face index the
mesher packs into the light for exactly this. Cross sprites (poppy, dandelion) are
drawn as the flat sprites they are — and so is a **bucket**, which is an `Item` rather than a
block: `Item::sprite` names its layer, and `inventory::push_item` draws one quad for it, in the
same box the cube would have filled. That is the whole of what the item/block split costs the
HUD, and it is why `BlockTextures::load` takes *two* directories: the sprites are layers of the
same array, because they ride the same pipeline, bind group and sampler. A `mesh::NO_TEXTURE`
layer sentinel lets the panel
and highlights ride the same pipeline with no texture at all: the tint *is* the colour.

**The screen's look** is one idea, applied twice: a slot is a *recessed box*, and the card it sits
on is a raised one. `ui::push_bevel` draws a rectangle with its rim — dark along the top and left
where a hollow turns away from the light, light along the bottom and right — which is the whole
difference between a grid of squares and a grid of slots; the card, its title bar and the slot in
hand are the same call with different colours and the bevel the other way round. Across the top
sits a **title bar** with the screen's name in the bitmap font, and under the cursor a **tooltip**
names whatever it is over — `lang.label_of_item` — on a small dark plate, so the screen says what
it is and what you are pointing at rather than leaving you to work it out from a picture. The
plate is placed by `inventory::tooltip_rect`, which sizes itself to the text
and slides back inside the screen near an edge, so a cell in the corner still gets a readable
label. `Layout` places every cell *and* answers what the cursor is over, so the drawing and the
hit test cannot drift apart — a test asserts nothing lands off screen. **How much of a slot an
icon fills** is `inventory::icon_half`: the cube is 1.0 wide and about 1.11 tall *in the same
units*, and a slot is square in **pixels** while NDC is not, so the two axes take scales that
differ by the aspect ratio — the adjustment every square cell makes — and the icon is then sized
so its height fills the slot. One scale on both axes is what once squashed the icons flat, the
blocks' sides crushed to a sliver under the top face and the whole model reading as a third of
the slot; two tests pin the icon's shape and its size in pixels.

The **crosshair** rides that pipeline too: four `NO_TEXTURE` bars — a white cross drawn over
a slightly larger dark one, so it reads against bright sky and dark stone alike — centred on
the screen and measured in NDC like the rest of the HUD, with a hole in the middle so the
pixel being aimed at stays visible. It is drawn only while nothing stands between the player
and the world — the creative screen and the pause menu both take it away — since that is when
the world is being aimed at. Sizing it by NDC rather than pixels hides a trap: on
a small window a bar becomes a sub-pixel line, and a quad that narrow can fall between pixel
centres and rasterize to *nothing* — so a test pins the crosshair's size in pixels at 1080p.

The **pause menu** (`Esc`) is the other screen on that pipeline, and the first thing here to
carry *words*. It offers **Continue** and **Exit**, and pausing does two things at once: the
menu takes the cursor — a menu you cannot click is not a menu — and `App` stops updating the
camera, so the world is frozen behind the panel rather than sliding about under it. Continue
hands the cursor back; Exit stops the event loop; `Esc` does what Continue does. Opening the
menu shuts the creative screen first, so exactly one screen ever holds the cursor, and the
world's own input (look, clicks, the wheel, `E`, the number keys) is simply not consulted while
it is up.

There is no font file. `src/font.rs` is a **5×7 bitmap font** — one `u64` per glyph, seven rows
of five bits, so `0b01110_10001_…` reads like the letter it draws — and every lit run of pixels
in a glyph becomes a `NO_TEXTURE` quad whose tint *is* the ink. That is what lets the menu be
readable while adding no asset, no second pipeline and no second bind group: it is more of the
same HUD geometry the hotbar and the crosshair are made of. Two rules keep text legible at any
window size: a line is sized by its *height* (the glyph is five by seven, so the width follows),
and every horizontal measure is divided by the aspect ratio, exactly as an inventory cell is.
`src/ui.rs` holds the pieces more than one screen needs — `Rect`, `UNIT_SQUARE`, `push_rect` and
`push_bevel` — so the quick-access row, the creative screen, the pause menu, the tooltips and the
font all place their quads the same way. A glyph's row is merged into runs before it is drawn, so a
letter is a handful of quads rather than one per pixel.

### The language

What the game *calls* the things in it lives in a file, not in the code: every block and item has
a label in **`resources/assets/lang/en.json`**, one flat JSON object, which is the shape
Minecraft's own language files have.

```json
{ "block.grass_block": "Grass Block", "item.water_bucket": "Water Bucket" }
```

A key is a category and an id — `block.<id>` for a block, `item.<id>` for an item — and the id is
the snake-case word the thing is known by (`Block::key`, `Item::key`), which is the same word its
textures are named after. `Lang::label_of_block`/`label_of_item` are the two lookups;
`Renderer` reads the file once at startup, beside the textures, and hands it to the screens.
The labels are written the way Minecraft writes them — "Grass Block", not "GRASS BLOCK" — and the
bitmap font folds them to upper case as it draws, because that is the register five pixels wide
does well.

They show up in two places, both of them Minecraft's:

* the creative screen's **tooltip**, naming whatever the cursor is over;
* the **caption** above the quick-access row, which names what is in hand for two seconds after it
  changes — a slot key, the wheel, a pick-block, or a bucket emptying itself into the world. It is
  what `Inventory::announcing` is for, and `mesh_data` takes the *clock* as an argument rather than
  reading one, so the screen stays a pure function of its arguments. The renderer remembers
  *whether the caption is up* in its HUD key, so the mesh is rebuilt once when it appears and once
  when it lapses, and never in between.

**The JSON reader is hand-rolled, and deliberately.** A language file is the only JSON in the
project and holds one shape — an object whose values are strings — so `lang::parse_labels` reads
exactly that and refuses everything else loudly, with the position in the message. That is a
smaller thing than a JSON dependency and a much smaller thing than the bug it prevents: a file
that grew a nested object being silently half-read. If the format ever needs more than a flat map,
*that* is when to reach for a real parser rather than grow this one. A missing label is not fatal —
the HUD falls back to the id, the way Minecraft prints the raw key — but a **test** holds the real
file to every block and item, so forgetting a line fails the build rather than the picture.

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
* **A depth-only pass may have no colour attachment at all**: `color_attachments: &[]` is
  legal, because a pass needs *an* attachment and the depth buffer counts. Build its pipeline
  with `fragment: None` — that field is an `Option` — and the pass then writes nothing but
  depth, which is all a shadow map is. (A pass with no fragment stage cannot alpha-test, so
  nothing cutout casts a cutout-shaped shadow.)
* **Reading that map back takes three things in a row**: a `texture_depth_2d` binding
  (`TextureSampleType::Depth`), a sampler built with `compare: Some(CompareFunction::..)`
  (`SamplerBindingType::Comparison`) and `textureSampleCompareLevel` in the shader. Plain
  `textureSampleCompare` needs implicit derivatives and so may not appear in a branch; the
  `...Level` form may. `Linear` filtering on a comparison sampler averages four stored depths
  per tap, which is percentage-closer filtering for free. A texture that is both drawn into
  *and* sampled needs `RENDER_ATTACHMENT | TEXTURE_BINDING`.
* Readback and resize details that bite: `Device::poll` takes a `PollType`, whose `Wait` is a
  *struct* variant (`{ submission_index, timeout }`) — `PollType::wait_indefinitely()` is the
  convenience for the usual case — and `BufferSlice::get_mapped_range` returns a `Result`.
  `TextureFormat` has no `Display`, so log it as `{format:?}`. A pass whose depth is loaded but
  never written should store `StoreOp::Discard`.

### glam 0.34

* `glam::camera::rh::proj::directx::{perspective, orthographic}` output **z in `[0, 1]`**
  already — wgpu's own convention — and take `near`/`far` as positive distances. So a value
  that projection produces is exactly what a depth texture stores: **do not** rescale a clip
  `z` from `[-1, 1]` on the way to a shadow-map coordinate. Only `x` and `y` need the
  NDC → texture-space remap (and `v` runs *down* a texture, so it is `0.5 - ndc.y * 0.5`).
  Getting the `z` wrong puts every shadow half a depth range out and looks like shadows not
  working at all — `shadow::tests` pins the convention for exactly that reason.

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
   meshing still to come).
4. **Terrain** — ✅ a 3D density field of combined multi-octave `Fbm<Perlin>` noise
   (blended layers, linearly interpolated down each column, like Minecraft), giving
   hills, cliffs and overhangs, plus **oceans filled to `SEA_LEVEL`** and drawn
   translucently (see **Transparency**), and **oak trees** planted on the open grass.
   ✅ **Caves and ravines** are carved into it afterwards by Minecraft Beta's own worm
   carvers — see **Caves & ravines** — with a **lava sea** at `LAVA_LEVEL` (11) filling
   whatever is still open at the bottom of the world.
   Still to come: biomes and flowing water.
5. **Blocks & textures** — ✅ individual PNGs in a `texture_2d_array` (no atlas),
   per-face textures (grass, oak log), greyscale blocks tinted in code (water, oak
   leaves), **blended** (water, glass) and **cut out** (leaves, flowers) passes,
   **cross sprites** (`gfx::mesh::CROSS_PLANES`) for the flowers, and **fluids** — water and
   **lava**, both shaped by `Block::surface_height` and `is_fluid` (see **Water**). Still to
   come: animated textures, **planting flowers** in world generation, and generating lava in
   cave floors.
6. **Lighting** — ✅ Minecraft-style sky light at levels 0–15 (vertical sunlight + a
   flood-fill spread), baked **per corner** as **smooth lighting** — the cells around each corner
   averaged in brightness, plus **ambient occlusion** in the creases — along with Minecraft's
   directional face shading, and
   split into **ambient** (the voxel sky light) and **direct** (a GPU **sun shadow map**),
   so trees, cliffs and walls cast real shadows. ✅ A second **block-light channel** carries the
   light of the blocks that *emit* it — **lava**, both the **lava sea** the carvers leave at the
   bottom of the world and any pool you place from the creative screen — which is lit
   *unshadowed*, so a pool lights the cave around it (see **Emissive blocks & block light**).
   ✅ The **water surface** has a pass of its own: a normal built from the water texture read as a
   height map and from drifting fBm, catching the sun as a highlight and the sky as a sheen (see
   **The water surface** in **Transparency**).
   ✅ **Full bright**: the `N` key, held down, lights every cell as if it held level 15 — the caves
   included, and the sun's shadow put aside with it — so the underground can be read like open
   ground (see **Lighting**). The sea asks for the same thing in the blend pass, so nothing is left
   dark.
   Still to come: torches (the same `Block::light()` hook, a smaller number) and a point-light
   falloff for them in `lighting.wgsl`; cross-chunk light propagation; and a dappled canopy shadow
   and shadows on water (both small changes — see **Shadows**).
7. **Player** — WASD + mouse-look, gravity and AABB collision, jumping.
8. **Interaction** — ✅ voxel raycast break (right-click) and place (left-click), a
   **crosshair** marking the block being aimed at, a
   nine-slot inventory chosen with `1`–`9` or the wheel, a **creative inventory**
   (`E`) whose palette draws every block as a 3D cube, **pick block** on the wheel button, and a
   **pause menu** (`Esc`) offering
   **Continue** and **Exit** — the first screen here to be *labelled*, by the 5×7 bitmap font in
   `src/font.rs`; affected chunks re-mesh automatically. The screens' shared quad primitives
   live in `src/ui.rs`. **Buckets** (`src/item.rs`) put the fluids in and out of the world: a
   slot holds an `Item` — a block or a bucket — which is why the palette draws a bucket as a
   sprite and everything else as a cube (see **Buckets**).
9. **Streaming** — ✅ chunks stream around the player, generated on **worker threads**
   (nearest first) while the frame loop only collects finished ones, and (re)meshed under a
   **time budget** so meshing shares the frame instead of owning it; a chunk is only meshed
   once its neighbours have arrived, so streaming never draws a seam. Still to come: greedy
   meshing, meshing on a worker thread too, and a faster debug build (`opt-level = 1`).
10. **Persistence** — save/load the world (e.g. a simple binary or region format).

## Notes on running

* The app needs a working GPU driver. On this machine it selects the discrete
  GPU (Vulkan). A harmless `wgpu_hal::vulkan::instance` warning about layer
  manifest registry lookups is normal on Windows without the Vulkan SDK.
* `cargo run` opens a real window and blocks until you close it.
* `Cargo.lock` is committed (application, not library) so builds are reproducible.



