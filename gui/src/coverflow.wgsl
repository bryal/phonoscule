// Cover Flow: perspective-tilted cover quads with floor reflections.

struct Uniforms {
    view_proj: mat4x4<f32>,
    // The background color; reflections fade towards it.
    floor: vec4<f32>,
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
    let rgb = mix(uniforms.floor.rgb, color.rgb * in.misc.y, gradient);
    // Premultiplied alpha; in.misc.z is the carousel's distance fade.
    return vec4<f32>(rgb, color.a) * in.misc.z;
}
