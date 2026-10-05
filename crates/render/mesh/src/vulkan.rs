//! Vulkan implementation of the reusable mesh renderer.

use std::{io::Cursor, mem, ptr, rc::Rc, slice};

use ash::{util::read_spv, vk};
use neocogi_render_core::prelude::Recti;
use neocogi_render_vulkan::{VulkanApplicationPrepareContext, VulkanApplicationRenderContext, VulkanApplicationRenderer, VulkanApplicationResult};
use rs_math3d::Mat4f;

use crate::{MeshCommand, MeshBuffers, MeshVertex};

#[cfg(test)]
#[path = "vulkan_tests.rs"]
mod tests;

/// Application renderer owning the reusable Vulkan mesh resources.
pub struct MeshRenderer {
    /// Pipeline compatible with the current backend render-pass generation.
    pipeline: Option<MeshPipeline>,
    /// One immutable GPU upload per retained CPU mesh, independent of draw order and visibility.
    meshes: Vec<Rc<PreparedMesh>>,
    /// Each slot retains its submitted meshes until the backend has waited for that slot's fence.
    prepared_frames: Vec<Vec<Option<Rc<PreparedMesh>>>>,
    /// Frame slot selected by the most recent preparation phase.
    active_frame: usize,
    /// Swapchain generation baked into `pipeline`.
    pipeline_generation: Option<u64>,
}

impl MeshRenderer {
    /// Creates an empty renderer whose Vulkan objects are allocated on first use.
    pub fn new() -> Self {
        Self {
            pipeline: None,
            meshes: Vec::new(),
            prepared_frames: Vec::new(),
            active_frame: 0,
            pipeline_generation: None,
        }
    }

    /// Resolves shared uploads and retires only references belonging to a completed frame slot.
    fn prepare_meshes(
        &mut self,
        device: &ash::Device,
        memory_properties: &vk::PhysicalDeviceMemoryProperties,
        frame_index: usize,
        frames_in_flight: usize,
        commands: &[&MeshCommand],
    ) -> VulkanApplicationResult<()> {
        if frame_index >= frames_in_flight {
            return Err(format!("invalid Vulkan application frame slot {frame_index}"));
        }
        if self.prepared_frames.len() != frames_in_flight {
            // The backend fixes the slot count for the device lifetime. Initial construction has
            // no submitted work; a backend replacing that configuration must first finish it all.
            self.prepared_frames = (0..frames_in_flight).map(|_| Vec::new()).collect();
        }
        self.active_frame = frame_index;
        let frame = &mut self.prepared_frames[frame_index];
        // Only this slot's fence has completed. Other slots keep strong GPU references even when
        // a scene replaces or releases its CPU mesh and the cache stops retaining the old upload.
        frame.clear();
        self.meshes.retain(|mesh| mesh.source_is_retained());
        frame.reserve(commands.len());
        for command in commands {
            let MeshCommand::DrawMesh(submission) = command;
            let prepared = match &submission.mesh {
                mesh if mesh.is_empty() => None,
                mesh => {
                    // Immutable allocation identity is enough: transforms never modify these
                    // arrays. Hold the source with the upload so its address cannot be recycled.
                    let prepared = match self.meshes.iter().find(|prepared| prepared.matches(mesh)) {
                        Some(prepared) => Rc::clone(prepared),
                        None => {
                            let prepared = Rc::new(PreparedMesh::new(device, memory_properties, mesh)?);
                            self.meshes.push(Rc::clone(&prepared));
                            prepared
                        }
                    };
                    Some(prepared)
                }
            };
            frame.push(prepared);
        }
        Ok(())
    }
}

impl Default for MeshRenderer {
    /// Uses the same lazy native-resource construction as [`MeshRenderer::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl VulkanApplicationRenderer for MeshRenderer {
    type Command = MeshCommand;

    /// Rebuilds render-pass-dependent state and resolves the active slot's shared mesh uploads.
    fn prepare(&mut self, context: VulkanApplicationPrepareContext<'_>, commands: &[&Self::Command]) -> VulkanApplicationResult<()> {
        if self.pipeline_generation != Some(context.swapchain_generation) {
            // The backend waits for device idle before changing render-pass generations, so the
            // obsolete compatible pipeline can be destroyed immediately at this boundary.
            self.pipeline = None;
            self.pipeline = Some(MeshPipeline::new(context.device, context.render_pass)?);
            self.pipeline_generation = Some(context.swapchain_generation);
        }
        self.prepare_meshes(
            context.device,
            &context.memory_properties,
            context.frame_index,
            context.frames_in_flight,
            commands,
        )
    }

    /// Clears item-local depth and records one indexed mesh draw inside the backend-owned pass.
    /// Color stays in painter order, while triangles inside this item still test against each other.
    fn render(
        &self,
        context: VulkanApplicationRenderContext<'_>,
        args: neocogi_render_core::RenderItemArgs,
        command_index: usize,
        command: &Self::Command,
    ) -> VulkanApplicationResult<()> {
        let MeshCommand::DrawMesh(submission) = command;
        let Some(prepared) = self
            .prepared_frames
            .get(self.active_frame)
            .and_then(|frame| frame.get(command_index))
            .and_then(Option::as_ref)
        else {
            return Ok(());
        };
        let pipeline = self.pipeline.as_ref().ok_or("Vulkan mesh pipeline was not prepared")?;
        let Some(scissor_rect) = surface_rect(args.view, (context.logical_width, context.logical_height), context.extent) else {
            return Ok(());
        };
        let viewport = vk::Viewport {
            // One physical rectangle fits the entire scene and bounds its rasterization.
            x: scissor_rect.x as f32,
            y: scissor_rect.y as f32,
            width: scissor_rect.width as f32,
            height: scissor_rect.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let scissor = vk::Rect2D {
            offset: vk::Offset2D { x: scissor_rect.x, y: scissor_rect.y },
            extent: vk::Extent2D {
                width: scissor_rect.width as u32,
                height: scissor_rect.height as u32,
            },
        };
        let mut matrices = [0.0f32; 32];
        let pvm = Mat4f::mul_matrix_matrix(&opengl_to_vulkan_clip_matrix(), &submission.pvm);
        matrices[..16].copy_from_slice(matrix_values(&pvm));
        matrices[16..].copy_from_slice(matrix_values(&submission.view_model));

        // Independent painter-ordered scenes never inherit an earlier item's depth. The clear
        // touches depth only, inside the same bounded physical rectangle used for this draw.
        let clear = vk::ClearAttachment::default()
            .aspect_mask(vk::ImageAspectFlags::DEPTH)
            .clear_value(vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 } });
        let clear_rect = vk::ClearRect::default().rect(scissor).base_array_layer(0).layer_count(1);
        // SAFETY: The backend lends an active compatible graphics pass with a depth attachment.
        // Its live command buffer, prepared buffers and pipeline outlive recording/submission;
        // the clear rectangle is clipped to the framebuffer and addresses its sole array layer.
        unsafe {
            context.device.cmd_clear_attachments(context.command_buffer, &[clear], &[clear_rect]);
            context
                .device
                .cmd_bind_pipeline(context.command_buffer, vk::PipelineBindPoint::GRAPHICS, pipeline.pipeline);
            context
                .device
                .cmd_bind_vertex_buffers(context.command_buffer, 0, &[prepared.vertex.buffer], &[0]);
            context
                .device
                .cmd_bind_index_buffer(context.command_buffer, prepared.index.buffer, 0, vk::IndexType::UINT32);
            context.device.cmd_set_viewport(context.command_buffer, 0, &[viewport]);
            context.device.cmd_set_scissor(context.command_buffer, 0, &[scissor]);
            context
                .device
                .cmd_push_constants(context.command_buffer, pipeline.layout, vk::ShaderStageFlags::VERTEX, 0, bytes(&matrices));
            context.device.cmd_draw_indexed(context.command_buffer, prepared.index_count, 1, 0, 0, 0);
        }
        Ok(())
    }
}

/// Render-pass-compatible Vulkan pipeline owned entirely by the mesh renderer.
struct MeshPipeline {
    /// Device dispatch retained so Drop can release native pipeline objects.
    device: ash::Device,
    /// Pipeline layout declaring two vertex-stage matrix push constants.
    layout: vk::PipelineLayout,
    /// Depth-tested application graphics pipeline.
    pipeline: vk::Pipeline,
}

impl MeshPipeline {
    /// Creates shader modules, pipeline layout, and graphics pipeline with explicit cleanup.
    fn new(device: &ash::Device, render_pass: vk::RenderPass) -> VulkanApplicationResult<Self> {
        let push_range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size((mem::size_of::<f32>() * 32) as u32);
        let layout_info = vk::PipelineLayoutCreateInfo::default().push_constant_ranges(slice::from_ref(&push_range));
        let layout =
            unsafe { device.create_pipeline_layout(&layout_info, None) }.map_err(|error| format!("create scene mesh pipeline layout failed: {error:?}"))?;

        let vertex_module = match create_shader_module(device, MESH_VERTEX_SPV) {
            Ok(module) => module,
            Err(error) => {
                unsafe { device.destroy_pipeline_layout(layout, None) };
                return Err(error);
            }
        };
        let fragment_module = match create_shader_module(device, MESH_FRAGMENT_SPV) {
            Ok(module) => module,
            Err(error) => {
                unsafe {
                    device.destroy_shader_module(vertex_module, None);
                    device.destroy_pipeline_layout(layout, None);
                }
                return Err(error);
            }
        };
        let entry = c"main";
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vertex_module)
                .name(entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fragment_module)
                .name(entry),
        ];
        let binding = vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(mem::size_of::<MeshVertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX);
        let attributes = [
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(1)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(12),
            vk::VertexInputAttributeDescription::default()
                .binding(0)
                .location(2)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(24),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(slice::from_ref(&binding))
            .vertex_attribute_descriptions(&attributes);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport_state = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let rasterization = vk::PipelineRasterizationStateCreateInfo::default()
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);
        let color_attachment = vk::PipelineColorBlendAttachmentState::default()
            .color_write_mask(vk::ColorComponentFlags::R | vk::ColorComponentFlags::G | vk::ColorComponentFlags::B | vk::ColorComponentFlags::A)
            .blend_enable(false);
        let color_blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(slice::from_ref(&color_attachment));
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterization)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic_state)
            .layout(layout)
            .render_pass(render_pass)
            .subpass(0);
        let pipeline_result = unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), slice::from_ref(&pipeline_info), None) };
        unsafe {
            device.destroy_shader_module(fragment_module, None);
            device.destroy_shader_module(vertex_module, None);
        }
        let pipeline = match pipeline_result {
            Ok(mut pipelines) => pipelines.remove(0),
            Err((_, error)) => {
                unsafe { device.destroy_pipeline_layout(layout, None) };
                return Err(format!("create scene mesh graphics pipeline failed: {error:?}"));
            }
        };
        Ok(Self { device: device.clone(), layout, pipeline })
    }
}

impl Drop for MeshPipeline {
    /// Releases the native graphics pipeline before its layout.
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
        }
    }
}

/// One immutable GPU upload shared by retained scenes and every frame that draws their mesh.
struct PreparedMesh {
    /// Retains the allocation identity used for lookup; other CPU owners keep this upload cached.
    source: MeshBuffers,
    /// Vertex allocation containing application-owned [`MeshVertex`] data.
    vertex: VulkanBuffer,
    /// Index allocation containing the triangle-list u32 data.
    index: VulkanBuffer,
    /// Number of indices issued by the corresponding draw.
    index_count: u32,
}

impl PreparedMesh {
    /// Creates and fills both buffers once; subsequent frames only bind these immutable objects.
    fn new(device: &ash::Device, memory_properties: &vk::PhysicalDeviceMemoryProperties, mesh: &MeshBuffers) -> VulkanApplicationResult<Self> {
        let vertex = VulkanBuffer::new(device, memory_properties, bytes(mesh.vertices()), vk::BufferUsageFlags::VERTEX_BUFFER)?;
        let index = VulkanBuffer::new(device, memory_properties, bytes(mesh.indices()), vk::BufferUsageFlags::INDEX_BUFFER)?;
        Ok(Self {
            source: mesh.clone(),
            vertex,
            index,
            index_count: mesh.indices().len() as u32,
        })
    }

    /// Compares both immutable allocations without hashing or reading their potentially large data.
    fn matches(&self, mesh: &MeshBuffers) -> bool {
        self.source.same_storage(mesh)
    }

    /// Keeps hidden meshes cached while a scene or CPU command still owns their source arrays.
    fn source_is_retained(&self) -> bool {
        // This upload owns exactly one clone. Frame references clone Rc<PreparedMesh>, not its
        // source, so they delay GPU destruction without making retired CPU meshes look live.
        self.source.externally_retained()
    }
}

/// One application-owned Vulkan buffer and its bound device memory.
struct VulkanBuffer {
    /// Device dispatch retained for deterministic Drop cleanup.
    device: ash::Device,
    /// Native buffer bound at offset zero into `memory`.
    buffer: vk::Buffer,
    /// Host-visible coherent allocation containing the uploaded bytes.
    memory: vk::DeviceMemory,
}

impl VulkanBuffer {
    /// Allocates, binds, maps, and fills one immutable application buffer.
    fn new(
        device: &ash::Device,
        memory_properties: &vk::PhysicalDeviceMemoryProperties,
        data: &[u8],
        usage: vk::BufferUsageFlags,
    ) -> VulkanApplicationResult<Self> {
        let buffer_info = vk::BufferCreateInfo::default()
            .size(data.len() as u64)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { device.create_buffer(&buffer_info, None) }.map_err(|error| format!("create scene mesh buffer failed: {error:?}"))?;
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let memory_type = match find_memory_type(
            memory_properties,
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        ) {
            Some(index) => index,
            None => {
                unsafe { device.destroy_buffer(buffer, None) };
                return Err("no coherent host-visible Vulkan memory type is available for scene mesh buffers".into());
            }
        };
        let allocation_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type);
        let memory = match unsafe { device.allocate_memory(&allocation_info, None) } {
            Ok(memory) => memory,
            Err(error) => {
                unsafe { device.destroy_buffer(buffer, None) };
                return Err(format!("allocate scene mesh buffer memory failed: {error:?}"));
            }
        };
        if let Err(error) = unsafe { device.bind_buffer_memory(buffer, memory, 0) } {
            unsafe {
                device.free_memory(memory, None);
                device.destroy_buffer(buffer, None);
            }
            return Err(format!("bind scene mesh buffer memory failed: {error:?}"));
        }
        let mapped = match unsafe { device.map_memory(memory, 0, data.len() as u64, vk::MemoryMapFlags::empty()) } {
            Ok(mapped) => mapped,
            Err(error) => {
                unsafe {
                    device.free_memory(memory, None);
                    device.destroy_buffer(buffer, None);
                }
                return Err(format!("map scene mesh buffer memory failed: {error:?}"));
            }
        };
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), mapped.cast::<u8>(), data.len());
            device.unmap_memory(memory);
        }
        Ok(Self { device: device.clone(), buffer, memory })
    }
}

impl Drop for VulkanBuffer {
    /// Destroys the buffer before freeing its bound memory allocation.
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

/// Finds a memory type accepted by the buffer and containing every requested property.
fn find_memory_type(properties: &vk::PhysicalDeviceMemoryProperties, accepted_bits: u32, required: vk::MemoryPropertyFlags) -> Option<u32> {
    (0..properties.memory_type_count).find(|index| {
        let accepted = accepted_bits & (1 << index) != 0;
        let flags = properties.memory_types[*index as usize].property_flags;
        accepted && flags.contains(required)
    })
}

/// Creates one shader module from checked SPIR-V bytes.
fn create_shader_module(device: &ash::Device, code: &[u8]) -> VulkanApplicationResult<vk::ShaderModule> {
    let words = read_spv(&mut Cursor::new(code)).map_err(|error| format!("read scene mesh SPIR-V failed: {error:?}"))?;
    let info = vk::ShaderModuleCreateInfo::default().code(&words);
    unsafe { device.create_shader_module(&info, None) }.map_err(|error| format!("create scene mesh shader module failed: {error:?}"))
}

/// Converts a logical rectangle into a clamped top-left Vulkan surface rectangle.
fn surface_rect(rect: Recti, logical: (u32, u32), extent: vk::Extent2D) -> Option<Recti> {
    if rect.width <= 0 || rect.height <= 0 {
        return None;
    }
    let scale_x = extent.width as f32 / logical.0.max(1) as f32;
    let scale_y = extent.height as f32 / logical.1.max(1) as f32;
    let scaled = Recti::new(
        (rect.x as f32 * scale_x).round() as i32,
        (rect.y as f32 * scale_y).round() as i32,
        (rect.width as f32 * scale_x).round() as i32,
        (rect.height as f32 * scale_y).round() as i32,
    );
    let left = scaled.x.clamp(0, extent.width as i32);
    let top = scaled.y.clamp(0, extent.height as i32);
    let right = scaled.x.saturating_add(scaled.width).clamp(0, extent.width as i32);
    let bottom = scaled.y.saturating_add(scaled.height).clamp(0, extent.height as i32);
    (right > left && bottom > top).then_some(Recti::new(left, top, right - left, bottom - top))
}

/// Copies one rs-math3d matrix into a plain float slice.
fn matrix_values(matrix: &Mat4f) -> &[f32] {
    // SAFETY: Mat4f stores four contiguous f32 columns and remains borrowed for the result.
    unsafe { slice::from_raw_parts(matrix.col.as_ptr().cast::<f32>(), 16) }
}

/// Reinterprets plain GPU input values as bytes for upload or push constants.
fn bytes<T>(values: &[T]) -> &[u8] {
    // SAFETY: the output shares the input lifetime and spans its exact contiguous byte size.
    unsafe { slice::from_raw_parts(values.as_ptr().cast::<u8>(), mem::size_of_val(values)) }
}

/// Converts OpenGL-style Y/Z clip coordinates into Vulkan's clip convention.
fn opengl_to_vulkan_clip_matrix() -> Mat4f {
    Mat4f::new(
        1.0, 0.0, 0.0, 0.0, //
        0.0, -1.0, 0.0, 0.0, //
        0.0, 0.0, 0.5, 0.0, //
        0.0, 0.0, 0.5, 1.0, //
    )
}

/// Mesh-renderer-owned vertex SPIR-V embedded into the Vulkan application renderer.
const MESH_VERTEX_SPV: &[u8] = include_bytes!("../shaders/mesh.vert.spv");
/// Mesh-renderer-owned fragment SPIR-V embedded into the Vulkan application renderer.
const MESH_FRAGMENT_SPV: &[u8] = include_bytes!("../shaders/mesh.frag.spv");
