//! Block textures stored in a GPU `texture_2d_array`.
//!
//! Instead of packing every block into one atlas, each source PNG becomes its own
//! *layer* of a `texture_2d_array`. This keeps the per-frame cost flat as the
//! number of textures grows:
//!
//! * **One bind group, one draw call** covers every block type — draw calls do
//!   not multiply with the number of textures (the whole point).
//! * Layers are independent, so mipmaps never bleed between neighbouring
//!   textures (a classic atlas problem).
//! * **Cheap animation:** an animated texture updates with a single
//!   [`TextureArray::update_layer`] call rather than re-uploading an atlas.
//! * Sources of differing sizes are fine — they are resized to a common size at
//!   load time.
//!
//! The shader selects a layer per vertex, so a block's six faces can each use a
//! different texture with no extra draw calls.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use image::imageops::FilterType;

/// Every block texture is resized to this square size, in texels.
///
/// Change this to match your texture pack (e.g. `32` for higher resolution); all
/// textures in the set are resized to it so they can share one array.
pub const TEXTURE_SIZE: u32 = 16;

/// Format the array is stored in. sRGB so colours round-trip through the sRGB
/// swap-chain correctly.
const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// A GPU 2D texture array: one layer per block texture.
pub struct TextureArray {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    size: u32,
    layer_count: u32,
}

impl TextureArray {
    /// Upload `layers` (one image each) into a new array texture.
    ///
    /// Every image is resized to `size`×`size` with nearest-neighbour sampling so
    /// pixel art stays crisp.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        size: u32,
        layers: &[image::RgbaImage],
    ) -> Self {
        let layer_count = layers.len() as u32;
        assert!(
            layer_count > 0,
            "a block texture array needs at least one layer"
        );
        assert!(
            layer_count <= 256,
            "too many texture layers ({layer_count})"
        );

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: layer_count,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        for (layer, image) in layers.iter().enumerate() {
            let resized = resize_to(image, size);
            write_layer(queue, &texture, layer as u32, size, &resized);
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some(label),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });

        // Nearest filtering keeps the pixel-art look; `Repeat` lets UVs outside
        // 0..1 wrap (some future models tile a texture).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("{label}-sampler")),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Self {
            texture,
            view,
            sampler,
            size,
            layer_count,
        }
    }

    /// The array's view, for binding in a bind group.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// The sampler to pair with the array.
    pub fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    /// How many texture layers the array holds.
    pub fn layer_count(&self) -> u32 {
        self.layer_count
    }

    /// Replace the pixels of a single layer.
    ///
    /// This is the animation hook: animated blocks (water, lava, ...) call it
    /// each frame (or every few frames) for just their own layer — far cheaper
    /// than re-uploading every texture.
    #[allow(dead_code)] // Public API for animated textures; not exercised yet.
    pub fn update_layer(&self, queue: &wgpu::Queue, layer: u32, image: &image::RgbaImage) {
        assert!(layer < self.layer_count, "layer {layer} out of range");
        let image = resize_to(image, self.size);
        write_layer(queue, &self.texture, layer, self.size, &image);
    }
}

/// Copy one image's pixels into a single array layer.
fn write_layer(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    layer: u32,
    size: u32,
    image: &image::RgbaImage,
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: layer,
            },
            aspect: wgpu::TextureAspect::All,
        },
        image.as_raw(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size * 4),
            rows_per_image: Some(size),
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
}

/// Resize an image to `size`×`size` (nearest-neighbour) if it is not already.
fn resize_to(image: &image::RgbaImage, size: u32) -> std::borrow::Cow<'_, image::RgbaImage> {
    if image.width() == size && image.height() == size {
        std::borrow::Cow::Borrowed(image)
    } else {
        std::borrow::Cow::Owned(image::imageops::resize(
            image,
            size,
            size,
            FilterType::Nearest,
        ))
    }
}

/// The set of block textures, addressed by file name.
pub struct BlockTextures {
    array: TextureArray,
    layers: HashMap<String, u32>,
}

impl BlockTextures {
    /// Load every `*.png` in `dir` into a texture array.
    ///
    /// Files are read in sorted order so layer indices are stable between runs,
    /// and the file stem (name without `.png`) is the lookup key, e.g.
    /// `grass_block_top`.
    ///
    /// # Panics
    ///
    /// Panics if the directory cannot be read or a PNG cannot be decoded — a
    /// missing texture should fail loudly at startup, not silently render wrong.
    pub fn load(device: &wgpu::Device, queue: &wgpu::Queue, dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|err| panic!("cannot read texture dir {}: {err}", dir.display()))
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
            })
            .collect();
        paths.sort();

        let mut layers = HashMap::new();
        let mut images = Vec::with_capacity(paths.len());
        for path in &paths {
            let name = path
                .file_stem()
                .expect("texture file has no name")
                .to_string_lossy()
                .into_owned();
            layers.insert(name, images.len() as u32);
            images.push(load_layer(path));
        }
        log::info!(
            "loaded {} block textures from {}",
            images.len(),
            dir.display()
        );

        let array = TextureArray::new(device, queue, "block-textures", TEXTURE_SIZE, &images);
        Self { array, layers }
    }

    /// The underlying texture array.
    pub fn array(&self) -> &TextureArray {
        &self.array
    }

    /// The layer index for a texture name (its file stem).
    ///
    /// # Panics
    ///
    /// Panics if no such texture was loaded, so a typo fails loudly.
    pub fn layer(&self, name: &str) -> u32 {
        *self
            .layers
            .get(name)
            .unwrap_or_else(|| panic!("unknown block texture `{name}`"))
    }
}

/// Load a PNG as an RGBA image resized to [`TEXTURE_SIZE`].
fn load_layer(path: &Path) -> image::RgbaImage {
    let image = image::open(path)
        .unwrap_or_else(|err| panic!("cannot load texture {}: {err}", path.display()));
    let rgba = image.to_rgba8();
    resize_to(&rgba, TEXTURE_SIZE).into_owned()
}
