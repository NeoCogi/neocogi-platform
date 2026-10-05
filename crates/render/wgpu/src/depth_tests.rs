//! Real production command replay with depth-tested items and actual UI draws between them.
use super::*;
use wgpu::util::DeviceExt;

/// Concrete test payload keeps errors distinct from triangles and requires no erased render registry.
#[derive(Clone)]
enum DepthCommand {
    /// Seven initialized floats per vertex: clip position xyz followed by RGBA.
    Triangles(Vec<[f32; 7]>),
    /// Recording failure must abandon all unsubmitted passes and permit a later valid frame.
    Reject,
}
/// Immutable geometry retained while a production render pass borrows its native buffer.
struct Prepared {
    /// Packed triangle vertices live until all pass borrows end.
    buffer: wgpu::Buffer,
    /// Complete triangle-list length for the native draw.
    count: u32,
}
#[derive(Default)]
/// A small depth-tested renderer isolates the backend's painter-order contract from mesh materials.
struct DepthApplication {
    /// Persistent pipeline writes depth and compares LessEqual just like the production mesh path.
    pipeline: Option<wgpu::RenderPipeline>,
    /// Prepared entries preserve application command indices, including deliberate failure entries.
    prepared: Vec<Option<Prepared>>,
}
impl WgpuApplicationRenderer for DepthApplication {
    type Command = DepthCommand;

    /// Allocates before any pass can borrow resources and copies the current command's immutable input.
    fn prepare(&mut self, context: WgpuApplicationPrepareContext<'_>, commands: &[&DepthCommand]) -> Result<(), String> {
        if self.pipeline.is_none() {
            let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ordered depth test shader"),
                source: wgpu::ShaderSource::Wgsl(
                    r#"
struct Output { @builtin(position) position: vec4<f32>, @location(0) color: vec4<f32> };
@vertex fn vs(@location(0) position: vec3<f32>, @location(1) color: vec4<f32>) -> Output {
    var output: Output; output.position = vec4<f32>(position, 1.0); output.color = color; return output;
}
@fragment fn fs(input: Output) -> @location(0) vec4<f32> { return input.color; }
"#
                    .into(),
                ),
            });
            self.pipeline = Some(context.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("ordered depth test pipeline"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: 28,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 12,
                                shader_location: 1,
                            },
                        ],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: context.color_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: APPLICATION_DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            }));
        }
        self.prepared = commands
            .iter()
            .map(|command| match command {
                DepthCommand::Triangles(vertices) => {
                    // Explicit byte conversion avoids relying on a Rust struct's padding or layout.
                    let bytes: Vec<_> = vertices.iter().flat_map(|vertex| vertex.iter().flat_map(|value| value.to_ne_bytes())).collect();
                    Some(Prepared {
                        buffer: context.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("ordered depth test vertices"),
                            contents: &bytes,
                            usage: wgpu::BufferUsages::VERTEX,
                        }),
                        count: vertices.len() as u32,
                    })
                }
                DepthCommand::Reject => None,
            })
            .collect();
        Ok(())
    }

    /// Each command may issue multiple triangles; only the backend chooses the independent depth boundary.
    fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>, args: RenderItemArgs, index: usize, command: &'pass DepthCommand) -> Result<(), String> {
        match command {
            DepthCommand::Reject => Err("deliberate independent-item failure".into()),
            DepthCommand::Triangles(_) => {
                let prepared = self.prepared[index].as_ref().unwrap();
                let clip = WgpuRenderer::<Self>::clip_to_scissor(args.view, 32, 32).unwrap();
                pass.set_pipeline(self.pipeline.as_ref().unwrap());
                pass.set_viewport(clip.0 as f32, clip.1 as f32, clip.2 as f32, clip.3 as f32, 0., 1.);
                pass.set_scissor_rect(clip.0, clip.1, clip.2, clip.3);
                pass.set_vertex_buffer(0, prepared.buffer.slice(..));
                pass.draw(0..prepared.count, 0..1);
                Ok(())
            }
        }
    }
}

/// Identical silhouettes with disjoint color/depth reveal stale history and accidental depth disabling.
fn triangle(depth: f32, rgba: [f32; 4]) -> DepthCommand {
    DepthCommand::Triangles(
        [(-0.7, -0.6), (0.6, -0.5), (-0.2, 0.8)]
            .map(|(x, y)| [x, y, depth, rgba[0], rgba[1], rgba[2], rgba[3]])
            .to_vec(),
    )
}
/// Retains a concrete payload at its real position in the production command stream.
fn item(args: RenderItemArgs, command: &DepthCommand) -> RenderCommand<DepthCommand> {
    RenderCommand::Application { args, command: command.clone() }
}
/// Actual UI pipeline triangles overwrite color while their pipeline leaves scene depth untouched.
fn background(rect: Recti) -> RenderCommand<DepthCommand> {
    let vertices = [(0., 0.), (32., 0.), (32., 32.), (0., 0.), (32., 32.), (0., 32.)]
        .map(|(x, y)| GpuVertex::from_vertex(&Vertex::new(Vec2f::new(x, y), Vec2f::new(0., 0.), color4b(0, 255, 0, 255))))
        .to_vec();
    RenderCommand::DrawColored { clip: rect, vertices }
}

/// Replays exactly the production prepare/upload/pass loop into a copyable native-format attachment.
/// Public texture sessions remain UI-only; this private fixture never changes their admission rules.
fn draw(renderer: &mut WgpuRenderer<DepthApplication>, target: &wgpu::Texture, commands: Vec<RenderCommand<DepthCommand>>) -> Result<Vec<u8>, FrameError> {
    renderer.begin(32, 32, color(0, 0, 0, 255));
    renderer.commands = commands;
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = renderer.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    renderer.record_commands(&mut encoder, &view, None)?;
    renderer.queue.submit(Some(encoder.finish()));
    let mut pixels = texture_pixels(&renderer.device, &renderer.queue, target, Dimensioni::new(32, 32));
    if renderer.config.format == wgpu::TextureFormat::Bgra8Unorm {
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
    }
    Ok(pixels)
}

/// Actual WGPU replay preserves painter order across items, UI draws and repeated frames.
#[test]
#[ignore = "requires an SDL native display and WGPU adapter"]
fn independent_items_keep_depth_local_and_preserve_ui_order_and_self_occlusion() {
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    let window = video.window("WGPU independent depth regression", 32, 32).hidden().build().unwrap();
    let mut renderer = WgpuRenderer::new_with_application(
        &window,
        ImageSource { width: 1, height: 1, pixels: &[255; 4] },
        32,
        32,
        DepthApplication::default(),
    )
    .unwrap();
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ordered depth readback target"),
        size: wgpu::Extent3d {
            width: 32,
            height: 32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: renderer.config.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let errors = renderer.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let full = Recti::new(0, 0, 32, 32);
    let args = RenderItemArgs {
        dimensions: Dimensioni::new(32, 32),
        content_area: full,
        view: full,
    };
    let near = triangle(0.2, [1., 0., 0., 1.]);
    let far = triangle(0.8, [0., 0., 1., 1.]);
    let near_only = draw(&mut renderer, &target, vec![item(args, &near)]).unwrap();
    let far_only = draw(&mut renderer, &target, vec![item(args, &far)]).unwrap();
    assert!(near_only.chunks_exact(4).any(|pixel| pixel == [255, 0, 0, 255]));
    assert!(far_only.chunks_exact(4).any(|pixel| pixel == [0, 0, 255, 255]));
    for _ in 0..3 {
        assert_eq!(draw(&mut renderer, &target, vec![item(args, &near), item(args, &far)]).unwrap(), far_only);
        assert_eq!(draw(&mut renderer, &target, vec![item(args, &far), item(args, &near)]).unwrap(), near_only);
        assert_eq!(
            draw(&mut renderer, &target, vec![item(args, &near), item(args, &far), item(args, &near)]).unwrap(),
            near_only
        );
    }
    let painted = draw(&mut renderer, &target, vec![background(full), item(args, &far)]).unwrap();
    assert_eq!(
        draw(&mut renderer, &target, vec![item(args, &near), background(full), item(args, &far)]).unwrap(),
        painted
    );
    let interior = RenderItemArgs { view: Recti::new(7, 5, 19, 13), ..args };
    let clipped = draw(&mut renderer, &target, vec![background(full), item(interior, &far)]).unwrap();
    assert_eq!(
        draw(&mut renderer, &target, vec![item(args, &near), background(full), item(interior, &far)]).unwrap(),
        clipped
    );
    let partial = draw(&mut renderer, &target, vec![item(interior, &far)]).unwrap();
    let mut expected_partial = near_only.clone();
    for (destination, source) in expected_partial.chunks_exact_mut(4).zip(partial.chunks_exact(4)) {
        if source != [0, 0, 0, 255] {
            destination.copy_from_slice(source);
        }
    }
    assert_eq!(
        draw(&mut renderer, &target, vec![item(args, &near), item(interior, &far)]).unwrap(),
        expected_partial,
        "clearing depth must retain preceding item color outside the later silhouette"
    );
    assert_eq!(
        draw(&mut renderer, &target, vec![item(args, &near), item(args, &far), background(full)]).unwrap(),
        [0, 255, 0, 255].repeat(32 * 32)
    );
    // An empty frame still clears all prior color even though there are no application boundaries.
    assert_eq!(draw(&mut renderer, &target, vec![]).unwrap(), [0, 0, 0, 255].repeat(32 * 32));
    assert_eq!(draw(&mut renderer, &target, vec![background(full)]).unwrap(), [0, 255, 0, 255].repeat(32 * 32));

    let DepthCommand::Triangles(mut combined) = near.clone() else {
        unreachable!()
    };
    let DepthCommand::Triangles(far_vertices) = &far else { unreachable!() };
    combined.extend_from_slice(far_vertices);
    assert_eq!(
        draw(&mut renderer, &target, vec![item(args, &DepthCommand::Triangles(combined))]).unwrap(),
        near_only,
        "triangles in one command must retain self-occlusion"
    );

    let before_failure = texture_pixels(&renderer.device, &renderer.queue, &target, Dimensioni::new(32, 32));
    assert!(draw(&mut renderer, &target, vec![item(args, &far), item(args, &DepthCommand::Reject)]).is_err());
    assert_eq!(
        texture_pixels(&renderer.device, &renderer.queue, &target, Dimensioni::new(32, 32)),
        before_failure,
        "failed recording abandons every pass before color submission"
    );
    assert_eq!(
        draw(&mut renderer, &target, vec![item(args, &far)]).unwrap(),
        far_only,
        "failed recording cannot poison the following frame"
    );
    assert!(pollster::block_on(errors.pop()).is_none());
}
