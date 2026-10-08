//! Scene renderer.
//!
//! This is the seam between the voxel world and GPU draw calls. The chunks around
//! the player (see [`crate::world`]) are meshed — hidden faces culled, one mesh per
//! chunk — and drawn through the first-person [`Camera`]. `update_chunks` uploads
//! meshes for newly streamed chunks and drops meshes for unloaded ones.
//!
//! Every block texture lives in one [`super::texture::TextureArray`], so all block
//! types share a single bind group and draw call. Each chunk's mesh is split in two
//! (see [`super::mesh::MeshData`]): **opaque** geometry is drawn first — nearest
//! chunk first, so hidden fragments are rejected early — with depth writes on, then
//! **translucent** geometry (water) farthest chunk first, blended with depth writes
//! off. See [`Renderer::render`].

use std::collections::HashMap;
use std::sync::Arc;

use winit::window::Window;

use super::context::GraphicsContext;
use super::mesh::{Mesh, Vertex};
use super::texture::BlockTextures;
use crate::camera::Camera;
use crate::hotbar::Hotbar;
use crate::world::{CHUNK_SIZE, World, mesh_chunk};

/// Background colour used to clear each frame (a daytime sky blue).
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.55,
    g: 0.72,
    b: 0.90,
    a: 1.0,
};

/// Depth buffer format. `Depth32Float` is widely supported and needs no feature.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// How many dirty chunks to (re)mesh per frame, so streaming never stalls a frame.
const MESH_BUDGET: usize = 4;

/// The per-frame data handed to the shader (currently just a transform).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    /// Model · view · projection, column-major (as WGSL/wgpu expect).
    mvp: [[f32; 4]; 4],
}

/// Draws a single window using a [`GraphicsContext`].
pub struct Renderer {
    context: GraphicsContext,
    /// Pipeline for opaque (and alpha-cutout) geometry.
    opaque_pipeline: wgpu::RenderPipeline,
    /// Pipeline for blended geometry, drawn after the opaque pass.
    transparent_pipeline: wgpu::RenderPipeline,
    /// Screen-space HUD pipeline (blended, no depth writes).
    ui_pipeline: wgpu::RenderPipeline,
    /// The hotbar HUD mesh, rebuilt when the selection or viewport changes.
    hud_mesh: Option<Mesh>,
    /// The `(selection, width, height)` the HUD mesh was built for.
    hud_key: Option<(usize, u32, u32)>,
    /// One mesh per loaded chunk, keyed by chunk coordinate.
    chunk_meshes: HashMap<(i32, i32), Mesh>,
    /// This frame's chunk draw order, nearest first. Reused between frames so
    /// ordering does not allocate.
    draw_order: Vec<((i32, i32), f32)>,
    /// The block texture array; also used to mesh streamed chunks.
    textures: BlockTextures,
    /// Uniform buffer holding the [`Globals`] for the current frame.
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// Depth buffer; must match the surface size, so it is rebuilt on resize.
    depth_view: wgpu::TextureView,
}

impl Renderer {
    /// Create a renderer and its underlying [`GraphicsContext`] for `window`.
    pub fn new(window: Arc<Window>) -> Self {
        let context = GraphicsContext::new(window);

        let shader = context
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("block-shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/block.wgsl").into()),
            });

        // A uniform buffer to hold the transform for each frame.
        let globals = context.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Load every PNG under `resources/textures` into one array texture.
        let textures = BlockTextures::load(
            context.device(),
            context.queue(),
            concat!(env!("CARGO_MANIFEST_DIR"), "/resources/assets/textures"),
        );
        log::info!(
            "block texture array has {} layers",
            textures.array().layer_count()
        );

        let bind_group_layout =
            context
                .device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("scene-layout"),
                    entries: &[
                        // 0: the per-frame transform.
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::VERTEX,
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
        let opaque_pipeline = create_pipeline(
            context.device(),
            &pipeline_layout,
            &shader,
            format,
            false,
            "blocks-opaque",
        );
        let transparent_pipeline = create_pipeline(
            context.device(),
            &pipeline_layout,
            &shader,
            format,
            true,
            "blocks-transparent",
        );

        // The HUD reuses the same layout and bind group, but draws screen-space
        // quads with a shader that skips the transform.
        let ui_shader = context
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ui-shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/ui.wgsl").into()),
            });
        let ui_pipeline = create_pipeline(
            context.device(),
            &pipeline_layout,
            &ui_shader,
            format,
            true,
            "ui",
        );

        let depth_view = create_depth_view(
            context.device(),
            context.config().width,
            context.config().height,
        );

        Self {
            context,
            opaque_pipeline,
            transparent_pipeline,
            ui_pipeline,
            hud_mesh: None,
            hud_key: None,
            chunk_meshes: HashMap::new(),
            draw_order: Vec::new(),
            textures,
            globals,
            bind_group,
            depth_view,
        }
    }

    /// Rebuild the hotbar HUD mesh if the selection or the viewport changed.
    fn ensure_hud(&mut self, hotbar: &Hotbar) {
        let key = (
            hotbar.selected_index(),
            self.context.config().width,
            self.context.config().height,
        );
        if self.hud_key == Some(key) {
            return;
        }
        let data = hotbar.mesh_data(&self.textures, self.aspect());
        self.hud_mesh = Some(Mesh::upload(self.context.device(), "hotbar", &data));
        self.hud_key = Some(key);
    }

    /// Drop meshes for chunks that are no longer loaded, then (re)mesh a bounded
    /// number of the dirty ones (nearest first) so streaming never stalls a frame.
    pub fn update_chunks(&mut self, world: &mut World) {
        self.chunk_meshes.retain(|pos, _| world.is_loaded(*pos));

        for pos in world.take_dirty(MESH_BUDGET) {
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
        }
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
        // The depth buffer must match the new surface size.
        let config = self.context.config();
        self.depth_view = create_depth_view(self.context.device(), config.width, config.height);
    }

    /// Render one frame from `camera`: clear, draw the world's opaque geometry then
    /// its translucent geometry, then the `hotbar` HUD on top.
    pub fn render(&mut self, camera: &Camera, hotbar: &Hotbar) {
        self.ensure_hud(hotbar);
        self.order_chunks(camera);

        // The scene is in world space, so its model matrix is the identity and
        // the MVP is just the camera's view-projection.
        let globals = Globals {
            mvp: camera.view_projection(self.aspect()).to_cols_array_2d(),
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

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR_COLOR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });

            pass.set_bind_group(0, &self.bind_group, &[]);

            // Opaque / cutout geometry first, writing depth — nearest chunks first so
            // the depth buffer rejects hidden fragments as early as it can.
            pass.set_pipeline(&self.opaque_pipeline);
            for &(pos, _) in &self.draw_order {
                if let Some(mesh) = self.chunk_meshes.get(&pos) {
                    mesh.draw_opaque(&mut pass);
                }
            }

            // Then blended geometry. It tests depth but does not write it, so it must
            // run back to front — here that means walking the near-to-far order
            // backwards — for nearer water to blend over the water behind it.
            pass.set_pipeline(&self.transparent_pipeline);
            for &(pos, _) in self.draw_order.iter().rev() {
                if let Some(mesh) = self.chunk_meshes.get(&pos) {
                    mesh.draw_translucent(&mut pass);
                }
            }

            // Finally the hotbar HUD, drawn on top in screen space.
            if let Some(hud) = &self.hud_mesh {
                pass.set_pipeline(&self.ui_pipeline);
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

/// Build a block pipeline. `transparent` enables alpha blending and turns depth
/// writes off (blended geometry must not occlude itself).
fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    transparent: bool,
    label: &str,
) -> wgpu::RenderPipeline {
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
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: transparent.then_some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Create a depth buffer sized to the surface.
fn create_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth-texture"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}
