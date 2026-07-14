// The application backdrop: a soft radial glow on black.

struct Uniforms {
    // Premultiplied by the glow intensity, in linear space.
    color: vec4<f32>,
    // Framebuffer pixel coordinates & pixels.
    center: vec2<f32>,
    radius: f32,
    _pad: f32,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

@vertex
fn vs_main(@builtin(vertex_index) ix: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the whole viewport.
    let x = f32(i32(ix % 2u) * 4 - 1);
    let y = f32(i32(ix / 2u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

// Keep in sync with the copy in coverflow.wgsl, which evaluates the same glow as the floor
// color of its reflections.
fn glow(pos: vec2<f32>) -> vec3<f32> {
    let d = distance(pos, uniforms.center) / uniforms.radius;
    return uniforms.color.rgb * exp(-3.0 * d * d);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(glow(pos.xy), 1.0);
}
