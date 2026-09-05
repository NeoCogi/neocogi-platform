//! OpenGL ES 3 / WebGL 2 implementation of the full demo's mesh renderer.

use std::sync::Arc;

use glow::HasContext;
use microui_ui::{prelude::Recti, render::CustomRenderArgs};
use microui_redux_renderer_glow::{GlApplicationRenderContext, GlApplicationRenderer, create_program};

use crate::{DemoCommand, MeshSubmission, MeshVertex};

/// Application renderer owning the demo's GL program and mesh buffers.
pub struct DemoRenderer {
    /// Lazily created resources, initialized only if a frame actually contains a mesh command.
    resources: Option<GlMeshResources>,
}

impl DemoRenderer {
    /// Creates an empty renderer whose GL objects will be allocated on its first mesh draw.
    pub fn new() -> Self {
        Self { resources: None }
    }

    /// Creates persistent GL resources against the active backend context when first needed.
    fn resources(&mut self, gl: &Arc<glow::Context>) -> Result<&mut GlMeshResources, String> {
        if self.resources.is_none() {
            self.resources = Some(GlMeshResources::new(gl.clone())?);
        }
        Ok(self.resources.as_mut().expect("mesh resources were initialized above"))
    }
}

impl Default for DemoRenderer {
    /// Uses the same lazy construction policy as [`DemoRenderer::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl GlApplicationRenderer for DemoRenderer {
    type Command = DemoCommand;

    /// Uploads and draws the command's mesh through native GL buffers and a depth-tested pipeline.
    fn render(&mut self, context: GlApplicationRenderContext<'_>, command: Self::Command) -> Result<(), String> {
        match command {
            DemoCommand::DrawMesh(submission) => self.resources(context.gl)?.draw(context.framebuffer_size, context.args, &submission),
        }
    }
}

/// Persistent native handles used by the GL mesh path.
struct GlMeshResources {
    /// Shared context retained so Drop can delete every application-owned native handle.
    gl: Arc<glow::Context>,
    /// Linked vertex/fragment program implementing transform and normal shading.
    program: glow::Program,
    /// Vertex array recording the demo mesh's attribute and buffer bindings.
    vertex_array: glow::VertexArray,
    /// Dynamic buffer receiving application-owned mesh vertices.
    vertex_buffer: glow::Buffer,
    /// Dynamic buffer receiving application-owned triangle indices.
    index_buffer: glow::Buffer,
}

impl GlMeshResources {
    /// Allocates the application pipeline and empty native buffers transactionally.
    fn new(gl: Arc<glow::Context>) -> Result<Self, String> {
        let program = create_program(&gl, MESH_VERTEX_SHADER, MESH_FRAGMENT_SHADER).map_err(|error| error.to_string())?;
        let vertex_array = match unsafe { gl.create_vertex_array() } {
            Ok(array) => array,
            Err(error) => {
                // The program already exists, but no resource owner can be published after this failure.
                unsafe {
                    gl.delete_program(program);
                }
                return Err(format!("create mesh vertex array failed: {error}"));
            }
        };
        let vertex_buffer = match unsafe { gl.create_buffer() } {
            Ok(buffer) => buffer,
            Err(error) => {
                unsafe {
                    gl.delete_vertex_array(vertex_array);
                    gl.delete_program(program);
                }
                return Err(format!("create mesh vertex buffer failed: {error}"));
            }
        };
        let index_buffer = match unsafe { gl.create_buffer() } {
            Ok(buffer) => buffer,
            Err(error) => {
                unsafe {
                    gl.delete_buffer(vertex_buffer);
                    gl.delete_vertex_array(vertex_array);
                    gl.delete_program(program);
                }
                return Err(format!("create mesh index buffer failed: {error}"));
            }
        };

        Ok(Self {
            gl,
            program,
            vertex_array,
            vertex_buffer,
            index_buffer,
        })
    }

    /// Uploads one mesh and records a native indexed draw in the widget's viewport and clip.
    fn draw(&mut self, framebuffer_size: (u32, u32), args: CustomRenderArgs, submission: &MeshSubmission) -> Result<(), String> {
        if submission.mesh.is_empty() {
            return Ok(());
        }
        let Some(viewport) = gl_rect(args.content_area, args.dimensions.width, args.dimensions.height, framebuffer_size) else {
            return Ok(());
        };
        let Some(scissor) = gl_rect(args.view, args.dimensions.width, args.dimensions.height, framebuffer_size) else {
            return Ok(());
        };

        let gl = &self.gl;
        unsafe {
            // The application's viewport is independent of the UI projection. Its scissor both
            // enforces microui clipping and limits the local depth clear to the visible widget.
            gl.viewport(viewport.0, viewport.1, viewport.2, viewport.3);
            gl.scissor(scissor.0, scissor.1, scissor.2, scissor.3);
            gl.enable(glow::SCISSOR_TEST);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::DEPTH_BUFFER_BIT);

            gl.use_program(Some(self.program));
            gl.bind_vertex_array(Some(self.vertex_array));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vertex_buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(submission.mesh.vertices()), glow::DYNAMIC_DRAW);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.index_buffer));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytes(submission.mesh.indices()), glow::DYNAMIC_DRAW);

            // MeshVertex is repr(C), so the explicit offsets match its three packed float arrays.
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, std::mem::size_of::<MeshVertex>() as i32, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, std::mem::size_of::<MeshVertex>() as i32, 12);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 2, glow::FLOAT, false, std::mem::size_of::<MeshVertex>() as i32, 24);

            set_matrix(gl, self.program, "uPvm", &submission.pvm)?;
            set_matrix(gl, self.program, "uViewModel", &submission.view_model)?;
            gl.draw_elements(glow::TRIANGLES, submission.mesh.indices().len() as i32, glow::UNSIGNED_INT, 0);

            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
        Ok(())
    }
}

impl Drop for GlMeshResources {
    /// Deletes application-owned GL handles while the retained shared context is still alive.
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_buffer(self.index_buffer);
            self.gl.delete_buffer(self.vertex_buffer);
            self.gl.delete_vertex_array(self.vertex_array);
            self.gl.delete_program(self.program);
        }
    }
}

/// Reinterprets a plain repr(C) slice as bytes for native buffer upload.
fn bytes<T>(values: &[T]) -> &[u8] {
    // SAFETY: the returned slice borrows the original contiguous allocation and uses its exact
    // byte length. MeshVertex and u32 contain no references or invalid padding requirements for a
    // raw GPU copy.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), std::mem::size_of_val(values)) }
}

/// Uploads one rs-math3d column-major matrix to a named uniform.
unsafe fn set_matrix(gl: &glow::Context, program: glow::Program, name: &str, matrix: &rs_math3d::Mat4f) -> Result<(), String> {
    let location = unsafe { gl.get_uniform_location(program, name) }.ok_or_else(|| format!("mesh shader uniform `{name}` is unavailable"))?;
    let values = unsafe { std::slice::from_raw_parts(matrix.col.as_ptr().cast::<f32>(), 16) };
    unsafe {
        gl.uniform_matrix_4_f32_slice(Some(&location), false, values);
    }
    Ok(())
}

/// Converts a logical top-left rectangle into a clamped GL bottom-left pixel rectangle.
fn gl_rect(rect: Recti, logical_width: i32, logical_height: i32, framebuffer: (u32, u32)) -> Option<(i32, i32, i32, i32)> {
    if logical_width <= 0 || logical_height <= 0 || rect.width <= 0 || rect.height <= 0 {
        return None;
    }
    let scale_x = framebuffer.0 as f32 / logical_width as f32;
    let scale_y = framebuffer.1 as f32 / logical_height as f32;
    let left = ((rect.x as f32) * scale_x).floor().clamp(0.0, framebuffer.0 as f32) as i32;
    let right = (((rect.x + rect.width) as f32) * scale_x).ceil().clamp(0.0, framebuffer.0 as f32) as i32;
    let top = ((rect.y as f32) * scale_y).floor().clamp(0.0, framebuffer.1 as f32) as i32;
    let bottom = (((rect.y + rect.height) as f32) * scale_y).ceil().clamp(0.0, framebuffer.1 as f32) as i32;
    let width = right - left;
    let height = bottom - top;
    (width > 0 && height > 0).then_some((left, framebuffer.1 as i32 - bottom, width, height))
}

/// OpenGL ES 3 shader transforming real mesh vertices on the GPU.
const MESH_VERTEX_SHADER: &str = r#"#version 300 es
precision highp float;
layout(location = 0) in vec3 inPosition;
layout(location = 1) in vec3 inNormal;
layout(location = 2) in vec2 inUv;
uniform mat4 uPvm;
uniform mat4 uViewModel;
out vec3 normal;
out vec3 originalNormal;
out vec2 uv;
void main() {
    gl_Position = uPvm * vec4(inPosition, 1.0);
    normal = normalize((uViewModel * vec4(inNormal, 0.0)).xyz);
    originalNormal = inNormal;
    uv = inUv;
}
"#;

/// OpenGL ES 3 shader providing the demo's simple directional material.
const MESH_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
in vec3 normal;
in vec3 originalNormal;
in vec2 uv;
out vec4 outColor;
void main() {
    vec3 light = normalize(vec3(0.6, 0.4, 1.0));
    float intensity = max(dot(light, normalize(normal)), 0.0);
    vec3 base = originalNormal * 0.45 + vec3(0.5) + vec3(uv, 0.0) * 0.1;
    outColor = vec4(base * intensity + vec3(0.1), 1.0);
}
"#;
