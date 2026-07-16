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
    return vec4<f32>(dither(glow(pos.xy), pos.xy), 1.0);
}

// Break up the 8-bit banding this smooth, dark gradient would otherwise show. Keep in sync with
// the copy in coverflow.wgsl (its reflection floor is the same glow and bands the same way).
//
// The render target is an sRGB format, so the hardware quantises the gamma-*encoded* value on
// write. The dither is therefore applied in that encoded space -- encode, nudge, decode, and let
// the hardware re-encode -- so it lands as a uniform ~1 code step everywhere. A linear-space
// nudge would be swamped by the transfer curve in shadows and oversized in highlights.
fn dither(rgb: vec3<f32>, pos: vec2<f32>) -> vec3<f32> {
    // Two near-independent uniform samples subtracted give triangular-PDF noise on (-1, 1), whose
    // error is signal-independent (no faint contours left where a band edge used to sit).
    let n = hash(pos) - hash(pos + vec2<f32>(37.0, 17.0));
    return srgb_to_linear(linear_to_srgb(rgb) + n / 255.0);
}

// A well-distributed value in [0, 1) per pixel (Dave Hoskins). Nearby pixels decorrelate, so two
// lookups an offset apart are effectively independent samples.
fn hash(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.xyx) * 0.1031);
    p3 += dot(p3, p3.zyx + 31.32);
    return fract((p3.x + p3.y) * p3.z);
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let cc = max(c, vec3<f32>(0.0));
    let lo = cc * 12.92;
    let hi = 1.055 * pow(cc, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, cc <= vec3<f32>(0.0031308));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}
