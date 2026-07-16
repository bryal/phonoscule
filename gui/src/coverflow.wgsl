// Cover Flow: perspective-tilted cover quads with floor reflections.

struct Uniforms {
    view_proj: mat4x4<f32>,
    // The backdrop glow (see background.wgsl); reflections fade towards the backdrop.
    glow_color: vec4<f32>,
    glow_center: vec2<f32>,
    glow_radius: f32,
    _pad: f32,
}

// Keep in sync with the copy in background.wgsl: this must evaluate the exact backdrop.
fn glow(pos: vec2<f32>) -> vec3<f32> {
    let d = distance(pos, uniforms.glow_center) / uniforms.glow_radius;
    return uniforms.glow_color.rgb * exp(-3.0 * d * d);
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(1) @binding(0) var cover_t: texture_2d<f32>;
@group(1) @binding(1) var cover_s: sampler;

struct VertexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // x: 1.0 when this instance is a reflection, y: brightness, z: fade (alpha)
    @location(1) misc: vec3<f32>,
}

@vertex
fn vs_main(
    @location(0) v_pos: vec2<f32>,
    @location(1) m0: vec4<f32>,
    @location(2) m1: vec4<f32>,
    @location(3) m2: vec4<f32>,
    @location(4) m3: vec4<f32>,
    @location(5) misc: vec4<f32>,
) -> VertexOut {
    let model = mat4x4<f32>(m0, m1, m2, m3);
    var out: VertexOut;
    out.pos = uniforms.view_proj * model * vec4<f32>(v_pos, 0.0, 1.0);
    out.uv = vec2<f32>(v_pos.x + 0.5, 0.5 - v_pos.y);
    out.misc = misc.xyz;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    var uv = in.uv;
    var gradient = 1.0;
    if in.misc.x > 0.5 {
        // Reflection: mirror the texture and fade with distance below the floor. The fade
        // blends toward the floor's own color rather than toward transparency: opaque
        // reflections occlude each other back-to-front like a real mirrored scene, instead of
        // ghosting through one another where covers overlap.
        uv.y = 1.0 - uv.y;
        gradient = 0.3 * pow(1.0 - in.uv.y, 2.0);
    }
    let color = textureSample(cover_t, cover_s, uv);
    let rgb = mix(glow(in.pos.xy), color.rgb * in.misc.y, gradient);
    // Premultiplied alpha; in.misc.z is the carousel's distance fade. Dither before premultiplying
    // so the reflection floor's glow doesn't band (opaque near the center, where the backdrop
    // behind it doesn't show through to carry its own dither).
    return vec4<f32>(dither(rgb, in.pos.xy), color.a) * in.misc.z;
}

// Break up the 8-bit banding on the reflection floor's glow. Keep in sync with the copy in
// background.wgsl (which documents why the dither is applied in the sRGB-encoded space).
fn dither(rgb: vec3<f32>, pos: vec2<f32>) -> vec3<f32> {
    let n = hash(pos) - hash(pos + vec2<f32>(37.0, 17.0));
    return srgb_to_linear(linear_to_srgb(rgb) + n / 255.0);
}

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
