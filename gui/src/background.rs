//! The application backdrop: a soft radial glow -- an off-center, massively "blurred" disc of
//! the playing album's accent color -- on black. The glow is an analytic gaussian falloff, so
//! other shaders (the cover flow's floor reflections) can evaluate the very same function and
//! stay consistent with the backdrop.

use iced::mouse;
use iced::wgpu;
use iced::widget::shader::{self, Viewport};
use iced::{Event, Rectangle};

/// The glow uniform data shared by this shader and the cover flow's: intensity-scaled linear
/// color, center and radius in framebuffer pixels. `color` is animated (it crossfades between
/// tracks), so the center is placed from `seed` instead -- the stable per-album id -- to keep the
/// glow from jumping around mid-crossfade.
pub fn glow_uniform(color: iced::Color, seed: u64, viewport: &Viewport) -> [f32; 8] {
    /// Possible horizontal positions for the glow's center, as a fraction of the viewport size.
    const POSSIBLE_CENTERS_X: [f32; 7] = [0.20, 0.30, 0.40, 0.50, 0.60, 0.70, 0.80];
    /// Possible vertical positions for the glow's center, as a fraction of the viewport size.
    const POSSIBLE_CENTERS_Y: [f32; 4] = [0.10, 0.20, 0.30, 0.40];
    /// The glow's radius, as a fraction of the viewport's larger dimension.
    const RADIUS: f32 = 0.80;
    /// Peak brightness of the glow.
    const INTENSITY: f32 = 0.30;

    // Scatter the glow per album by indexing the position tables with the album's id (already a
    // well-mixed hash). x takes the low digit, y a higher one, so the two axes don't correlate.
    let nx = POSSIBLE_CENTERS_X.len() as u64;
    let ny = POSSIBLE_CENTERS_Y.len() as u64;
    let center_x = POSSIBLE_CENTERS_X[(seed % nx) as usize];
    let center_y = POSSIBLE_CENTERS_Y[(seed / nx % ny) as usize];

    let size = viewport.physical_size();
    let (w, h) = (size.width as f32, size.height as f32);
    let [r, g, b, _a] = color.into_linear();
    let center = (w * center_x, h * center_y);
    let radius = w.max(h) * RADIUS;
    [r * INTENSITY, g * INTENSITY, b * INTENSITY, 1.0, center.0, center.1, radius, 0.0]
}

pub fn background<Message>(color: iced::Color, seed: u64) -> iced::widget::Shader<Message, Background> {
    iced::widget::shader(Background { color, seed }).width(iced::Fill).height(iced::Fill)
}

pub struct Background {
    color: iced::Color,
    /// Stable per-album id, seeds the glow position (see [`glow_uniform`]).
    seed: u64,
}

impl<Message> shader::Program<Message> for Background {
    type State = ();
    type Primitive = Glow;

    fn update(
        &self,
        _state: &mut Self::State,
        _event: &Event,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Option<shader::Action<Message>> {
        None
    }

    fn draw(&self, _state: &Self::State, _cursor: mouse::Cursor, _bounds: Rectangle) -> Glow {
        Glow { color: self.color, seed: self.seed }
    }
}

#[derive(Debug)]
pub struct Glow {
    color: iced::Color,
    seed: u64,
}

impl shader::Primitive for Glow {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        queue.write_buffer(&pipeline.uniforms, 0, bytemuck::cast_slice(&glow_uniform(self.color, self.seed, viewport)));
    }

    fn draw(&self, pipeline: &Pipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &pipeline.uniform_bind, &[]);
        pass.draw(0..3, 0..1);
        true
    }
}

pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("backdrop glow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("background.wgsl").into()),
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("backdrop glow uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("backdrop glow"),
            bind_group_layouts: &[&uniform_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("backdrop glow"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None, // opaque: this is the bottom layer
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("backdrop glow uniforms"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("backdrop glow uniforms"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        Pipeline { pipeline, uniforms, uniform_bind }
    }
}
