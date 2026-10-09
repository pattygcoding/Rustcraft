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
            mip_level_count: mip_levels(size),
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Each layer is uploaded as a whole mip pyramid, averaged on the CPU. Because the
        // pyramid lives *inside* one layer, a level can never bleed into a neighbouring
        // texture — the atlas problem the module doc promises we avoid.
        for (layer, image) in layers.iter().enumerate() {
            write_mips(queue, &texture, layer as u32, size, image);
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some(label),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });

        // Nearest filtering *within* a level keeps the pixel-art look, and `Repeat` lets
        // UVs outside 0..1 wrap (some future models tile a texture). Between levels the
        // filter is linear, so a block does not visibly pop as it crosses a mip boundary.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("{label}-sampler")),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
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
        // Refresh the whole pyramid, not just level 0: a distant animated texture would
        // otherwise keep sampling the mip levels of the *previous* frame.
        write_mips(queue, &self.texture, layer, self.size, image);
    }
}

/// Copy one image, and every mip level below it, into a single array layer.
fn write_mips(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    layer: u32,
    size: u32,
    image: &image::RgbaImage,
) {
    let levels = mip_levels(size);
    let mut level = resize_to(image, size).into_owned();
    for mip in 0..levels {
        write_layer(queue, texture, layer, mip, &level);
        if mip + 1 < levels {
            level = downsample(&level);
        }
    }
}

/// How many mip levels a `size`×`size` texture has: level 0 and every halving down to
/// 1×1, so 16 gives 5 (16, 8, 4, 2, 1).
fn mip_levels(size: u32) -> u32 {
    size.max(1).ilog2() + 1
}

/// The next mip level down: every 2×2 block of texels averaged into one.
///
/// A plain box filter, on purpose. It averages *alpha* along with colour, which is what a
/// mipmapped cutout needs — a leaf's transparent holes soften into the foliage around them
/// as they shrink, instead of being point-sampled into speckle.
fn downsample(image: &image::RgbaImage) -> image::RgbaImage {
    let (width, height) = (image.width(), image.height());
    let (w, h) = ((width / 2).max(1), (height / 2).max(1));
    let mut out = image::RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut sums = [0u32; 4];
            let mut count = 0u32;
            for sy in 0..2 {
                for sx in 0..2 {
                    let (px, py) = (2 * x + sx, 2 * y + sy);
                    if px < width && py < height {
                        for (channel, value) in image.get_pixel(px, py).0.iter().enumerate() {
                            sums[channel] += u32::from(*value);
                        }
                        count += 1;
                    }
                }
            }
            let pixel = std::array::from_fn(|channel| (sums[channel] / count.max(1)) as u8);
            out.put_pixel(x, y, image::Rgba(pixel));
        }
    }
    out
}

/// Copy one image's pixels into one mip level of one array layer.
fn write_layer(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    layer: u32,
    mip: u32,
    image: &image::RgbaImage,
) {
    let (width, height) = (image.width(), image.height());
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: mip,
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
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
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
    /// Load every `*.png` under each of `dirs` into one texture array.
    ///
    /// Several directories, because **items share the array with blocks**: a bucket is drawn by
    /// the same pipeline, the same bind group and the same sampler as a block face, so it is a
    /// layer like any other — `assets/textures/blocks` and `assets/textures/items`.
    ///
    /// Files are read in sorted order so layer indices are stable between runs,
    /// and the file stem (name without `.png`) is the lookup key, e.g. `grass_block_top`
    /// or `water_bucket`.
    ///
    /// # Panics
    ///
    /// Panics if a directory cannot be read or a PNG cannot be decoded — a
    /// missing texture should fail loudly at startup, not silently render wrong.
    pub fn load(device: &wgpu::Device, queue: &wgpu::Queue, dirs: &[&Path]) -> Self {
        let mut paths: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            paths.extend(
                std::fs::read_dir(dir)
                    .unwrap_or_else(|err| {
                        panic!("cannot read texture dir {}: {err}", dir.display())
                    })
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| {
                        path.extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
                    }),
            );
        }
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
            "loaded {} textures from {} directories",
            images.len(),
            dirs.len()
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
///
/// A PNG that is a **frame strip** — a whole number of squares stacked one above the other, as
/// the animated water and lava textures are — contributes its *first* frame. Squashing a 16×512
/// sheet into one 16×16 tile is not a small version of the picture, it is a different picture:
/// for those blocks it reads as fine banding, and the water's is the height the shader takes its
/// ripples from.
fn load_layer(path: &Path) -> image::RgbaImage {
    let image = image::open(path)
        .unwrap_or_else(|err| panic!("cannot load texture {}: {err}", path.display()));
    let rgba = image.to_rgba8();
    let frame = {
        let (width, height) = frame_size(rgba.width(), rgba.height());
        if (width, height) == rgba.dimensions() {
            std::borrow::Cow::Borrowed(&rgba)
        } else {
            std::borrow::Cow::Owned(
                image::imageops::crop_imm(&rgba, 0, 0, width, height).to_image(),
            )
        }
    };
    resize_to(&frame, TEXTURE_SIZE).into_owned()
}

/// The size of the region one texture contributes: the whole image, or one frame's square if the
/// image is a **frame strip** — taller than it is wide by a whole number of times, so 16×512 is
/// thirty-two 16×16 frames.
///
/// Only the first frame is used for now; moving between them is what
/// [`TextureArray::update_layer`] is for.
fn frame_size(width: u32, height: u32) -> (u32, u32) {
    if width > 0 && height > width && height.is_multiple_of(width) {
        (width, width)
    } else {
        (width, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG taller than it is wide by a whole number of times is a frame strip, and only its
    /// first frame is loaded — a plain texture is loaded whole.
    #[test]
    fn a_frame_strip_contributes_its_first_frame() {
        // The animated blocks' real sizes.
        assert_eq!(frame_size(16, 512), (16, 16), "thirty-two 16x16 frames");
        assert_eq!(frame_size(32, 1024), (32, 32), "and 32x32 ones");
        // A texture that is not a strip is itself.
        assert_eq!(frame_size(16, 16), (16, 16));
        assert_eq!(frame_size(32, 16), (32, 16), "wide is not a strip");
        assert_eq!(frame_size(16, 40), (16, 40), "nor is two and a half frames");
        assert_eq!(frame_size(0, 0), (0, 0), "and nothing stays nothing");
        // Exactly two frames counts.
        assert_eq!(frame_size(16, 32), (16, 16));
    }

    #[test]
    fn mip_levels_halve_down_to_a_single_texel() {
        assert_eq!(mip_levels(16), 5, "16, 8, 4, 2, 1");
        assert_eq!(mip_levels(8), 4);
        assert_eq!(mip_levels(2), 2);
        assert_eq!(mip_levels(1), 1);
    }

    #[test]
    fn downsampling_averages_each_two_by_two_block() {
        let mut image = image::RgbaImage::new(2, 2);
        image.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        image.put_pixel(1, 0, image::Rgba([255, 255, 255, 255]));
        image.put_pixel(0, 1, image::Rgba([100, 200, 40, 255]));
        image.put_pixel(1, 1, image::Rgba([0, 0, 0, 0]));

        let half = downsample(&image);
        assert_eq!(half.dimensions(), (1, 1));
        // (0+255+100+0)/4 = 88 red, 113 green, 73 blue, and the alpha averages too —
        // which is the whole point: a hole fading into its foliage as the texture shrinks.
        assert_eq!(half.get_pixel(0, 0).0, [88, 113, 73, 127]);
    }

    #[test]
    fn a_one_texel_image_still_has_a_level() {
        let image = image::RgbaImage::new(1, 1);
        let half = downsample(&image);
        assert_eq!(half.dimensions(), (1, 1), "cannot halve below one texel");
    }
}
