//! Scene renderer.
//!
//! This is the seam between the voxel world and GPU draw calls, and it is **deferred**:
//! rather than shading each fragment as it is drawn, the world is drawn into a G-buffer
//! that records *what* each pixel shows — its colour, the way it faces, how far away it is,
//! and how bright it is — and then one fullscreen pass lights all of it.
//! Expensive lighting is therefore paid once per pixel on screen rather than once per
//! fragment drawn, however many surfaces overlap that pixel.
//!
//! A frame is four passes (see [`Renderer::render`]):
//!
//! 1. **Shadow** — the same chunk meshes, drawn from the sun's point of view, keeping only
//!    depth (see [`super::shadow`]). This is what lets the lighting pass ask whether the
//!    sun reaches a given point.
//! 2. **Geometry** — the opaque and cutout world, into the G-buffer. No lighting at all.
//! 3. **Lighting** — one fullscreen triangle that lights the whole G-buffer onto the
//!    surface.
//! 4. **Blend** — water, then the HUD, over the lit scene. Translucency *cannot* be
//!    deferred: it has to blend in order, so water shades itself and is drawn last.
//!
//! Every block texture lives in one [`super::texture::TextureArray`], so all block types
//! share a single bind group and draw call. `update_chunks` uploads meshes for newly
//! streamed chunks and drops meshes for unloaded ones.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::Vec3;
use wgpu::util::DeviceExt;
use winit::window::Window;

use super::context::GraphicsContext;
use super::mesh::{Mesh, Vertex};
use super::shadow::{self, ShadowMap, Sun};
use super::texture::BlockTextures;
use crate::camera::Camera;
use crate::inventory::Inventory;
use crate::lang::Lang;
use crate::pause::{Action, PauseMenu};
use crate::world::{CHUNK_SIZE, World, mesh_chunk};

/// Background colour used to clear each frame (a daytime sky blue).
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.55,
    g: 0.72,
    b: 0.90,
    a: 1.0,
};

/// Depth buffer format for the scene. The shadow map uses the same format (see
/// [`shadow::FORMAT`]) but is its own, fixed-size texture.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The G-buffer's colour targets: a surface's colour, and the way it faces.
///
/// The albedo is `sRGB` so the hardware turns what the geometry pass writes back into
/// linear as the lighting pass reads it — the two passes then agree about what "half
/// brightness" means. The normal needs no gamma, so it is plain 0..1. Both are one byte a
/// channel, which is what the block textures are anyway.
const ALBEDO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// How long a frame may spend (re)meshing chunks before it gets on with drawing, so streaming
/// never stalls a frame.
///
/// A *time* budget rather than a count of chunks: a chunk costs what it costs, and meshing is the
/// part of streaming that stayed on the render thread. Three milliseconds is a fifth of a 60 Hz
/// frame, which leaves the rest of the frame untouched — and the loop always meshes at least one
/// chunk per frame, however slow that chunk is, so a backlog still drains.
const MESH_BUDGET: Duration = Duration::from_millis(3);

/// The per-frame data handed to the shaders.
///
/// One buffer, shared by every pass, so a field is declared here whether or not every pass reads
/// it — which is why `shaders/block.wgsl` spells out the same layout, and why a test pins the
/// offsets the two have to agree on.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    /// View · projection, column-major (as WGSL/wgpu expect).
    view_proj: [[f32; 4]; 4],
    /// Its inverse. Only the lighting pass needs it, to turn a pixel's depth back into a
    /// world position — but all three passes share one buffer, so it is always here.
    inv_view_proj: [[f32; 4]; 4],
    /// The sun, as a unit vector pointing *toward* it, and where the camera is. Only the water's
    /// ripples read them: nothing else in the picture asks which way the sun faces, which is why
    /// there is no `dot(N, L)` anywhere else. `w` pads each to a `vec4`'s sixteen bytes.
    sun_direction: [f32; 4],
    eye_position: [f32; 4],
    /// Seconds since the renderer started, for the water to drift on, and then the per-frame
    /// *view options* — read by the passes that light something, never by the ones that project
    /// geometry. A `u32` because a switch is not a number, and the two `f32`s after it are
    /// padding: a uniform buffer's size is a multiple of sixteen bytes.
    time: f32,
    /// Non-zero while the player holds the **full bright** key (`N`): light every cell as if it
    /// held level 15, which is what makes a cave as visible as open ground. See
    /// `shaders/lighting.wgsl`.
    full_bright: u32,
    _padding: [f32; 2],
}

/// The shared sunlight prelude: the face shade, the sun/ambient mix and the tint maths.
///
/// WGSL has no `#include`, so [`compose`] pastes these onto the front of every shader. One
/// definition, three users — the deferred pass, the blended water and the HUD — instead of three
/// copies that drift.
const SHARED_SHADER: &str = include_str!("../../shaders/sun.wgsl");

/// The water surface: fBm ripples and a normal from the height map.
///
/// Only `block.wgsl` gets this one — it is where the blended pass lives — so it is a separate
/// prelude rather than part of [`SHARED_SHADER`].
const WATER_SHADER: &str = include_str!("../../shaders/water.wgsl");

/// The source of a shader: the shared preludes, and then the body, in that order.
///
/// Kept apart from [`compose`] so a test can hand the result to a WGSL parser without a GPU.
fn shader_source(preludes: &[&str], body: &str) -> String {
    let mut source = String::with_capacity(SHARED_SHADER.len() + body.len() + 64);
    source.push_str(SHARED_SHADER);
    for prelude in preludes {
        source.push('\n');
        source.push_str(prelude);
    }
    source.push('\n');
    source.push_str(body);
    source
}

/// Paste a shader body onto the shared sunlight prelude, for wgpu.
fn compose(preludes: &[&str], body: &str) -> wgpu::ShaderSource<'static> {
    wgpu::ShaderSource::Wgsl(shader_source(preludes, body).into())
}

/// The G-buffer: what the geometry pass leaves behind for the lighting pass to light.
///
/// Three targets, one texel each per pixel of the screen — see [`ALBEDO_FORMAT`] for why
/// they are the formats they are. Together they add eight bytes a pixel to what the frame
/// costs in memory, which is the price of separating "what a surface is" from "how it is
/// lit".
struct GBuffer {
    /// Block colour in `rgb`, the surface's brightness in `a` (see `world::light`).
    albedo: wgpu::TextureView,
    /// The surface normal as `n * 0.5 + 0.5`.
    normal: wgpu::TextureView,
    /// Depth, which doubles as the depth attachment water and the HUD test against.
    depth: wgpu::TextureView,
}

impl GBuffer {
    /// Build one sized to `width` × `height`.
    ///
    /// Unlike the shadow map — which is sized in *blocks* and so never changes — this one is
    /// sized in pixels and is rebuilt whenever the window changes size.
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let size = wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        };
        // Written by the geometry pass, read by the lighting pass.
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let color = |label: &str, format: wgpu::TextureFormat| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("gbuffer-depth"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            albedo: color("gbuffer-albedo", ALBEDO_FORMAT),
            normal: color("gbuffer-normal", NORMAL_FORMAT),
            depth,
        }
    }
}

/// Everything the HUD mesh is built from, so a frame can tell whether it has to be rebuilt:
/// the quick-access selection, the viewport, which screens are up, and where the cursor is
/// (quantised, because floats cannot be compared for equality).
///
/// Named fields rather than a tuple: it now spans two screens, and a six-tuple whose field
/// order nobody can remember is exactly the thing that goes stale without anyone noticing. The
/// pause menu needs no cursor of its own — a *hovered button* is the whole of what its cursor
/// changes, so the button is what the key holds.
#[derive(Clone, Copy, PartialEq, Eq)]
struct HudKey {
    selected: usize,
    width: u32,
    height: u32,
    inventory_open: bool,
    cursor: [i32; 2],
    paused: bool,
    menu_hovered: Option<Action>,
    /// Whether the caption naming what is in hand is up. It goes away on its own after a moment,
    /// so *this* is what tells a frame that the mesh has to be built again — once when the caption
    /// appears and once when it lapses, and not a single time in between.
    caption: bool,
}

/// Draws a single window using a [`GraphicsContext`].
pub struct Renderer {
    context: GraphicsContext,
    /// Pipeline for the opaque (and alpha-cutout) geometry that fills the G-buffer.
    opaque_pipeline: wgpu::RenderPipeline,
    /// Pipeline for blended geometry, drawn after the deferred lighting pass.
    transparent_pipeline: wgpu::RenderPipeline,
    /// Screen-space HUD pipeline (blended, no depth writes).
    ui_pipeline: wgpu::RenderPipeline,
    /// Pipeline for the shadow pass: depth only, no fragment shader at all.
    shadow_pipeline: wgpu::RenderPipeline,
    /// Pipeline for the one fullscreen pass that lights the G-buffer.
    lighting_pipeline: wgpu::RenderPipeline,
    /// The HUD mesh — the inventory, and whichever screen stands over it — rebuilt whenever
    /// anything it draws changes.
    hud_mesh: Option<Mesh>,
    /// What the HUD mesh was built for. See [`HudKey`].
    hud_key: Option<HudKey>,
    /// One mesh per loaded chunk, keyed by chunk coordinate.
    chunk_meshes: HashMap<(i32, i32), Mesh>,
    /// This frame's chunk draw order, nearest first. Reused between frames so
    /// ordering does not allocate.
    draw_order: Vec<((i32, i32), f32)>,
    /// The block texture array; also used to mesh streamed chunks.
    textures: BlockTextures,
    /// What the HUD calls the things in it — the labels the tooltip and the caption print, read
    /// once at startup from `resources/assets/lang/en.json` (see [`crate::lang`]).
    lang: Lang,
    /// Uniform buffer holding the [`Globals`] for the current frame.
    globals: wgpu::Buffer,
    /// The G-buffer, and the bind group that hands it to the lighting pass. Both are
    /// rebuilt on resize, which is why the layout is kept.
    gbuffer: GBuffer,
    gbuffer_layout: wgpu::BindGroupLayout,
    gbuffer_bind_group: wgpu::BindGroup,
    /// The scene bind group: the shared transform and the block textures.
    bind_group: wgpu::BindGroup,
    /// When this renderer was built, so a frame can say how long the world has been running: the
    /// water's ripples drift on that clock, and nothing else needs a clock at all.
    started: Instant,
    /// The sun: where it is, and the map of what it can see.
    sun: Sun,
    shadow_map: ShadowMap,
    /// The sun's uniform buffer, and the two bind groups that read it — one for the pass
    /// that *draws* the shadow map, one for the pass that samples it.
    sun_uniform: wgpu::Buffer,
    shadow_bind_group: wgpu::BindGroup,
    sun_bind_group: wgpu::BindGroup,
    /// Whether the shadow map needs redrawing: true until it has been drawn once (an
    /// untouched depth texture reads as 0, which would shadow the entire world), and true
    /// again whenever a chunk is remeshed under it.
    shadow_stale: bool,
    /// How long last frame spent rebuilding chunk meshes, in milliseconds — reported by the frame
    /// timing log when a frame runs long, which is where streaming shows up.
    mesh_millis: f32,
}

impl Renderer {
    /// Create a renderer and its underlying [`GraphicsContext`] for `window`.
    pub fn new(window: Arc<Window>) -> Self {
        let context = GraphicsContext::new(window);

        let device = context.device();

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("block-shader"),
            source: compose(&[WATER_SHADER], include_str!("../../shaders/block.wgsl")),
        });
        // The shadow pass needs nothing but a matrix, so it takes no prelude.
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/shadow.wgsl").into()),
        });
        let lighting_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lighting-shader"),
            source: compose(&[], include_str!("../../shaders/lighting.wgsl")),
        });

        // A uniform buffer to hold the transforms for each frame.
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // The sun, and the buffer carrying its matrix to the two passes that need it. The box
        // starts at the origin and is moved onto the player on the first frame.
        let sun = Sun::new(Vec3::ZERO);
        let sun_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sun"),
            contents: bytemuck::bytes_of(&sun.uniform()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let shadow_map = ShadowMap::new(device);

        // Every PNG under the two texture directories, into one array: the blocks, and the items
        // that are drawn beside them (buckets), which share the pipeline and the bind group.
        let textures = BlockTextures::load(
            context.device(),
            context.queue(),
            &[
                Path::new(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/resources/assets/textures/blocks"
                )),
                Path::new(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/resources/assets/textures/items"
                )),
            ],
        );
        log::info!(
            "block texture array has {} layers",
            textures.array().layer_count()
        );

        // The labels the HUD prints, from the one language file there is. A second language would
        // be a second file beside it (see `lang`).
        let lang = Lang::load(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/resources/assets/lang/en.json"
        ));

        let bind_group_layout =
            context
                .device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("scene-layout"),
                    entries: &[
                        // 0: the per-frame transform — and, for the water, the sun and the eye.
                        // Read by the fragment stage too, which is what the water's highlight
                        // needs an eye for, so both stages have to see it.
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // 1: the block texture array.
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2Array,
                                multisampled: false,
                            },
                            count: None,
                        },
                        // 2: the sampler for the array.
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });

        let bind_group = context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scene-bind-group"),
                layout: &bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: globals.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(textures.array().view()),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(textures.array().sampler()),
                    },
                ],
            });

        let pipeline_layout =
            context
                .device()
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("scene-pipeline-layout"),
                    bind_group_layouts: &[Some(&bind_group_layout)],
                    immediate_size: 0,
                });

        let format = context.config().format;
        // The geometry pass fills the G-buffer; nothing is lit until the pass after it, so its
        // two targets are the surface's colour and normal — not the window.
        let opaque_pipeline = create_pipeline(
            device,
            &pipeline_layout,
            &shader,
            &[ALBEDO_FORMAT, NORMAL_FORMAT],
            false,
            "blocks-opaque",
        );
        let transparent_pipeline = create_pipeline(
            device,
            &pipeline_layout,
            &shader,
            &[format],
            true,
            "blocks-transparent",
        );

        // The HUD reuses the same layout and bind group, but draws screen-space
        // quads with a shader that skips the transform.
        let ui_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui-shader"),
            source: compose(&[], include_str!("../../shaders/ui.wgsl")),
        });
        let ui_pipeline =
            create_pipeline(device, &pipeline_layout, &ui_shader, &[format], true, "ui");

        // The shadow pass: the world from the sun's point of view, into its own depth map.
        // The sun's matrix is a binding of its own, so this is the one pass that does not use
        // the scene bind group at all.
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let shadow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow-bind-group"),
            layout: &shadow_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: sun_uniform.as_entire_binding(),
            }],
        });
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("shadow-pipeline-layout"),
                bind_group_layouts: &[Some(&shadow_layout)],
                immediate_size: 0,
            });
        let shadow_pipeline =
            create_shadow_pipeline(device, &shadow_pipeline_layout, &shadow_shader);

        // The lighting pass reads the G-buffer in group 0 and the sun in group 1.
        let gbuffer_layout = create_gbuffer_layout(device);
        let sun_layout = create_sun_layout(device);
        let gbuffer = GBuffer::new(device, context.config().width, context.config().height);
        let gbuffer_bind_group =
            create_gbuffer_bind_group(device, &gbuffer_layout, &globals, &gbuffer);
        let sun_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sun-bind-group"),
            layout: &sun_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: sun_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&shadow_map.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&shadow_map.sampler),
                },
            ],
        });
        let lighting_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("lighting-pipeline-layout"),
                bind_group_layouts: &[Some(&gbuffer_layout), Some(&sun_layout)],
                immediate_size: 0,
            });
        let lighting_pipeline =
            create_lighting_pipeline(device, &lighting_pipeline_layout, &lighting_shader, format);

        Self {
            context,
            opaque_pipeline,
            transparent_pipeline,
            ui_pipeline,
            shadow_pipeline,
            lighting_pipeline,
            hud_mesh: None,
            hud_key: None,
            chunk_meshes: HashMap::new(),
            draw_order: Vec::new(),
            textures,
            globals,
            gbuffer,
            gbuffer_layout,
            gbuffer_bind_group,
            bind_group,
            sun,
            shadow_map,
            sun_uniform,
            shadow_bind_group,
            sun_bind_group,
            // Nothing has been drawn into the shadow map yet.
            shadow_stale: true,
            mesh_millis: 0.0,
            started: Instant::now(),
            lang,
        }
    }

    /// Rebuild the HUD whenever anything either screen draws has changed.
    fn ensure_hud(&mut self, inventory: &Inventory, pause: &PauseMenu) {
        let (width, height) = {
            let config = self.context.config();
            (config.width, config.height)
        };
        // The clock the caption above the quick-access row is measured against. Read here, passed
        // down, and remembered in the key as whether it is up — so the mesh is rebuilt the moment
        // the caption appears and the moment it lapses, and never in between.
        let now = Instant::now();
        let key = HudKey {
            selected: inventory.selected_index(),
            width,
            height,
            inventory_open: inventory.is_open(),
            cursor: inventory.cursor_key(),
            paused: pause.is_open(),
            menu_hovered: pause.hovered(),
            caption: inventory.announcing(now),
        };
        if self.hud_key == Some(key) {
            return;
        }

        let aspect = self.aspect();
        // The crosshair marks the block a click would act on, so it belongs on screen only
        // while nothing stands between the player and the world.
        let aiming = !inventory.is_open() && !pause.is_open();
        let mut data = inventory.mesh_data(&self.textures, aspect, aiming, &self.lang, now);
        // The pause menu is drawn *into* the same geometry, and after the inventory, so it
        // blends over the quick-access row rather than fighting it for the top of the screen.
        if pause.is_open() {
            pause.push_geometry(&mut data.opaque, aspect);
        }

        self.hud_mesh = Some(Mesh::upload(self.context.device(), "hud", &data));
        self.hud_key = Some(key);
    }

    /// Drop meshes for chunks that are no longer loaded, then (re)mesh dirty ones (nearest first)
    /// until this frame's [`MESH_BUDGET`] is spent, so streaming never stalls a frame.
    ///
    /// The budget is a *time*, checked between chunks, rather than a count — which is why the loop
    /// takes one chunk at a time: it can only know what the next chunk would cost by meshing it.
    /// Asking the world for one at a time costs a sort of the dirty set per chunk, which is
    /// microseconds against the milliseconds a chunk takes to mesh.
    pub fn update_chunks(&mut self, world: &mut World) {
        let before = self.chunk_meshes.len();
        self.chunk_meshes.retain(|pos, _| world.is_loaded(*pos));
        let mut changed = self.chunk_meshes.len() != before;

        let started = Instant::now();
        let mut meshed = 0;
        while meshed == 0 || started.elapsed() < MESH_BUDGET {
            let Some(pos) = world.take_dirty(1).pop() else {
                break;
            };
            let origin = world.origin_of(pos);
            let data = {
                let world: &World = world;
                match world.chunk(pos) {
                    Some(chunk) => mesh_chunk(chunk, origin, world, &self.textures),
                    None => continue,
                }
            };
            let mesh = Mesh::upload(self.context.device(), "chunk", &data);
            self.chunk_meshes.insert(pos, mesh);
            changed = true;
            meshed += 1;
        }
        self.mesh_millis = started.elapsed().as_secs_f32() * 1000.0;
        if meshed > 0 {
            log::debug!("meshed {meshed} chunk(s) in {:.1} ms", self.mesh_millis);
        }

        if changed {
            // The shadow map was drawn from the geometry as it stood a moment ago, and a
            // chunk arriving, leaving or being edited has changed what the sun can see.
            self.shadow_stale = true;
        }
    }

    /// How long the last frame spent rebuilding chunk meshes, in milliseconds.
    ///
    /// Streaming happens on the render thread, so this is the share of a long frame that chunk
    /// streaming can be blamed for; the frame timing log prints it.
    pub fn mesh_millis(&self) -> f32 {
        self.mesh_millis
    }

    /// Order the loaded chunks by distance from `camera`, nearest first.
    ///
    /// The opaque pass walks this list forwards — drawing near geometry first lets
    /// the depth buffer reject hidden fragments as early as possible — and the
    /// blended pass walks it backwards, because translucent surfaces have to be
    /// drawn far to near. One distance sort serves both.
    fn order_chunks(&mut self, camera: &Camera) {
        self.draw_order.clear();
        let (cx, cz) = (camera.position.x, camera.position.z);
        self.draw_order
            .extend(self.chunk_meshes.keys().map(|&(x, z)| {
                // Squared distance from the camera to the chunk's centre; ordering
                // does not need the square root.
                let centre = CHUNK_SIZE as f32 * 0.5;
                let dx = x as f32 * CHUNK_SIZE as f32 + centre - cx;
                let dz = z as f32 * CHUNK_SIZE as f32 + centre - cz;
                ((x, z), dx * dx + dz * dz)
            }));
        self.draw_order
            .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    }

    /// Handle a window resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.context.resize(width, height);
        // The G-buffer holds one texel per pixel of the *window*, so all three of its targets
        // have to match the new size — and the lighting pass's bind group still points at the
        // old ones, so it has to be rebuilt alongside them.
        let config = self.context.config();
        self.gbuffer = GBuffer::new(self.context.device(), config.width, config.height);
        self.gbuffer_bind_group = create_gbuffer_bind_group(
            self.context.device(),
            &self.gbuffer_layout,
            &self.globals,
            &self.gbuffer,
        );
    }

    /// Render one frame from `camera`: fill the shadow map, fill the G-buffer, light it, then
    /// blend the world's translucent geometry and the HUD — the `inventory`, and the `pause`
    /// menu over it — on top of the result.
    ///
    /// `full_bright` is the view option the `N` key holds down: light every cell as if it held
    /// level 15, whatever it really holds. It travels in the per-frame uniform rather than as a
    /// pipeline state, because it changes no geometry — only the number the lighting pass starts
    /// from.
    pub fn render(
        &mut self,
        camera: &Camera,
        inventory: &Inventory,
        pause: &PauseMenu,
        full_bright: bool,
    ) {
        self.ensure_hud(inventory, pause);
        self.order_chunks(camera);

        // Move the sun's box onto the player, and rewrite its matrix only when that moved the
        // box by a whole texel — which is what keeps shadow edges from crawling.
        if self.sun.follow(camera.position) {
            self.context.queue().write_buffer(
                &self.sun_uniform,
                0,
                bytemuck::bytes_of(&self.sun.uniform()),
            );
            self.shadow_stale = true;
        }
        // Whether this frame has to redraw the map at all. Standing still over a still world,
        // most frames do not — and that is one whole pass saved.
        let shadow_stale = std::mem::take(&mut self.shadow_stale);

        // The scene is in world space, so its model matrix is the identity and the
        // view-projection is the whole transform. The lighting pass also wants its inverse, to
        // get from a pixel's depth back to a world position.
        let view_proj = camera.view_projection(self.aspect());
        let sun = shadow::direction();
        let globals = Globals {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            sun_direction: [sun.x, sun.y, sun.z, 0.0],
            eye_position: [camera.position.x, camera.position.y, camera.position.z, 0.0],
            time: self.started.elapsed().as_secs_f32(),
            full_bright: u32::from(full_bright),
            _padding: [0.0; 2],
        };
        self.context
            .queue()
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        // Whether the swap chain must be rebuilt once this frame is presented.
        let mut reconfigure = false;

        let frame = match self.context.surface().get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                reconfigure = true;
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.context.reconfigure();
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => return,
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder =
            self.context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("rustcraft-frame"),
                });

        // 1. The shadow pass: the world from the sun's point of view, keeping only depth. It
        //    goes first because the lighting pass reads what it wrote.
        if shadow_stale {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow-pass"),
                // A render pass needs at least one attachment, and this is the only one: no
                // colour target at all, just the sun's depth buffer.
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_map.view,
                    depth_ops: Some(wgpu::Operations {
                        // 1.0 is the far plane, so a texel the sun sees nothing in reads as
                        // "unoccluded" and can never cast a shadow of its own.
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            pass.set_pipeline(&self.shadow_pipeline);
            pass.set_bind_group(0, &self.shadow_bind_group, &[]);
            // The same meshes the scene draws, world-space vertices and all — only the matrix
            // in `sun_uniform` differs, which is the whole reason shadows are cheap here.
            for &(pos, _) in &self.draw_order {
                if let Some(mesh) = self.chunk_meshes.get(&pos) {
                    mesh.draw_opaque(&mut pass);
                }
            }
        }

        // 2. The geometry pass: the world's opaque and cutout geometry into the G-buffer,
        //    nearest chunks first so the depth buffer rejects hidden fragments early.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("geometry-pass"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.gbuffer.albedo,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.gbuffer.normal,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.gbuffer.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            pass.set_pipeline(&self.opaque_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            for &(pos, _) in &self.draw_order {
                if let Some(mesh) = self.chunk_meshes.get(&pos) {
                    mesh.draw_opaque(&mut pass);
                }
            }
        }

        // 3. The lighting pass: one fullscreen triangle reading the G-buffer and the sun, and
        //    writing the lit picture straight to the window. No depth attachment — the depth
        //    it needs is the one it reads.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("lighting-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // The pass writes every pixel, but clearing to the sky keeps the sky
                        // the sky even if the triangle ever misses one.
                        load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });

            pass.set_pipeline(&self.lighting_pipeline);
            pass.set_bind_group(0, &self.gbuffer_bind_group, &[]);
            pass.set_bind_group(1, &self.sun_bind_group, &[]);
            // Three vertices and no vertex buffer at all: the shader builds the triangle.
            pass.draw(0..3, 0..1);
        }

        // 4. What cannot be deferred, over the top of it. Water blends, so it has to be drawn
        //    in order and shaded as it goes — back to front, so nearer water blends over the
        //    water behind it. It tests the scene's depth but must not write it.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blend-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Keep the lit picture this pass is drawn over.
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    // The scene's own depth, so terrain still hides water and the HUD. Loaded
                    // rather than cleared, and discarded afterwards because nothing writes it.
                    view: &self.gbuffer.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            pass.set_pipeline(&self.transparent_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            for &(pos, _) in self.draw_order.iter().rev() {
                if let Some(mesh) = self.chunk_meshes.get(&pos) {
                    mesh.draw_translucent(&mut pass);
                }
            }

            // Finally the HUD, drawn on top in screen space: the quick-access row, and then
            // whichever screen (the creative inventory, the pause menu) is over it.
            if let Some(hud) = &self.hud_mesh {
                pass.set_pipeline(&self.ui_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                hud.draw_opaque(&mut pass);
            }
        }

        self.context.queue().submit(Some(encoder.finish()));
        self.context.queue().present(frame);

        if reconfigure {
            self.context.reconfigure();
        }
    }

    /// The viewport aspect ratio (width / height).
    fn aspect(&self) -> f32 {
        let config = self.context.config();
        config.width as f32 / config.height.max(1) as f32
    }
}

/// Build a block pipeline.
///
/// `formats` are the colour targets it writes, in `@location` order — the G-buffer for the
/// geometry pass, the window for the passes that draw over the finished picture.
/// `transparent` enables alpha blending and turns depth writes off (blended geometry must not
/// occlude itself), and picks the matching fragment entry point: the blended pass is
/// Minecraft's *translucent* type and carries no alpha test, while the opaque pass cuts out
/// (see `shaders/block.wgsl`).
fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    formats: &[wgpu::TextureFormat],
    transparent: bool,
    label: &str,
) -> wgpu::RenderPipeline {
    let fragment_entry = if transparent { "fs_blend" } else { "fs_cutout" };
    let targets: Vec<Option<wgpu::ColorTargetState>> = formats
        .iter()
        .map(|format| {
            Some(wgpu::ColorTargetState {
                format: *format,
                blend: transparent.then_some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })
        })
        .collect();
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(Vertex::LAYOUT)],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            // No culling: with the depth buffer the geometry is correct either way.
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(!transparent),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            compilation_options: Default::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Build the shadow pass's pipeline.
///
/// There is no fragment stage at all: the pass keeps depth and nothing else, which is exactly
/// what a shadow map is. A render pass still needs an attachment of some kind, and the depth
/// buffer is it.
///
/// The `bias` nudges the recorded depth away from the sun. The main defence against a surface
/// shadowing itself is the normal offset in `shaders/lighting.wgsl`; this is the small extra
/// that catches the very steepest angles.
fn create_shadow_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("shadow"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_shadow"),
            compilation_options: Default::default(),
            // A shadow is cast by where the corners *are*, so this pass declares 12 bytes a
            // vertex instead of the 32 the scene reads.
            buffers: &[Some(Vertex::POSITION_LAYOUT)],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            // A chunk's shell is closed, so culling either way would work — and not culling
            // cannot wrongly drop a surface that ought to have cast a shadow.
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: shadow::FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState {
                constant: 1,
                slope_scale: 1.0,
                clamp: 0.0,
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        // No fragment stage: depth is all this pass writes.
        fragment: None,
        multiview_mask: None,
        cache: None,
    })
}

/// Build the deferred lighting pipeline: one fullscreen triangle, no vertex buffer, no depth
/// attachment, drawn straight onto the window.
fn create_lighting_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("lighting"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_fullscreen"),
            compilation_options: Default::default(),
            // The triangle comes from the vertex index, so there is nothing to read.
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        // The depth this pass needs is the scene's own, read out of the G-buffer as a texture.
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_lighting"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// The lighting pass's group 0: the G-buffer, one texel per pixel of the screen.
fn create_gbuffer_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    // The two colour targets are read with `textureLoad` and never sampled, so they need no
    // filtering; the depth buffer is a depth texture, which is the one thing a comparison
    // sampler can read.
    let texture = |binding: u32, sample_type: wgpu::TextureSampleType| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("gbuffer-layout"),
        entries: &[
            // 0: the transforms, so the pass can undo the projection.
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            texture(1, wgpu::TextureSampleType::Float { filterable: false }),
            texture(2, wgpu::TextureSampleType::Float { filterable: false }),
            texture(3, wgpu::TextureSampleType::Depth),
        ],
    })
}

/// The lighting pass's group 1: the sun's matrix, and the map it drew.
fn create_sun_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sun-layout"),
        entries: &[
            // 0: the sun's matrix and direction.
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // 1: the shadow map. A depth texture, because the shadow test *is* a depth
            // comparison.
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            // 2: a *comparison* sampler, so the depth test happens inside the sampler.
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
        ],
    })
}

/// Point the lighting pass at the G-buffer as it stands.
///
/// Kept apart from the layout because the *views* change whenever the window is resized, and a
/// bind group holds the views, not the layout.
fn create_gbuffer_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    globals: &wgpu::Buffer,
    gbuffer: &GBuffer,
) -> wgpu::BindGroup {
    let view = wgpu::BindingResource::TextureView;
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("gbuffer-bind-group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: view(&gbuffer.albedo),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: view(&gbuffer.normal),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: view(&gbuffer.depth),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every shader parses and validates** — a GPU-free way to catch the one kind of mistake
    /// this project cannot otherwise compile-check.
    ///
    /// WGSL is a string as far as `rustc` is concerned, so a typo in it is only found when the
    /// game opens a window and wgpu refuses the module (or, worse, when it does not). `naga` is
    /// the front end and validator wgpu itself uses, and already in the tree as its dependency —
    /// so this runs the real thing, without a device.
    #[test]
    fn every_shader_parses_and_validates() {
        let sources = [
            (
                "block",
                shader_source(&[WATER_SHADER], include_str!("../../shaders/block.wgsl")),
            ),
            (
                "lighting",
                shader_source(&[], include_str!("../../shaders/lighting.wgsl")),
            ),
            (
                "ui",
                shader_source(&[], include_str!("../../shaders/ui.wgsl")),
            ),
            (
                "shadow",
                include_str!("../../shaders/shadow.wgsl").to_owned(),
            ),
        ];

        for (name, source) in sources {
            let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|err| {
                panic!(
                    "{name}.wgsl does not parse:\n{}",
                    err.emit_to_string(&source)
                )
            });
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .unwrap_or_else(|err| {
                panic!(
                    "{name}.wgsl does not validate:\n{}",
                    err.emit_to_string(&source)
                )
            });
        }
    }

    /// The uniform Rust writes and the one the shaders read have to agree byte for byte.
    ///
    /// WGSL lays a `vec4` out at sixteen bytes and a bare `f32` at four, and a field that moved
    /// on one side without the other being told would not fail to compile: it would quietly light
    /// the water with the wrong numbers. This is the test that would notice.
    #[test]
    fn the_globals_layout_is_the_one_the_shaders_expect() {
        use std::mem::{offset_of, size_of};
        assert_eq!(offset_of!(Globals, view_proj), 0);
        assert_eq!(offset_of!(Globals, inv_view_proj), 64);
        assert_eq!(offset_of!(Globals, sun_direction), 128);
        assert_eq!(offset_of!(Globals, eye_position), 144);
        assert_eq!(offset_of!(Globals, time), 160);
        assert_eq!(offset_of!(Globals, full_bright), 164);
        // A uniform buffer's size is a multiple of sixteen bytes; the padding makes it so.
        assert_eq!(size_of::<Globals>(), 176);
        assert!(size_of::<Globals>().is_multiple_of(16));
    }
}
