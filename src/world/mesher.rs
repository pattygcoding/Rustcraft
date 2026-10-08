//! Chunk meshing with hidden-face culling.
//!
//! For every solid block and each of its six faces, the face is emitted **only
//! if the neighbouring block is not solid**. Interior faces between adjacent
//! blocks (the vast majority of blocks in a chunk) are therefore never turned
//! into triangles or vertices — the single biggest win over "one quad per block".
//!
//! Neighbour lookups cross chunk borders via [`World::block`], so the faces where
//! two chunks meet are culled as well — only the world's outer shell is drawn.

use crate::gfx::mesh::{self, MeshData};
use crate::gfx::texture::BlockTextures;

use super::World;
use super::block::Block;
use super::chunk::{Chunk, DEPTH, HEIGHT, WIDTH};

/// Neighbour offset per face, in `[+X, -X, +Y, -Y, +Z, -Z]` order to match
/// [`crate::gfx::mesh::CUBE_FACES`].
const NEIGHBOURS: [[i32; 3]; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];

/// Build mesh data for `chunk`, whose minimum corner is at world-space `origin`.
///
/// `world` is used to sample neighbouring chunks, so shared faces between chunks
/// are culled too.
pub fn mesh_chunk(
    chunk: &Chunk,
    origin: [i32; 3],
    world: &World,
    textures: &BlockTextures,
) -> MeshData {
    let mut data = MeshData::default();

    for y in 0..chunk.height() {
        for z in 0..DEPTH {
            for x in 0..WIDTH {
                let block = chunk.get(x as i32, y as i32, z as i32);
                if !block.is_solid() {
                    continue;
                }

                let faces = block.faces(textures);
                for (face, [dx, dy, dz]) in NEIGHBOURS.iter().copied().enumerate() {
                    let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                    // Skip faces hidden behind a solid neighbour (in this chunk or
                    // a neighbouring one).
                    if neighbour_block(chunk, origin, world, nx, ny, nz).is_solid() {
                        continue;
                    }

                    let block_origin = [
                        (origin[0] + x as i32) as f32,
                        (origin[1] + y as i32) as f32,
                        (origin[2] + z as i32) as f32,
                    ];
                    mesh::push_face(
                        &mut data.vertices,
                        &mut data.indices,
                        block_origin,
                        face,
                        faces.layers[face],
                    );
                }
            }
        }
    }

    // All current blocks are opaque; transparent faces would be appended after
    // this point and recorded via a smaller `opaque_index_count`.
    data.opaque_index_count = data.indices.len() as u32;
    data
}

/// The block at chunk-local `(nx, ny, nz)`.
///
/// Inside the chunk we read directly; outside it we go through the world, which
/// covers both neighbouring chunks and the empty space beyond the world edge.
fn neighbour_block(
    chunk: &Chunk,
    origin: [i32; 3],
    world: &World,
    nx: i32,
    ny: i32,
    nz: i32,
) -> Block {
    if (0..WIDTH as i32).contains(&nx)
        && (0..HEIGHT as i32).contains(&ny)
        && (0..DEPTH as i32).contains(&nz)
    {
        chunk.get(nx, ny, nz)
    } else {
        world.block(origin[0] + nx, origin[1] + ny, origin[2] + nz)
    }
}
