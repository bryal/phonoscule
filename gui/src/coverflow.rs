//! iPod-style Cover Flow: a custom wgpu shader widget that renders the play queue's cover art as
//! perspective-tilted quads with floor reflections.
//!
//! The widget itself is stateless: the app passes the (fractional, animated) carousel `position`
//! and receives a message with the clicked item's queue index. Rendering uses no depth buffer;
//! quads are drawn back-to-front (iced's custom-primitive render pass has no depth attachment).

use crate::library::FULL;
use glam::{Mat4, Vec3};
use iced::mouse;
use iced::wgpu;
use iced::widget::shader::{self, Viewport};
use iced::{Event, Rectangle};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

/// A cover to show in the flow, at whatever detail is available: the album's accent color (known
/// from the persisted index before any pixels load), the thumbnail, and the on-demand high-res
/// version (see `ensure_hires`) -- the best resident tier is drawn. The high-res bitmap is shared
/// straight from the global cache (`Arc<[u8]>`), not copied.
pub struct FlowCover {
    /// The cover art's id when pixels exist; otherwise any stable stand-in (the album id) -- it
    /// only namespaces the texture cache.
    pub id: u64,
    pub thumb: Option<iced::widget::image::Handle>,
    pub accent: Option<iced::Color>,
    pub full: Option<Arc<[u8]>>,
}

/// GPU texture cache key: a cover id plus its detail tier. Keying on the tier lets a cover's
/// uploads coexist, so an LOD swap is just "draw the better key once it exists".
type TexKey = (u64, Tier);
const PLACEHOLDER_KEY: TexKey = (PLACEHOLDER_ID, Tier::Accent);

/// The detail tiers a cover can be drawn at, lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Tier {
    /// A solid quad of the album's accent color: the zeroth level of detail, before any pixels.
    Accent,
    Thumb,
    Full,
}

/// Distance from the center (in item units) where covers start fading out...
const FADE_START: f32 = 1.0;
/// ...and where they reach full transparency.
const FADE_END: f32 = 7.5;
/// How far to each side covers are still laid out & drawn: just past the fade, so a cover never
/// pops in or out visibly.
const VISIBLE_RANGE: f32 = FADE_END + 0.5;
/// Where the tilted side stacks start, in world units from the center. Covers are 1.0 wide, and
/// the side covers sit further back (see [`SIDE_Z`]), so this is small enough that the nearest
/// side covers tuck slightly under the center cover, like the iPod did.
const SIDE_X: f32 = 0.75;
/// Spacing between covers within a side stack.
const STEP_X: f32 = 0.22;
/// How far side covers recede from the camera.
const SIDE_Z: f32 = -0.8;
/// How much deeper each further cover in a side stack sits, so covers appear progressively
/// smaller (and their spacing tighter) the farther they are from the center.
const STEP_Z: f32 = -0.12;
/// Tilt of the side covers, in radians.
const TILT: f32 = 1.1;
/// Texture cache key used for items without cover art.
const PLACEHOLDER_ID: u64 = u64::MAX;

pub fn cover_flow<Message>(
    covers: Vec<Option<FlowCover>>,
    position: f32,
    glow: iced::Color,
    glow_center: (f32, f32),
    obscured_bottom: f32,
    on_click: fn(usize) -> Message,
) -> iced::widget::Shader<Message, CoverFlow<Message>> {
    iced::widget::shader(CoverFlow { covers, position, glow, glow_center, obscured_bottom, on_click })
        .width(iced::Fill)
        .height(iced::Fill)
}

pub struct CoverFlow<Message> {
    covers: Vec<Option<FlowCover>>,
    position: f32,
    /// The backdrop's glow color and center: reflections fade towards the backdrop, so they must
    /// evaluate the same glow (see the shader).
    glow: iced::Color,
    glow_center: (f32, f32),
    /// Pixels of the widget's bottom hidden behind the player bar. The covers center in the
    /// region above it (only the reflections run down behind the bar).
    obscured_bottom: f32,
    on_click: fn(usize) -> Message,
}

impl<Message> CoverFlow<Message> {
    /// The projection for this frame, lifted so covers center above the obscured bottom strip.
    fn view_proj(&self, bounds: Rectangle) -> Mat4 {
        let aspect = bounds.width / bounds.height.max(1.0);
        // The visible region's center sits this far up in NDC (y up, [-1, 1]); shift the whole
        // scene up to match, which the reflections follow down behind the bar.
        let want = (self.obscured_bottom / bounds.height.max(1.0)).clamp(0.0, 1.0);
        // But never so far that the front cover's top clips the widget's top edge (the shader's
        // scissor rect): on a short window there is simply no room to fully lift it, so cap the
        // shift and let the cover extend down behind the bar instead of vanishing upward.
        let cover_top = view_proj(aspect, 0.0).project_point3(Vec3::new(0.0, 0.5, 0.0)).y;
        const TOP_MARGIN: f32 = 0.05;
        let max_shift = (1.0 - TOP_MARGIN - cover_top).max(0.0);
        view_proj(aspect, want.min(max_shift))
    }
}

impl<Message> shader::Program<Message> for CoverFlow<Message> {
    type State = ();
    type Primitive = Flow;

    fn update(
        &self,
        _state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<shader::Action<Message>> {
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
            let hit = self.hit_test(bounds, cursor)?;
            return Some(shader::Action::publish((self.on_click)(hit)).and_capture());
        }
        None
    }

    fn draw(&self, _state: &Self::State, _cursor: mouse::Cursor, bounds: Rectangle) -> Flow {
        let view_proj = self.view_proj(bounds);
        let mut order = self.visible().collect::<Vec<_>>();
        // No depth buffer: draw back-to-front, i.e. the covers furthest from the center first.
        order.sort_by(|(_, d0), (_, d1)| d1.abs().total_cmp(&d0.abs()));

        let mut instances = Vec::with_capacity(order.len() * 2);
        let mut draws = Vec::with_capacity(order.len());
        let mut uploads = Vec::new();
        for (ix, d) in order {
            // Prefer the high-res tier when it's loaded, else the thumbnail (LOD); an item with no
            // cover at all falls back to the placeholder texture. Queue the pixels for upload (a
            // cheap ref-counted clone) -- `prepare` skips it if that key is already on the GPU.
            let (texture, upload) = match self.covers[ix].as_ref() {
                Some(c) => cover_texture(c),
                None => (PLACEHOLDER_KEY, None),
            };
            if let Some(upload) = upload {
                uploads.push(upload);
            }
            let model = model(d);
            let brightness = 1.0 - 0.4 * d.abs().min(1.0);
            let fade = fade(d);
            let first = instances.len() as u32;
            // Reflection: the same quad translated one unit down in local space; the mirroring
            // happens in the fragment shader.
            instances.push(Instance::new(model * Mat4::from_translation(Vec3::new(0.0, -1.0, 0.0)), 1.0, brightness, fade));
            instances.push(Instance::new(model, 0.0, brightness, fade));
            draws.push(Draw { texture, instances: first..first + 2 });
        }
        Flow { view_proj, glow: self.glow, glow_center: self.glow_center, instances, draws, uploads }
    }

    fn mouse_interaction(&self, _state: &Self::State, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        if self.hit_test(bounds, cursor).is_some() { mouse::Interaction::Pointer } else { mouse::Interaction::default() }
    }
}

impl<Message> CoverFlow<Message> {
    /// The indices near enough to `position` to be laid out, with their fractional offsets.
    fn visible(&self) -> impl Iterator<Item = (usize, f32)> + '_ {
        let lo = ((self.position - VISIBLE_RANGE).ceil() as i64).max(0) as usize;
        let hi = ((self.position + VISIBLE_RANGE).floor() as i64).max(-1) as usize;
        (lo..=hi.min(self.covers.len().saturating_sub(1))).map(move |ix| (ix, ix as f32 - self.position))
    }

    /// Returns the index of the cover under the cursor, front-most first.
    fn hit_test(&self, bounds: Rectangle, cursor: mouse::Cursor) -> Option<usize> {
        let p = cursor.position_in(bounds)?;
        let ndc = glam::vec2(p.x / bounds.width * 2.0 - 1.0, 1.0 - p.y / bounds.height * 2.0);
        let inv = self.view_proj(bounds).inverse();
        let origin = inv.project_point3(Vec3::new(ndc.x, ndc.y, 0.0));
        let target = inv.project_point3(Vec3::new(ndc.x, ndc.y, 0.9));
        let dir = (target - origin).normalize();

        let mut candidates = self.visible().collect::<Vec<_>>();
        candidates.sort_by(|(_, d0), (_, d1)| d0.abs().total_cmp(&d1.abs()));
        for (ix, d) in candidates {
            if fade(d) < 0.1 {
                continue; // all but invisible: don't let it swallow clicks
            }
            let inv_model = model(d).inverse();
            let o = inv_model.transform_point3(origin);
            let dl = inv_model.transform_vector3(dir);
            if dl.z.abs() < 1e-6 {
                continue;
            }
            let t = -o.z / dl.z;
            if t < 0.0 {
                continue;
            }
            let hit = o + dl * t;
            if hit.x.abs() <= 0.5 && hit.y.abs() <= 0.5 {
                return Some(ix);
            }
        }
        None
    }
}

fn view_proj(aspect: f32, ndc_shift_up: f32) -> Mat4 {
    // directx convention: NDC depth in [0, 1], like wgpu.
    let proj = glam::camera::rh::proj::directx::perspective(35_f32.to_radians(), aspect.max(0.1), 0.1, 100.0);
    let view = glam::camera::rh::view::look_at_mat4(Vec3::new(0.0, 0.25, 3.2), Vec3::new(0.0, -0.05, 0.0), Vec3::Y);
    // A post-projection clip-space translation shifts NDC y uniformly at every depth (it adds
    // shift * w before the perspective divide), unlike moving the camera.
    Mat4::from_translation(Vec3::new(0.0, ndc_shift_up, 0.0)) * proj * view
}

/// How visible a cover at offset `d` is: 1.0 up to [`FADE_START`], smoothly falling to 0.0 at
/// [`FADE_END`].
fn fade(d: f32) -> f32 {
    let t = ((FADE_END - d.abs()) / (FADE_END - FADE_START)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t) // smoothstep
}

/// The pose of a cover at fractional offset `d` from the carousel position.
fn model(d: f32) -> Mat4 {
    // Within |d| < 1 the cover swings between the front-facing center pose and the tilted side
    // pose; beyond that it slides along the side stack.
    let swing = d.clamp(-1.0, 1.0);
    let slide = d - swing;
    let x = swing * SIDE_X + slide * STEP_X;
    let z = swing.abs() * SIDE_Z + slide.abs() * STEP_Z;
    let rot_y = -swing * TILT;
    let scale = 1.0 - 0.1 * swing.abs();
    Mat4::from_translation(Vec3::new(x, 0.0, z)) * Mat4::from_rotation_y(rot_y) * Mat4::from_scale(Vec3::splat(scale))
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    model: [f32; 16],
    /// x: 1.0 for the reflection instance, y: brightness, z: fade (alpha), w: padding.
    misc: [f32; 4],
}

impl Instance {
    fn new(model: Mat4, reflection: f32, brightness: f32, fade: f32) -> Self {
        Self { model: model.to_cols_array(), misc: [reflection, brightness, fade, 0.0] }
    }
}

#[derive(Debug)]
struct Draw {
    texture: TexKey,
    instances: std::ops::Range<u32>,
}

struct Upload {
    key: TexKey,
    size: (u32, u32),
    pixels: Pixels,
}

impl fmt::Debug for Upload {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Upload").field("key", &self.key).field("size", &self.size).finish()
    }
}

/// The pixels for a texture upload, from whichever tier we're drawing. The thumbnail lives in
/// iced's image `Handle` (which stores it as `bytes::Bytes`), so we hand that buffer straight to
/// the GPU; the high-res tier is our own `Arc<[u8]>` from the global cache. Both clones are cheap
/// ref-count bumps -- naming each in its own variant keeps a per-frame pixel copy off the table,
/// and confines the `bytes` dependency to the one buffer iced hands us in that type.
enum Pixels {
    Thumb(bytes::Bytes),
    Full(Arc<[u8]>),
    /// A single RGBA pixel: the accent-colored zeroth tier stretches it over the whole quad.
    Solid([u8; 4]),
}

impl Pixels {
    fn as_slice(&self) -> &[u8] {
        match self {
            Pixels::Thumb(pixels) => pixels,
            Pixels::Full(pixels) => pixels,
            Pixels::Solid(pixel) => pixel,
        }
    }
}

/// Chooses which tier to draw for a cover -- the best of high-res, thumbnail, and accent color --
/// returning its texture key and the pixels to upload (a cheap ref-counted clone or a single
/// pixel). `prepare` skips the upload if that key is already resident.
fn cover_texture(cover: &FlowCover) -> (TexKey, Option<Upload>) {
    if let Some(full) = &cover.full {
        let key = (cover.id, Tier::Full);
        (key, Some(Upload { key, size: (FULL, FULL), pixels: Pixels::Full(full.clone()) }))
    } else if let Some(iced::widget::image::Handle::Rgba { width, height, pixels, .. }) = &cover.thumb {
        let key = (cover.id, Tier::Thumb);
        (key, Some(Upload { key, size: (*width, *height), pixels: Pixels::Thumb(pixels.clone()) }))
    } else if let Some(accent) = cover.accent {
        // Dimmed by the same factor as the grid's fallback tiles, so the two zeroth LODs match.
        let level = |c: f32| (0.55 * c * 255.0).round() as u8;
        let key = (cover.id, Tier::Accent);
        (
            key,
            Some(Upload { key, size: (1, 1), pixels: Pixels::Solid([level(accent.r), level(accent.g), level(accent.b), 255]) }),
        )
    } else {
        (PLACEHOLDER_KEY, None)
    }
}

/// One frame of Cover Flow rendering, produced by [`shader::Program::draw`].
#[derive(Debug)]
pub struct Flow {
    view_proj: Mat4,
    glow: iced::Color,
    glow_center: (f32, f32),
    instances: Vec<Instance>,
    draws: Vec<Draw>,
    uploads: Vec<Upload>,
}

impl shader::Primitive for Flow {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        for upload in &self.uploads {
            pipeline.upload_texture(device, queue, upload);
        }
        // Drop textures not drawn this frame, so VRAM tracks the visible carousel rather than
        // every cover ever shown -- important since full-res tiers are ~3 MiB each.
        let live: HashSet<TexKey> = self.draws.iter().map(|d| d.texture).collect();
        pipeline.textures.retain(|key, _| live.contains(key));
        queue.write_buffer(&pipeline.uniforms, 0, bytemuck::cast_slice(&self.view_proj.to_cols_array()));
        // Same glow parameters as the backdrop, so the reflections' floor matches it exactly.
        let glow = crate::background::glow_uniform(self.glow, self.glow_center, viewport);
        queue.write_buffer(&pipeline.uniforms, 64, bytemuck::cast_slice(&glow));
        let instance_bytes: &[u8] = bytemuck::cast_slice(&self.instances);
        if pipeline.instances.size() < instance_bytes.len() as u64 {
            pipeline.instances = instance_buffer(device, instance_bytes.len() as u64);
        }
        queue.write_buffer(&pipeline.instances, 0, instance_bytes);
    }

    fn draw(&self, pipeline: &Pipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &pipeline.uniform_bind, &[]);
        pass.set_vertex_buffer(0, pipeline.vertices.slice(..));
        pass.set_vertex_buffer(1, pipeline.instances.slice(..));
        for draw in &self.draws {
            let texture = pipeline.textures.get(&draw.texture).unwrap_or(&pipeline.placeholder);
            pass.set_bind_group(1, texture, &[]);
            pass.draw(0..6, draw.instances.clone());
        }
        true
    }
}

pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    vertices: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    sampler: wgpu::Sampler,
    texture_layout: wgpu::BindGroupLayout,
    /// The format cover textures are created with, following the render target's color
    /// convention. iced's default `web-colors` mode renders to a non-sRGB target with sRGB values
    /// passed through raw (gamma-space blending, like browsers): covers must then upload as plain
    /// `Rgba8Unorm`, or the hardware sRGB decode has no matching encode and the flow displays
    /// linear values raw -- visibly crushed darks. On an sRGB target, the sRGB variant round-trips.
    texture_format: wgpu::TextureFormat,
    /// GPU texture cache, keyed by [`CoverArt::id`].
    textures: HashMap<TexKey, wgpu::BindGroup>,
    placeholder: wgpu::BindGroup,
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        use wgpu::util::DeviceExt;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("coverflow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("coverflow.wgsl").into()),
        });

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("coverflow uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                // The vertex stage reads the view projection, the fragment stage the floor color.
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("coverflow texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("coverflow"),
            bind_group_layouts: &[&uniform_layout, &texture_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("coverflow"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
                            5 => Float32x4,
                        ],
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // The fragment shader outputs premultiplied alpha.
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // A unit quad centered on the origin, as two triangles.
        #[rustfmt::skip]
        let quad: [f32; 12] = [
            -0.5, -0.5,  0.5, -0.5,  0.5, 0.5,
            -0.5, -0.5,  0.5,  0.5, -0.5, 0.5,
        ];
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("coverflow vertices"),
            contents: bytemuck::cast_slice(&quad),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("coverflow uniforms"),
            size: 64 + 32, // view_proj + the backdrop glow parameters
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("coverflow uniforms"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("coverflow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let texture_format =
            if format.is_srgb() { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
        let placeholder_pixels = [40u8, 40, 46, 255].repeat(4);
        let placeholder =
            make_texture_bind(device, queue, &texture_layout, texture_format, (2, 2), &placeholder_pixels, &sampler);

        Pipeline {
            pipeline,
            vertices,
            uniforms,
            uniform_bind,
            instances: instance_buffer(device, 64 * std::mem::size_of::<Instance>() as u64),
            sampler,
            texture_layout,
            texture_format,
            textures: HashMap::new(),
            placeholder,
        }
    }
}

impl Pipeline {
    fn upload_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, upload: &Upload) {
        if self.textures.contains_key(&upload.key) {
            return;
        }
        let bind = make_texture_bind(
            device,
            queue,
            &self.texture_layout,
            self.texture_format,
            upload.size,
            upload.pixels.as_slice(),
            &self.sampler,
        );
        self.textures.insert(upload.key, bind);
    }
}

fn make_texture_bind(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    (width, height): (u32, u32),
    rgba: &[u8],
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("coverflow cover"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * width), rows_per_image: Some(height) },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("coverflow cover"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}

fn instance_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("coverflow instances"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
