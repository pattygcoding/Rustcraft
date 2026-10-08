# Rustcraft

A lightweight Minecraft-style voxel game written in Rust.

Built from scratch on [`winit`](https://crates.io/crates/winit) (windowing) and
[`wgpu`](https://crates.io/crates/wgpu) (rendering) rather than a full game
engine, so we own the world, meshing and physics code.

> **Status:** early — an **infinite**, **noise-generated** world of rolling hills,
> cliffs, overhangs and translucent oceans (flooded to sea level, y = 62), from a 3D
> density field of combined multi-octave Perlin noise (bedrock, stone, dirt, grass,
> water)
> that streams chunks in and out around the player, meshed with cross-chunk
> hidden-face culling and lit with Minecraft-style sky light (0–15) plus
> directional face shading, drawn from individual PNGs in a `texture_2d_array` (no
> atlas), through a first-person fly camera. Right-click breaks blocks, left-click
> places them.

Controls: **mouse** to look, **WASD** to move, **Space**/**Shift** for up/down,
**Ctrl** to sprint, **double-tap Space** to toggle flight. **Right-click** breaks
the block you're looking at, **left-click** places the selected block, and the
**scroll wheel** changes the selection (shown in the hotbar). **Esc** frees the
cursor; click to grab it.

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
