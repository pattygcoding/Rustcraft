// The shadow pass: the world drawn from the sun's point of view, keeping only how far
// away each surface is.
//
// This shader has no fragment entry point at all — the pipeline is built without one, so
// the pass writes nothing but depth. Which is all a shadow map is: for every texel, the
// distance to the nearest thing the sun can see there.
//
// The same chunk vertex buffers feed this pass as feed the scene, because a chunk's
// vertices are already in world space (see `World`), so the sun's matrix is the entire
// transform. It is the same geometry, drawn to a different target with a different
// matrix, and that is why shadows cost almost nothing on top of the world we already
// draw.

struct Sun {
    view_proj: mat4x4<f32>,
    // Toward the sun; the trailing `w` is padding.
    direction: vec4<f32>,
    // x: blocks per texel, y: map coordinates per texel.
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> sun: Sun;

@vertex
fn vs_shadow(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return sun.view_proj * vec4<f32>(position, 1.0);
}
