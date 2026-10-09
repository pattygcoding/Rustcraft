# Rustcraft

A lightweight Minecraft-style voxel game written in Rust.

Built from scratch on [`winit`](https://crates.io/crates/winit) (windowing) and
[`wgpu`](https://crates.io/crates/wgpu) (rendering) rather than a full game
engine, so we own the world, meshing and physics code.

> **Status:** early — an **infinite**, **noise-generated** world of rolling hills,
> cliffs, overhangs and translucent oceans (flooded to sea level, y = 62), from a 3D
> density field of combined multi-octave Perlin noise (bedrock, stone, dirt, grass,
> water) and dotted with short oak trees. The ground is then **carved**: Minecraft
> Beta's own **cave and ravine carvers** walk worms of ellipsoids through the rock,
> forking into networks and cutting the odd long **ravine** across the landscape, and
> everything still open at lava level (y = 11) becomes a **lava sea** — so a shaft deep
> enough ends in a glowing pool. Chunks **stream around the player**, generated on
> worker threads while the frame loop gets on with drawing — the world opens on the ground
> under your feet in under a second and fills in around you — meshed with cross-chunk
> hidden-face culling and drawn from individual PNGs in a `texture_2d_array` (no
> atlas), through a first-person fly camera. The renderer is **deferred** — the world
> goes into a G-buffer and is lit in one fullscreen pass, in **two light channels** —
> with Minecraft-style sky light (0–15) as the ambient half, baked **per corner** with
> Minecraft's **smooth lighting** and ambient occlusion, so shadows soften into gradients and
> walls meet floors in soft dark creases, and a real **sun shadow map** as the direct
> half: trees, cliffs and walls throw shadows across the ground. The other channel is the
> **block light** that **lava** emits — lit *unshadowed*, so the lava sea at the bottom of the
> world lights the caves around it however dark it is. Right-click breaks
> blocks, left-click
> places them, and nine inventory slots hold grass, dirt, stone, oak log, oak planks,
> oak leaves, poppy, dandelion, glass, lava, water and bedrock — stocked from a **creative
> inventory** you open with **E**. A **crosshair** sits in the middle of the screen, showing
> exactly the block a click will break or place.

Controls: **mouse** to look, **WASD** to move, **Space**/**Shift** for up/down,
**Ctrl** to sprint, **double-tap Space** to toggle flight. **Right-click** breaks
the block you're looking at, **left-click** places the selected block, the **scroll
wheel** cycles the nine slots and **1–9** choose one directly. **E** opens the
creative inventory: click a block to pick it up, click a slot to drop it in, or hover
one and press **1–9** to stock that slot. **Esc** closes it (or frees the cursor);
click to grab the cursor back.

## Requirements

* A stable Rust toolchain (edition 2024).
* A GPU with up-to-date drivers (Vulkan/DX12/Metal/GL).

## Running

```powershell
cargo run
```

Enable engine logging with `RUST_LOG`:

```powershell
$env:RUST_LOG = "info"; cargo run
```

Pick a different world by choosing a terrain seed (default `1337`):

```powershell
$env:RUSTCRAFT_SEED = "42"; cargo run
```

## Development

```powershell
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

See [`AGENTS.md`](AGENTS.md) for architecture, conventions, the tech-stack
rationale and the roadmap.
