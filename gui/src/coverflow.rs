//! iPod-style Cover Flow: a custom wgpu shader widget that renders the play queue's cover art as
//! perspective-tilted quads with floor reflections.
//!
//! The widget itself is stateless: the app passes the (fractional, animated) carousel `position`
//! and receives a message with the clicked item's queue index. Rendering uses no depth buffer;
//! quads are drawn back-to-front (iced's custom-primitive render pass has no depth attachment).

use crate::library::CoverArt;
use glam::{Mat4, Vec3};
use iced::mouse;
use iced::wgpu;
use iced::widget::shader::{self, Viewport};
use iced::{Event, Rectangle};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// How far to each side (in item units) covers are still laid out & drawn.
const VISIBLE_RANGE: f32 = 7.0;
/// Where the tilted side stacks start, in world units from the center. Covers are 1.0 wide, and
/// the side covers sit further back (see [`SIDE_Z`]), so this is small enough that the nearest
/// side covers tuck slightly under the center cover, like the iPod did.
const SIDE_X: f32 = 0.75;
/// Spacing between covers within a side stack.
const STEP_X: f32 = 0.22;
/// How far side covers recede from the camera.
const SIDE_Z: f32 = -0.8;
/// Tilt of the side covers, in radians.
const TILT: f32 = 1.1;
/// Texture cache key used for items without cover art.
const PLACEHOLDER_ID: u64 = u64::MAX;

pub fn cover_flow<Message>(
    covers: Vec<Option<CoverArt>>,
    position: f32,
    on_click: fn(usize) -> Message,
) -> iced::widget::Shader<Message, CoverFlow<Message>> {
    iced::widget::shader(CoverFlow { covers, position, on_click })
        .width(iced::Fill)
        .height(iced::Fill)
}

pub struct CoverFlow<Message> {
    covers: Vec<Option<CoverArt>>,
    position: f32,
    on_click: fn(usize) -> Message,
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
        let view_proj = view_proj(bounds.width / bounds.height.max(1.0));
        let mut order = self.visible().collect::<Vec<_>>();
        // No depth buffer: draw back-to-front, i.e. the covers furthest from the center first.
        order.sort_by(|(_, d0), (_, d1)| d1.abs().total_cmp(&d0.abs()));

        let mut instances = Vec::with_capacity(order.len() * 2);
        let mut draws = Vec::with_capacity(order.len());
        let mut uploads = Vec::new();
        for (ix, d) in order {
            let cover = self.covers[ix].as_ref();
            let id = cover.map(|c| c.id).unwrap_or(PLACEHOLDER_ID);
            if let Some(c) = cover {
                uploads.push(Upload { id: c.id, size: c.size, rgba: c.rgba.clone() });
            }
            let model = model(d);
            let brightness = 1.0 - 0.4 * d.abs().min(1.0);
            let first = instances.len() as u32;
            // Reflection: the same quad translated one unit down in local space; the mirroring
            // happens in the fragment shader.
            instances.push(Instance::new(model * Mat4::from_translation(Vec3::new(0.0, -1.0, 0.0)), 1.0, brightness));
            instances.push(Instance::new(model, 0.0, brightness));
            draws.push(Draw { texture: id, instances: first..first + 2 });
        }
        Flow { view_proj, instances, draws, uploads }
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if self.hit_test(bounds, cursor).is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
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
        let inv = view_proj(bounds.width / bounds.height.max(1.0)).inverse();
        let origin = inv.project_point3(Vec3::new(ndc.x, ndc.y, 0.0));
        let target = inv.project_point3(Vec3::new(ndc.x, ndc.y, 0.9));
        let dir = (target - origin).normalize();

        let mut candidates = self.visible().collect::<Vec<_>>();
        candidates.sort_by(|(_, d0), (_, d1)| d0.abs().total_cmp(&d1.abs()));
        for (ix, d) in candidates {
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

fn view_proj(aspect: f32) -> Mat4 {
    // directx convention: NDC depth in [0, 1], like wgpu.
    let proj = glam::camera::rh::proj::directx::perspective(35_f32.to_radians(), aspect.max(0.1), 0.1, 100.0);
    let view = glam::camera::rh::view::look_at_mat4(Vec3::new(0.0, 0.25, 3.2), Vec3::new(0.0, -0.05, 0.0), Vec3::Y);
    proj * view
}

/// The pose of a cover at fractional offset `d` from the carousel position.
fn model(d: f32) -> Mat4 {
    // Within |d| < 1 the cover swings between the front-facing center pose and the tilted side
    // pose; beyond that it slides along the side stack.
    let swing = d.clamp(-1.0, 1.0);
    let slide = d - swing;
    let x = swing * SIDE_X + slide * STEP_X;
    let z = swing.abs() * SIDE_Z;
    let rot_y = -swing * TILT;
    let scale = 1.0 - 0.1 * swing.abs();
    Mat4::from_translation(Vec3::new(x, 0.0, z))
        * Mat4::from_rotation_y(rot_y)
        * Mat4::from_scale(Vec3::splat(scale))
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    model: [f32; 16],
    /// x: 1.0 for the reflection instance, y: brightness, zw: padding.
    misc: [f32; 4],
}

impl Instance {
    fn new(model: Mat4, reflection: f32, brightness: f32) -> Self {
        Self { model: model.to_cols_array(), misc: [reflection, brightness, 0.0, 0.0] }
    }
}

#[derive(Debug)]
struct Draw {
    texture: u64,
    instances: std::ops::Range<u32>,
}

struct Upload {
    id: u64,
    size: (u32, u32),
    rgba: Arc<Vec<u8>>,
}

impl fmt::Debug for Upload {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Upload").field("id", &self.id).field("size", &self.size).finish()
    }
}

/// One frame of Cover Flow rendering, produced by [`shader::Program::draw`].
#[derive(Debug)]
pub struct Flow {
    view_proj: Mat4,
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
        _viewport: &Viewport,
    ) {
        for upload in &self.uploads {
            pipeline.upload_texture(device, queue, upload);
        }
        queue.write_buffer(&pipeline.uniforms, 0, bytemuck::cast_slice(&self.view_proj.to_cols_array()));
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
    /// GPU texture cache, keyed by [`CoverArt::id`].
    textures: HashMap<u64, wgpu::BindGroup>,
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
                visibility: wgpu::ShaderStages::VERTEX,
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
            size: 64,
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

        let placeholder_pixels = [40u8, 40, 46, 255].repeat(4);
        let placeholder = make_texture_bind(device, queue, &texture_layout, (2, 2), &placeholder_pixels, &sampler);

        Pipeline {
            pipeline,
            vertices,
            uniforms,
            uniform_bind,
            instances: instance_buffer(device, 64 * std::mem::size_of::<Instance>() as u64),
            sampler,
            texture_layout,
            textures: HashMap::new(),
            placeholder,
        }
    }
}

impl Pipeline {
    fn upload_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, upload: &Upload) {
        if self.textures.contains_key(&upload.id) {
            return;
        }
        let bind = make_texture_bind(device, queue, &self.texture_layout, upload.size, &upload.rgba, &self.sampler);
        self.textures.insert(upload.id, bind);
    }
}

fn make_texture_bind(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
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
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("coverflow cover"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
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
