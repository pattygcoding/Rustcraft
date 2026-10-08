# Rustcraft

A lightweight Minecraft-style voxel game written in Rust.

Built from scratch on [`winit`](https://crates.io/crates/winit) (windowing) and
[`wgpu`](https://crates.io/crates/wgpu) (rendering) rather than a full game
engine, so we own the world, meshing and physics code.

> **Status:** early — an **infinite** superflat world (bedrock, stone, dirt, grass)
> that streams chunks in and out around the player, meshed with cross-chunk
> hidden-face culling and drawn from individual PNGs in a `texture_2d_array` (no
> atlas), through a first-person fly camera.

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

## Development

```powershell
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

See [`AGENTS.md`](AGENTS.md) for architecture, conventions, the tech-stack
rationale and the roadmap.
