//! Native glTF viewer using public platform/graphics COM contracts exclusively.
//! The application owns its client domain and every GPU resource until the loop ends.

#[path = "camera.rs"]
mod camera;
#[path = "scene.rs"]
mod scene;

use camera::Camera;
use neocogi_com::{ComBox, ComPtr, INcStringComPtrExt, NcCopyFromBufferOffset, NcStatus, com_object};
use neocogi_render_core::{abi::*, platform::*};
use neocogi_render_gpu::source::{Bytes, PipelineDesc, initial_bytes};
use scene::{MAX_JOINTS, Scene, VERTEX_STRIDE, floats, matrix_bytes};
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

/// Initial physical viewport. Subsequent frames always query the current drawable extent.
const SIZE: Size = Size { width: 960, height: 720 };
/// Canonical WGSL remains an example asset, compiled through the normal shader provider.
const SHADER: &[u8] = include_bytes!("viewer.wgsl");

#[com_object(interfaces(IRenderStream))]
/// One immutable owned block, retained through synchronous render and captured GPU work.
struct Stream {
    /// Exclusive public block owner; renderer receives only its weak observer.
    block: ComBox<IRenderViewBlockAbi>,
}
impl IRenderStream for Stream {
    /// Validates the single contribution index before exposing its bounded observer.
    fn view(&self, _: u64, index: u32) -> Result<neocogi_com::WeakComBox<IRenderViewBlockAbi>, NcStatus> {
        if index == 0 {
            Ok(self.block.downgrade())
        } else {
            Err(neocogi_com::NC_E_INVALID_ARG)
        }
    }
}

#[com_object(interfaces(IInputSink))]
/// Synchronous platform callback adapter; it never mutates the camera or renders reentrantly.
struct InputSink {
    /// Interior mutability is required by IInputSink::input(&self). The application drains
    /// these owned events only after window.poll returns, with no borrow across COM calls.
    events: Rc<RefCell<Vec<UiInput>>>,
}
impl IInputSink for InputSink {
    /// Copies normalized nontext input; this viewer has no text-editing state.
    fn input(&self, event: UiInput, _: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        self.events.borrow_mut().push(event);
        Ok(())
    }
}

/// Exclusive GPU owners for a primitive; geometry remains immutable during animation.
struct Mesh {
    /// Packed 64-byte mesh-local vertex stream.
    vertices: ComBox<IGpuBufferAbi>,
    /// Explicit little-endian U32 triangle list.
    indices: ComBox<IGpuBufferAbi>,
    /// Checked count submitted for each indexed draw.
    count: u32,
}

/// Rendering state separate from imported scene semantics and camera interaction.
struct Graphics {
    /// Shared shader/layout state for rigid and skinned geometry.
    pipeline: ComBox<IGpuPipelineAbi>,
    /// Immutable buffers in the same order as Scene::primitives.
    meshes: Vec<Mesh>,
    /// Authored image owners followed by four single-texel fallback maps.
    textures: Vec<ComBox<IGpuTextureAbi>>,
    /// Repeat/linear sampling with one mip, as supported by the portable profile.
    sampler: ComBox<IGpuSamplerAbi>,
}

impl Graphics {
    /// Uploads all immutable scene data once and validates the shader's exact byte contract.
    fn new(scene: &Scene, resources: &ComPtr<IRenderResourcesAbi>) -> Result<Self, NcStatus> {
        let code = Bytes::source(SHADER.to_vec());
        let vs = resources.create_shader(GpuShaderStage::Vertex, code.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))?;
        let fs = resources.create_shader(GpuShaderStage::Fragment, code.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))?;
        let mut bindings = vec![
            GpuShaderBinding {
                stage: GpuShaderStage::Vertex,
                slot: 0,
                kind: GpuBindingKind::Uniform,
                byte_len: 64,
            },
            GpuShaderBinding {
                stage: GpuShaderStage::Vertex,
                slot: 1,
                kind: GpuBindingKind::Uniform,
                byte_len: (MAX_JOINTS * 64) as u32,
            },
            GpuShaderBinding {
                stage: GpuShaderStage::Fragment,
                slot: 0,
                kind: GpuBindingKind::Uniform,
                byte_len: 64,
            },
        ];
        bindings.extend((0..4).map(|slot| GpuShaderBinding {
            stage: GpuShaderStage::Fragment,
            slot,
            kind: GpuBindingKind::SampledTexture,
            byte_len: 0,
        }));
        let descriptor = PipelineDesc {
            state: GpuPipelineInfo {
                depth_write: true,
                depth_compare: GpuCompare::Less,
                cull: GpuCullMode::Back,
                ..GpuPipelineInfo::default()
            },
            vertex_buffers: vec![GpuVertexBufferLayout {
                slot: 0,
                stride: VERTEX_STRIDE,
                step: GpuStepMode::Vertex,
            }],
            attributes: [
                (GpuVertexFormat::Float32x3, 0),
                (GpuVertexFormat::Float32x3, 12),
                (GpuVertexFormat::Float32x2, 24),
                (GpuVertexFormat::Uint32x4, 32),
                (GpuVertexFormat::Float32x4, 48),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (format, offset))| GpuVertexAttribute {
                location: i as u32,
                slot: 0,
                format,
                offset,
            })
            .collect(),
            bindings,
        }
        .source()?;
        let pipeline = resources.create_pipeline(descriptor.as_com_ref(), vs.as_com_ref(), fs.as_com_ref())?;
        let mut meshes = Vec::new();
        for primitive in &scene.primitives {
            let mut vertices = Vec::with_capacity(primitive.vertices.len() * VERTEX_STRIDE as usize);
            primitive.vertices.iter().for_each(|v| v.encode(&mut vertices));
            let indices: Vec<_> = primitive.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
            meshes.push(Mesh {
                vertices: buffer(resources, GpuBufferRole::Vertex, vertices)?,
                indices: buffer(resources, GpuBufferRole::Index, indices)?,
                count: primitive.indices.len().try_into().map_err(|_| neocogi_com::NC_E_OUT_OF_RANGE)?,
            });
        }
        let mut textures = Vec::new();
        for image in &scene.images {
            textures.push(texture(
                resources,
                Size {
                    width: image.width as i32,
                    height: image.height as i32,
                },
                image.pixels.clone(),
            )?);
        }
        // Base/MR white, flat tangent normal, and black emissive retain ordinary owned textures.
        for pixel in [[255, 255, 255, 255], [255, 255, 255, 255], [128, 128, 255, 255], [0, 0, 0, 255]] {
            textures.push(texture(resources, Size { width: 1, height: 1 }, pixel.to_vec())?);
        }
        let sampler = resources.create_sampler(GpuSamplerDesc {
            min_filter: GpuFilter::Linear,
            mag_filter: GpuFilter::Linear,
            address_u: GpuAddressMode::Repeat,
            address_v: GpuAddressMode::Repeat,
        })?;
        Ok(Self { pipeline, meshes, textures, sampler })
    }

    /// Copies this frame's camera/material/palette uniforms and records indexed depth-tested draws.
    fn record(&self, scene: &Scene, camera: &Camera, seconds: f32, view: &mut ComBox<IRenderViewAbi>) -> Result<(), NcStatus> {
        let pose = scene.pose(seconds);
        view.bind_pipeline(self.pipeline.as_com_ref())?;
        let mut frame = Vec::with_capacity(64);
        matrix_bytes(&mut frame, camera.view_projection());
        view.push_uniforms(GpuShaderStage::Vertex, 0, NcCopyFromBufferOffset::new(0, &frame))?;
        for (primitive, mesh) in scene.primitives.iter().zip(&self.meshes) {
            let palette = scene.palette(primitive, &pose);
            let mut skin = Vec::with_capacity(MAX_JOINTS * 64);
            for index in 0..MAX_JOINTS {
                matrix_bytes(&mut skin, palette.get(index).copied().unwrap_or_else(rs_math3d::Mat4f::identity));
            }
            view.push_uniforms(GpuShaderStage::Vertex, 1, NcCopyFromBufferOffset::new(0, &skin))?;
            let material = &scene.materials[primitive.material];
            let mut parameters = Vec::with_capacity(64);
            floats(&mut parameters, material.base);
            floats(&mut parameters, material.emissive.into_iter().chain([0.]));
            floats(&mut parameters, [material.metal_rough[0], material.metal_rough[1], material.normal_scale, 0.]);
            floats(&mut parameters, camera.eye().into_iter().chain([0.]));
            view.push_uniforms(GpuShaderStage::Fragment, 0, NcCopyFromBufferOffset::new(0, &parameters))?;
            for (slot, image) in material.images.iter().enumerate() {
                let index = image.unwrap_or(scene.images.len() + slot);
                view.bind_texture(
                    GpuShaderStage::Fragment,
                    slot as u32,
                    self.textures[index].as_com_ref(),
                    self.sampler.as_com_ref(),
                )?;
            }
            view.bind_vertex_buffer(0, mesh.vertices.as_com_ref(), 0)?;
            view.bind_index_buffer(mesh.indices.as_com_ref(), GpuIndexFormat::U32, 0)?;
            view.draw_indexed(mesh.count, 1, 0)?;
        }
        Ok(())
    }
}

/// Publishes complete initialized buffer bytes; the caller retains the returned exclusive owner.
fn buffer(resources: &ComPtr<IRenderResourcesAbi>, role: GpuBufferRole, bytes: Vec<u8>) -> Result<ComBox<IGpuBufferAbi>, NcStatus> {
    resources.create_buffer(GpuBufferDesc { role, byte_len: bytes.len() as u64 }, initial_bytes(bytes).borrow()?)
}

/// Uploads one tightly packed UNORM image. Color conversion belongs to the example shader.
fn texture(resources: &ComPtr<IRenderResourcesAbi>, size: Size, pixels: Vec<u8>) -> Result<ComBox<IGpuTextureAbi>, NcStatus> {
    resources.create_texture(
        GpuTextureDesc {
            size,
            format: GpuTextureFormat::Rgba8Unorm,
            mip_count: 1,
        },
        initial_bytes(pixels).borrow()?,
    )
}

/// Host-owned native target and its client admission controller, separate from scene resources.
struct Target {
    /// Native window/event lifetime and current drawable extent.
    window: ComPtr<IPlatformWindowAbi>,
    /// Synchronous renderer for this exact window and device.
    renderer: ComPtr<IRendererAbi>,
    /// Trusted maintenance and recording capability for this device.
    host: ComPtr<IRenderHostAbi>,
    /// Restricted resource factory used by this example's geometry.
    resources: ComPtr<IRenderResourcesAbi>,
    /// Retains domain admission for the entire loop; factory observers cannot own its authority.
    _domain: ComBox<IRenderClientDomainAbi>,
}
impl Target {
    /// Establishes one independent client domain before allocating its scene's GPU resources.
    fn new(window: ComPtr<IPlatformWindowAbi>) -> Result<Self, NcStatus> {
        let clients: ComPtr<IRenderClientFactoryAbi> = window.query()?;
        let domain = clients.create_client(RenderClientLimits::default())?;
        Ok(Self {
            renderer: window.query()?,
            host: window.graphics_host()?,
            resources: domain.resources()?,
            _domain: domain,
            window,
        })
    }

    /// Records one complete contribution and synchronously submits through public contracts.
    fn render(&self, scene: &Scene, graphics: &Graphics, camera: &Camera, seconds: f32, invocation: u64) -> Result<(), NcStatus> {
        self.host.maintain()?;
        let size = self.window.size()?;
        let broker = self.host.recording_broker()?;
        let mut frame = broker.begin_frame()?;
        let bounds = Rect {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height,
        };
        let mut session = frame.begin_view(RenderViewRoot { client: bounds, target: bounds }, self.resources.as_com_ref())?;
        let mut view = session.view()?;
        graphics.record(scene, camera, seconds, &mut view)?;
        view.finish()?;
        let stream: ComPtr<IRenderStreamAbi> = Stream::from_value(Stream { block: session.take_block()? }).query()?;
        frame.finish()?;
        self.renderer.render(
            RenderInfo {
                size,
                clear: 0xff202631,
                invocation,
                block_count: 1,
            },
            stream.as_com_ref(),
        )
    }
}

/// CLI values are parsed before host/window creation, including help and error handling.
struct Options {
    /// Default pinned model path, or an explicitly supplied local compatible glTF/GLB.
    asset: PathBuf,
    /// Optional bounded run for acceptance; absent keeps the interactive viewer open.
    frames: Option<u64>,
    /// Explicit fixed animation sample for reproducible inspection; absent uses elapsed seconds.
    time: Option<f32>,
}
impl Options {
    /// Parses the small native-only command line; malformed options never start graphics.
    fn parse(model: &str, args: impl Iterator<Item = String>) -> scene::Result<Option<Self>> {
        let mut options = Self {
            asset: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("examples/gltf/assets/{model}.glb")),
            frames: None,
            time: None,
        };
        let mut args = args;
        while let Some(option) = args.next() {
            if option == "--help" {
                println!(
                    "{model}: [--asset PATH] [--frames N] [--time SECONDS]\nLeft drag: arcball rotation. Right/middle drag: pan. Wheel: zoom. R: reset. Space: pause. Escape: close."
                );
                return Ok(None);
            }
            let value = args.next().ok_or_else(|| format!("missing value for {option}"))?;
            match option.as_str() {
                "--asset" => options.asset = PathBuf::from(value),
                "--frames" => options.frames = Some(value.parse().ok().filter(|v| *v > 0).ok_or("--frames requires a positive integer")?),
                "--time" => {
                    options.time = Some(
                        value
                            .parse::<f32>()
                            .ok()
                            .filter(|v| v.is_finite() && *v >= 0.)
                            .ok_or("--time requires finite nonnegative seconds")?,
                    )
                }
                _ => return Err(format!("unknown option {option}")),
            }
        }
        Ok(Some(options))
    }
}

/// Starts either requested example; all application errors retain readable scene/GPU context.
pub fn run(model: &str) -> scene::Result<()> {
    let Some(options) = Options::parse(model, std::env::args().skip(1))? else {
        return Ok(());
    };
    let scene = Scene::load(&options.asset)?;
    // SAFETY: This executable links and owns the immutable process-lived COM host.
    let _runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.map_err(|e| format!("COM host: {e:?}"))?;
    let compiler = check(nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject).query())?;
    let platform: ComPtr<PlatformAbi> = check(nc_render::PlatformObject::from_value(nc_render::PlatformObject::new(compiler)).query())?;
    let title = format!("NeoCogi glTF — {model}");
    let result = (|| -> Result<(), NcStatus> {
        let window = platform.create_window(NcCopyFromBufferOffset::new(0, title.as_bytes()), SIZE)?;
        let target = Target::new(window)?;
        let window = &target.window;
        let graphics = Graphics::new(&scene, &target.resources)?;
        let (center, radius) = scene.bounds();
        let mut camera = Camera::new(center, radius, window.size()?);
        let events = Rc::new(RefCell::new(Vec::new()));
        let sink: ComPtr<IInputSinkAbi> = InputSink::from_value(InputSink { events: events.clone() }).query()?;
        let mut previous = Instant::now();
        let mut seconds = 0.;
        let mut paused = false;
        let mut rendered = 0;
        'frames: loop {
            let tick = Instant::now();
            if !paused {
                seconds += tick.duration_since(previous).as_secs_f32();
            }
            previous = tick;
            camera.resize(window.size()?);
            loop {
                let status = window.poll(sink.as_com_ref())?;
                // End the callback's borrow before changing camera/application state.
                let batch = std::mem::take(&mut *events.borrow_mut());
                for event in batch {
                    if let UiInput::Key(key) = &event
                        && key.pressed
                        && !key.repeat
                    {
                        match key.key {
                            KeyCode::Escape => break 'frames,
                            KeyCode::Space => paused = !paused,
                            KeyCode::Character if key.value == u32::from('r') || key.value == u32::from('R') => camera.reset(),
                            _ => {}
                        }
                    }
                    camera.input(event);
                }
                if status == PollStatus::Close {
                    break 'frames;
                }
                if status == PollStatus::Idle {
                    break;
                }
            }
            let size = window.size()?;
            camera.resize(size);
            if size.width > 0 && size.height > 0 {
                target.render(&scene, &graphics, &camera, options.time.unwrap_or(seconds), rendered + 1)?;
                rendered += 1;
                if options.frames.is_some_and(|limit| rendered >= limit) {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        println!("Rendered {rendered} {model} frames.");
        Ok(())
    })();
    result.map_err(|status| {
        let diagnostic = platform.diagnostic().ok().and_then(|message| message.to_string().ok()).unwrap_or_default();
        format!("{model}: {status:?} {diagnostic}")
    })
}

/// Preserves concrete COM status diagnostics at the application error boundary.
fn check<T>(result: Result<T, NcStatus>) -> scene::Result<T> {
    result.map_err(|status| format!("graphics setup: {status:?}"))
}

#[cfg(test)]
#[path = "viewer_tests.rs"]
mod tests;
