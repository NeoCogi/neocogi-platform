//! WGPU implementation of the reusable mesh renderer.

use std::{mem, rc::Rc, slice};

use neocogi_render_core::prelude::Recti;
use neocogi_render_wgpu::{APPLICATION_DEPTH_FORMAT, WgpuApplicationPrepareContext, WgpuApplicationRenderer};
use wgpu::util::DeviceExt;

use crate::{MeshBuffers, MeshCommand, MeshSubmission, vertex};

#[cfg(test)]
#[path = "wgpu_tests.rs"]
mod tests;

/// Application renderer owning the mesh renderer's WGPU mesh pipeline and prepared frame resources.
pub struct MeshRenderer {
    /// Pipeline state recreated only if the surface color format changes.
    pipeline: Option<MeshPipeline>,
    /// Immutable geometry uploads shared by retained CPU meshes.
    meshes: Vec<Rc<MeshGeometry>>,
    /// Per-draw uniforms aligned one-for-one with commands in the active frame.
    prepared: Vec<Option<PreparedMesh>>,
}

impl MeshRenderer {
    /// Creates an empty renderer whose resources are allocated during frame preparation.
    pub fn new() -> Self {
        Self {
            pipeline: None,
            meshes: Vec::new(),
            prepared: Vec::new(),
        }
    }
}

impl Default for MeshRenderer {
    /// Uses the same lazy resource construction as [`MeshRenderer::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl WgpuApplicationRenderer for MeshRenderer {
    type Command = MeshCommand;

    /// Builds native buffers and uniforms before any ordered item pass borrows them.
    fn prepare(&mut self, context: WgpuApplicationPrepareContext<'_>, commands: &[&Self::Command]) -> Result<(), String> {
        let needs_pipeline = self
            .pipeline
            .as_ref()
            .map(|pipeline| pipeline.color_format != context.color_format)
            .unwrap_or(true);
        if needs_pipeline {
            self.pipeline = Some(MeshPipeline::new(context.device, context.color_format));
        }
        let pipeline = self.pipeline.as_ref().expect("mesh pipeline was initialized above");

        // Dropped WGPU handles remain alive while submitted command buffers reference them. The
        // next prepared vector can therefore replace the previous frame without a CPU wait.
        self.prepared.clear();
        self.meshes.retain(|mesh| mesh.source.externally_retained());
        self.prepared.reserve(commands.len());
        for command in commands {
            let prepared = match command {
                MeshCommand::DrawMesh(submission) if !submission.mesh.is_empty() => {
                    let geometry = match self.meshes.iter().find(|mesh| mesh.source.same_storage(&submission.mesh)) {
                        Some(mesh) => Rc::clone(mesh),
                        None => {
                            let mesh = Rc::new(MeshGeometry::new(context.device, &submission.mesh)?);
                            self.meshes.push(Rc::clone(&mesh));
                            mesh
                        }
                    };
                    Some(PreparedMesh::new(context.device, &pipeline.bind_group_layout, geometry, submission))
                }
                MeshCommand::DrawMesh(_) => None,
            };
            self.prepared.push(prepared);
        }
        Ok(())
    }

    /// Records an indexed native draw at the command's exact UI painter-order position.
    fn render<'pass>(
        &'pass self,
        pass: &mut wgpu::RenderPass<'pass>,
        args: neocogi_render_core::RenderItemArgs,
        command_index: usize,
        _command: &'pass Self::Command,
    ) -> Result<(), String> {
        let Some(prepared) = self.prepared.get(command_index).and_then(Option::as_ref) else {
            return Ok(());
        };
        let pipeline = self.pipeline.as_ref().ok_or("mesh pipeline was not prepared")?;
        let target = (args.dimensions.width.max(1) as u32, args.dimensions.height.max(1) as u32);
        let Some(scissor) = surface_rect(args.view, target) else {
            return Ok(());
        };

        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &prepared.bind_group, &[]);
        pass.set_vertex_buffer(0, prepared.geometry.vertex_buffer.slice(..));
        pass.set_index_buffer(prepared.geometry.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        // Scene projection belongs to the visible client area, which also bounds rasterization.
        pass.set_viewport(scissor.0 as f32, scissor.1 as f32, scissor.2 as f32, scissor.3 as f32, 0.0, 1.0);
        pass.set_scissor_rect(scissor.0, scissor.1, scissor.2, scissor.3);
        pass.draw_indexed(0..prepared.geometry.index_count, 0, 0..1);
        Ok(())
    }
}

/// Persistent WGPU pipeline objects shared by all mesh commands.
struct MeshPipeline {
    /// Surface format baked into the current render pipeline.
    color_format: wgpu::TextureFormat,
    /// Bind-group layout containing two transform matrices.
    bind_group_layout: wgpu::BindGroupLayout,
    /// Depth-tested triangle-list pipeline for the renderer's encoded vertex format.
    pipeline: wgpu::RenderPipeline,
}

impl MeshPipeline {
    /// Creates the scene mesh shader, uniform layout, and compatible graphics pipeline.
    fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("render.mesh.shader"),
            source: wgpu::ShaderSource::Wgsl(MESH_SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("render.mesh.bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(mem::size_of::<MeshUniforms>() as u64),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("render.mesh.pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("render.mesh.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: vertex::STRIDE as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: vertex::POSITION_OFFSET as u64,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: vertex::NORMAL_OFFSET as u64,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: vertex::UV_OFFSET as u64,
                            shader_location: 2,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: APPLICATION_DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            color_format,
            bind_group_layout,
            pipeline,
        }
    }
}

/// One immutable geometry upload shared by all draws of its retained CPU source.
struct MeshGeometry {
    /// Retains allocation identity until all owners release this cache entry.
    source: MeshBuffers,
    /// Vertex data uploaded once for this immutable source.
    vertex_buffer: wgpu::Buffer,
    /// Triangle indices uploaded once for this immutable source.
    index_buffer: wgpu::Buffer,
    /// Number of indices issued for every draw of this source.
    index_count: u32,
}

impl MeshGeometry {
    /// Creates both GPU buffers once for an immutable mesh allocation.
    fn new(device: &wgpu::Device, source: &MeshBuffers) -> Result<Self, String> {
        let vertices = vertex::PackedVertices::new(source.vertices())?;
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("render.mesh.vertices"),
            contents: vertices.as_bytes(),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("render.mesh.indices"),
            contents: vertex::index_bytes(source.indices()),
            usage: wgpu::BufferUsages::INDEX,
        });
        Ok(Self {
            source: source.clone(),
            vertex_buffer,
            index_buffer,
            index_count: source.indices().len() as u32,
        })
    }
}

/// Per-command transforms retain shared geometry while submitted work completes.
struct PreparedMesh {
    /// Geometry shared with other draws and previous submitted command buffers.
    geometry: Rc<MeshGeometry>,
    /// Uniform allocation retained because the bind group references it.
    _uniform_buffer: wgpu::Buffer,
    /// Binding exposing projection and normal transforms to the vertex shader.
    bind_group: wgpu::BindGroup,
}

impl PreparedMesh {
    /// Uploads only changing transform data for one frame command.
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, geometry: Rc<MeshGeometry>, submission: &MeshSubmission) -> Self {
        let uniforms = MeshUniforms {
            pvm: matrix_values(&submission.pvm),
            view_model: matrix_values(&submission.view_model),
        };
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("render.mesh.uniforms"),
            contents: uniform_bytes(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("render.mesh.bind_group"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        Self {
            geometry,
            _uniform_buffer: uniform_buffer,
            bind_group,
        }
    }
}

/// Uniform memory layout mirrored by the WGSL `MeshUniforms` declaration.
#[repr(C)]
struct MeshUniforms {
    /// Column-major projection-view-model matrix.
    pvm: [f32; 16],
    /// Column-major view-model normal transform.
    view_model: [f32; 16],
}

/// Copies one rs-math3d matrix into a plain, upload-safe float array.
fn matrix_values(matrix: &rs_math3d::Mat4f) -> [f32; 16] {
    let mut values = [0.0; 16];
    // SAFETY: Mat4f exposes four contiguous f32 columns, the representation already consumed by
    // the existing native backends. Copying into a plain array avoids retaining any library type
    // in GPU-facing uniform storage.
    let source = unsafe { slice::from_raw_parts(matrix.col.as_ptr().cast::<f32>(), 16) };
    values.copy_from_slice(source);
    values
}

/// Borrows the shader's two fully initialized dense matrices, never an arbitrary Rust record.
fn uniform_bytes(values: &MeshUniforms) -> &[u8] {
    const {
        assert!(mem::size_of::<MeshUniforms>() == 128);
        assert!(mem::offset_of!(MeshUniforms, pvm) == 0);
        assert!(mem::offset_of!(MeshUniforms, view_model) == 64);
    }
    // SAFETY: the C fields are initialized float arrays; the assertions exclude all padding.
    unsafe { slice::from_raw_parts(std::ptr::from_ref(values).cast::<u8>(), mem::size_of_val(values)) }
}

/// Clamps one logical top-left rectangle to the current WGPU render target.
fn surface_rect(rect: Recti, target: (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    if rect.width <= 0 || rect.height <= 0 {
        return None;
    }
    let left = rect.x.clamp(0, target.0 as i32) as u32;
    let top = rect.y.clamp(0, target.1 as i32) as u32;
    let right = rect.x.saturating_add(rect.width).clamp(0, target.0 as i32) as u32;
    let bottom = rect.y.saturating_add(rect.height).clamp(0, target.1 as i32) as u32;
    (right > left && bottom > top).then_some((left, top, right - left, bottom - top))
}

/// WGPU shader transforming and shading real scene mesh data on the GPU.
const MESH_SHADER: &str = r#"
struct MeshUniforms {
    pvm: mat4x4<f32>,
    view_model: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: MeshUniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) original_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let clip = uniforms.pvm * vec4<f32>(input.position, 1.0);
    // Camera matrices use OpenGL's [-1, 1] depth; WebGPU clip depth is [0, 1].
    output.position = vec4<f32>(clip.xy, clip.z * 0.5 + clip.w * 0.5, clip.w);
    output.normal = normalize((uniforms.view_model * vec4<f32>(input.normal, 0.0)).xyz);
    output.original_normal = input.normal;
    output.uv = input.uv;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let light = normalize(vec3<f32>(0.6, 0.4, 1.0));
    let intensity = max(dot(light, normalize(input.normal)), 0.0);
    let base = input.original_normal * 0.45 + vec3<f32>(0.5) + vec3<f32>(input.uv, 0.0) * 0.1;
    return vec4<f32>(base * intensity + vec3<f32>(0.1), 1.0);
}
"#;
