//! OpenGL ES 3 / WebGL 2 implementation of the reusable mesh renderer.

use std::rc::Rc;

use glow::HasContext;
use neocogi_render_core::{RenderItemArgs, prelude::Recti};
use neocogi_render_glow::{GlApplicationRenderContext, GlApplicationRenderer, create_program};

use crate::{MeshBuffers, MeshCommand, MeshSubmission, MeshVertex};

/// Application renderer owning the mesh renderer's GL program and mesh buffers.
pub struct MeshRenderer {
    /// Lazily created resources, initialized only if a frame actually contains a mesh command.
    resources: Option<GlMeshResources>,
}

impl MeshRenderer {
    /// Creates an empty renderer whose GL objects will be allocated on its first mesh draw.
    pub fn new() -> Self {
        Self { resources: None }
    }

    /// Creates persistent GL resources against the active backend context when first needed.
    fn resources(&mut self, gl: &Rc<glow::Context>) -> Result<&mut GlMeshResources, String> {
        if self.resources.is_none() {
            self.resources = Some(GlMeshResources::new(gl.clone())?);
        }
        Ok(self.resources.as_mut().expect("mesh resources were initialized above"))
    }
}

impl Default for MeshRenderer {
    /// Uses the same lazy construction policy as [`MeshRenderer::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl GlApplicationRenderer for MeshRenderer {
    type Command = MeshCommand;

    /// Uploads and draws the command's mesh through native GL buffers and a depth-tested pipeline.
    fn render(&mut self, context: GlApplicationRenderContext<'_>, command: Self::Command) -> Result<(), String> {
        match command {
            MeshCommand::DrawMesh(submission) => self.resources(context.gl)?.draw(context.framebuffer_size, context.args, &submission),
        }
    }
}

/// Persistent native handles used by the GL mesh path.
struct GlMeshResources {
    /// Shared context retained so Drop can delete every application-owned native handle.
    gl: Rc<glow::Context>,
    /// Linked vertex/fragment program implementing transform and normal shading.
    program: glow::Program,
    /// Vertex array recording the scene mesh's attribute and buffer bindings.
    vertex_array: glow::VertexArray,
    /// Immutable GPU uploads indexed by retained CPU allocation identity.
    meshes: Vec<GlMesh>,
}

impl GlMeshResources {
    /// Allocates the application pipeline; immutable buffers are lazy per mesh.
    fn new(gl: Rc<glow::Context>) -> Result<Self, String> {
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
        Ok(Self {
            gl,
            program,
            vertex_array,
            meshes: Vec::new(),
        })
    }

    /// Binds cached geometry and records an indexed draw in the widget's viewport and clip.
    fn draw(&mut self, framebuffer_size: (u32, u32), args: RenderItemArgs, submission: &MeshSubmission) -> Result<(), String> {
        if submission.mesh.is_empty() {
            return Ok(());
        }
        let Some(viewport) = gl_rect(args.content_area, args.dimensions.width, args.dimensions.height, framebuffer_size)? else {
            return Ok(());
        };
        let Some(scissor) = gl_rect(args.view, args.dimensions.width, args.dimensions.height, framebuffer_size)? else {
            return Ok(());
        };

        self.meshes.retain(|mesh| mesh.source.externally_retained());
        let mesh_index = match self.meshes.iter().position(|mesh| mesh.source.same_storage(&submission.mesh)) {
            Some(index) => index,
            None => {
                self.meshes.push(GlMesh::new(self.gl.clone(), &submission.mesh)?);
                self.meshes.len() - 1
            }
        };
        let mesh = &self.meshes[mesh_index];
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
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(mesh.vertex_buffer));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(mesh.index_buffer));

            // MeshVertex and its rs-math3d vectors are repr(C), so these offsets match their float fields.
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, std::mem::size_of::<MeshVertex>() as i32, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, std::mem::size_of::<MeshVertex>() as i32, 12);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 2, glow::FLOAT, false, std::mem::size_of::<MeshVertex>() as i32, 24);

            set_matrix(gl, self.program, "uPvm", &submission.pvm)?;
            set_matrix(gl, self.program, "uViewModel", &submission.view_model)?;
            gl.draw_elements(glow::TRIANGLES, mesh.index_count, glow::UNSIGNED_INT, 0);

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
            self.gl.delete_vertex_array(self.vertex_array);
            self.gl.delete_program(self.program);
        }
    }
}

/// One immutable GL upload retained while its source mesh remains in a scene.
struct GlMesh {
    /// Keeps the GL context alive through deletion of these buffer handles.
    gl: Rc<glow::Context>,
    /// Source allocation identity, without copying vertex or index contents.
    source: MeshBuffers,
    /// Static vertex buffer uploaded once.
    vertex_buffer: glow::Buffer,
    /// Static index buffer uploaded once.
    index_buffer: glow::Buffer,
    /// Number of indices supplied to the indexed draw.
    index_count: i32,
}

impl GlMesh {
    /// Allocates and uploads immutable geometry, rolling back partial handle creation.
    fn new(gl: Rc<glow::Context>, source: &MeshBuffers) -> Result<Self, String> {
        let vertex_buffer = unsafe { gl.create_buffer() }.map_err(|error| format!("create mesh vertex buffer failed: {error}"))?;
        let index_buffer = match unsafe { gl.create_buffer() } {
            Ok(buffer) => buffer,
            Err(error) => {
                unsafe {
                    gl.delete_buffer(vertex_buffer);
                }
                return Err(format!("create mesh index buffer failed: {error}"));
            }
        };
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(source.vertices()), glow::STATIC_DRAW);
            // Upload through ARRAY_BUFFER so preparation does not mutate the
            // renderer's vertex-array element binding before a draw begins.
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(index_buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes(source.indices()), glow::STATIC_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
        Ok(Self {
            gl,
            source: source.clone(),
            vertex_buffer,
            index_buffer,
            index_count: source.indices().len() as i32,
        })
    }
}

impl Drop for GlMesh {
    /// Releases GPU buffers after the renderer no longer retains their source.
    fn drop(&mut self) {
        unsafe {
            self.gl.delete_buffer(self.index_buffer);
            self.gl.delete_buffer(self.vertex_buffer);
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
/// Empty logical intersections produce no draw. Unrepresentable framebuffer extents report
/// a capability error before native viewport/scissor calls, rather than wrapping or skipping.
/// This preserves the existing bounded viewport policy; full-content projection is separate.
fn gl_rect(rect: Recti, logical_width: i32, logical_height: i32, framebuffer: (u32, u32)) -> Result<Option<(i32, i32, i32, i32)>, String> {
    if logical_width <= 0 || logical_height <= 0 || rect.width <= 0 || rect.height <= 0 {
        return Ok(None);
    }
    let framebuffer_width = i32::try_from(framebuffer.0).map_err(|_| "mesh framebuffer width exceeds GL's signed extent".to_owned())?;
    let framebuffer_height = i32::try_from(framebuffer.1).map_err(|_| "mesh framebuffer height exceeds GL's signed extent".to_owned())?;
    let (left, right) = clipped_pixel_span(rect.x, rect.width, logical_width, framebuffer_width);
    let (top, bottom) = clipped_pixel_span(rect.y, rect.height, logical_height, framebuffer_height);
    let width = right - left;
    let height = bottom - top;
    Ok((width > 0 && height > 0).then_some((left, framebuffer_height - bottom, width, height)))
}

/// Intersects one positive logical extent, then scales its edges outward into drawable pixels.
/// Callers validate positive logical/rectangle extents and a nonnegative signed pixel extent.
/// Widening before addition preserves the importer's entire i32 rectangle domain. After
/// clipping, each product and the ceil adjustment are below 2^62, and final edges fit pixels.
fn clipped_pixel_span(origin: i32, extent: i32, logical: i32, pixels: i32) -> (i32, i32) {
    let limit = i64::from(logical);
    let pixels = i64::from(pixels);
    let near = i64::from(origin).clamp(0, limit);
    let far = (i64::from(origin) + i64::from(extent)).clamp(0, limit);
    let near_pixel = near * pixels / limit;
    let far_pixel = (far * pixels + limit - 1) / limit;
    (near_pixel as i32, far_pixel as i32)
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    /// Both origin-plus-extent axes retain mathematical edges beyond i32::MAX.
    #[test]
    fn wide_content_edges_remain_visible() {
        for (rect, expected) in [
            (Recti::new(1, 0, i32::MAX, 32), (1, 0, 31, 32)),
            (Recti::new(0, 1, 32, i32::MAX), (0, 0, 32, 31)),
            (Recti::new(1, 1, i32::MAX, i32::MAX), (1, 0, 31, 31)),
            (Recti::new(i32::MIN + 17, 0, i32::MAX, 32), (0, 0, 16, 32)),
            (Recti::new(0, i32::MIN + 17, 32, i32::MAX), (0, 16, 32, 16)),
        ] {
            assert_eq!(gl_rect(rect, 32, 32, (32, 32)).unwrap(), Some(expected));
        }
    }

    /// Fractional scaling rounds outward, including exact edges at precision-sensitive magnitudes.
    #[test]
    fn scaling_has_exact_integer_edges() {
        assert_eq!(gl_rect(Recti::new(1, 1, 1, 1), 3, 3, (5, 5)).unwrap(), Some((1, 1, 3, 3)));
        let large = 16_777_217;
        assert_eq!(
            gl_rect(Recti::new(large, 0, 1, 1), i32::MAX, 1, (i32::MAX as u32, 1)).unwrap(),
            Some((large, 0, 1, 1))
        );
        assert_eq!(clipped_pixel_span(1, i32::MAX, i32::MAX, i32::MAX), (1, i32::MAX));
        assert_eq!(clipped_pixel_span(i32::MAX - 1, 1, i32::MAX, i32::MAX), (i32::MAX - 1, i32::MAX));
    }

    /// Scaled edges tightly enclose each intersection across extreme coordinates and scale ratios.
    #[test]
    fn extreme_coordinate_grid_has_tight_outward_rounding() {
        for origin in [i32::MIN, i32::MIN + 17, -32, -1, 0, 1, 31, i32::MAX - 1, i32::MAX] {
            for extent in [1, 2, 32, i32::MAX] {
                for logical in [1, 3, 32, 16_777_217, i32::MAX] {
                    for pixels in [0, 1, 5, 32, i32::MAX] {
                        let (near, far) = clipped_pixel_span(origin, extent, logical, pixels);
                        assert!(0 <= near && near <= far && far <= pixels);
                        // i128 residual inequalities verify exact floor/ceil independently
                        // of the implementation's quotient and ceil-adjustment expressions.
                        let divisor = i128::from(logical);
                        let exact_near = i128::from(origin).clamp(0, divisor) * i128::from(pixels);
                        let exact_far = (i128::from(origin) + i128::from(extent)).clamp(0, divisor) * i128::from(pixels);
                        assert!(i128::from(near) * divisor <= exact_near && (i128::from(near) + 1) * divisor > exact_near);
                        assert!(i128::from(far) * divisor >= exact_far && (i128::from(far) - 1) * divisor < exact_far);
                    }
                }
            }
        }
    }

    /// Only empty/invisible rectangles skip; unsupported signed GL extents return an error.
    #[test]
    fn empty_intersections_and_framebuffer_limits_are_explicit() {
        for rect in [
            Recti::new(i32::MIN, 0, i32::MAX, 32),
            Recti::new(32, 0, i32::MAX, 32),
            Recti::new(0, 32, 32, 1),
            Recti::new(0, 0, 0, 32),
        ] {
            assert_eq!(gl_rect(rect, 32, 32, (32, 32)).unwrap(), None);
        }
        assert_eq!(gl_rect(Recti::new(0, 0, 32, 32), 0, 32, (32, 32)).unwrap(), None);
        assert_eq!(gl_rect(Recti::new(0, 0, 32, 32), 32, 32, (0, 32)).unwrap(), None);
        assert!(gl_rect(Recti::new(0, 0, 32, 32), 32, 32, (u32::MAX, 32)).unwrap_err().contains("width"));
        assert!(gl_rect(Recti::new(0, 0, 32, 32), 32, 32, (32, u32::MAX)).unwrap_err().contains("height"));
    }
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

/// OpenGL ES 3 shader providing the mesh renderer's simple directional material.
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
