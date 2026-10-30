//! Native realization of the common immutable graphics vocabulary, with no client callbacks.
use glow::HasContext;
use neocogi_render_gpu::{abi::*, command::*, record::Block, resource::*, scope::physical_rect};
use std::{collections::HashMap, rc::Rc};

/// Device-local caches contain device-free CPU versions, never a domain or public COM owner.
pub(crate) struct Executor {
    /// The owning context remains current throughout preparation, execution and deletion.
    gl: Rc<glow::Context>,
    /// Private layout state prevents generic attributes from touching an unrelated VAO.
    array: glow::VertexArray,
    /// Immutable role-specific vertex/index uploads.
    buffers: HashMap<u64, Rc<Buffer>>,
    /// Immutable sampled pixel uploads.
    textures: HashMap<u64, Rc<Texture>>,
    /// Independent sampling state.
    samplers: HashMap<u64, Rc<Sampler>>,
    /// Linked variants distinguish direct presentation and top-left offscreen image storage.
    pipelines: HashMap<(u64, bool), Rc<Pipeline>>,
    /// Copied uniform payload identity is pinned by each entry, preventing address reuse.
    uniforms: HashMap<usize, Rc<Uniform>>,
    /// Immediate GL work retained even if its public frame is abandoned after issuing commands.
    pending: Vec<PreparedBlock>,
    /// Fences own only native snapshots and device-free accounting, never client domain authority.
    submitted: Vec<Submitted>,
    /// Preparatory uploads can be in flight even when the target frame is never acquired.
    uploads: Vec<Upload>,
    /// Original command metadata follows preparatory native work through completion too.
    upload_capacity: Vec<CommandCharge>,
}

macro_rules! native {
    ($name:ident, $source:ty, $handle:ty, $delete:ident) => {
        /// Native allocation and exactly one retained immutable CPU version.
        struct $name {
            /// Deletion context, without the renderer cache owner or a client domain.
            gl: Rc<glow::Context>,
            /// Detects retirement after public/current/recorded ownership ends.
            source: Rc<$source>,
            /// Exactly one native object of this class.
            raw: $handle,
        }
        impl Drop for $name {
            /// GL defers physical reclamation while previously issued GPU work still uses it.
            fn drop(&mut self) {
                // SAFETY: The renderer/plugin selects the owning context before resource retirement.
                unsafe { self.gl.$delete(self.raw) }
            }
        }
    };
}
native!(Buffer, BufferVersion, glow::Buffer, delete_buffer);
/// Shared native image and device-free quota receipt; imported COM leases stop at preparation.
struct Texture {
    /// Image lifetime is independent of the public ordinary-image allocation and its device facade.
    image: Rc<super::Image>,
    /// Passive private-version expiry retires cache ownership without refunding in-flight storage.
    source: TextureReceipt,
}
native!(Sampler, SamplerVersion, glow::Sampler, delete_sampler);
native!(Uniform, UniformData, glow::Buffer, delete_buffer);

/// Linked target-aware program with optional fragment-coordinate normalization.
struct Pipeline {
    /// Current context retained through deterministic program destruction.
    gl: Rc<glow::Context>,
    /// Exactly one device-free logical pipeline version.
    source: Rc<PipelineVersion>,
    /// Owned linked graphics program.
    raw: glow::Program,
    /// Direct window rendering converts GL's bottom-up fragment position to top-left WGSL.
    target_height: Option<glow::UniformLocation>,
}
impl Drop for Pipeline {
    /// Submitted GL work retains native driver ownership after deleting the public name.
    fn drop(&mut self) {
        // SAFETY: The plugin selects this context before retiring native resources.
        unsafe { self.gl.delete_program(self.raw) }
    }
}

/// Admitted command data contains no domain, device, COM callback, or mutable client state.
#[derive(Clone)]
pub struct PreparedBlock {
    /// Shared concrete data permits completion retention without copying command payloads.
    data: Rc<PreparedData>,
}
/// Native lowering retains the original capacity charge until actual GPU completion.
struct PreparedData {
    /// Painter-order operations with all native resources already realized.
    commands: Vec<PreparedCommand>,
    /// Device-free receipts prevent early refunds when the original block is released.
    _capacity: Vec<CommandCharge>,
}
/// One fence marks completion of every preceding native use in its bounded batch.
struct Submitted {
    /// Fence belongs to this executor's current context.
    fence: glow::Fence,
    /// Exact native allocations and logical byte/capacity receipts used by these commands.
    _blocks: Vec<PreparedBlock>,
    /// Device-free CPU versions retain accounting after partial or abandoned preparation.
    _uploads: Vec<Upload>,
    /// Shared original command receipts bound preparation metadata until completion.
    _capacity: Vec<CommandCharge>,
}

/// Upload retirement contains no domain/device-facade or public COM owner.
enum Upload {
    /// Vertex/index storage keeps its logical bytes reserved during driver initialization.
    Buffer { _source: Rc<BufferVersion> },
    /// Image initialization keeps the exact admitted pixel version charged.
    Texture { _source: Rc<TextureVersion> },
    /// Uniform initialization retains copied pending bytes.
    Uniform { _source: Rc<UniformData> },
}
/// Complete native state for a single direct draw.
struct PreparedDraw {
    /// Linked fixed-state variant and its device-free logical descriptor.
    pipeline: Rc<Pipeline>,
    /// Slot, native version and byte base for each consumed stream.
    vertices: Vec<(u32, Rc<Buffer>, u64)>,
    /// Native index version and its explicit element interpretation.
    index: Option<(Rc<Buffer>, GpuIndexFormat, u64)>,
    /// Flattened texture unit and immutable image/sampler pair.
    textures: Vec<(u32, Rc<Texture>, Rc<Sampler>)>,
    /// Flattened UBO binding and exact copied version.
    uniforms: Vec<(u32, Rc<Uniform>)>,
    /// Validated direct triangle range.
    range: DrawRange,
    /// Host-bounded logical viewport mapping.
    viewport: Rect,
    /// Host-bounded logical raster write region.
    clip: Rect,
}
/// Closed backend command set; clear operations ignore client pipeline masks and blending.
enum PreparedCommand {
    /// Native resources plus copied pointer-free direct draw metadata.
    Draw(PreparedDraw),
    /// Exact packed RGBA replacement within a checked clip.
    Color(u32, Rect),
    /// Exact normalized depth replacement within a checked clip.
    Depth(f32, Rect),
}

impl Executor {
    /// Allocates only private layout state; all resource realization is lazy and fallible.
    pub(crate) fn new(gl: Rc<glow::Context>) -> Result<Self, String> {
        // SAFETY: Called with the owning live GL ES 3/WebGL 2 context current.
        let array = unsafe { gl.create_vertex_array()? };
        Ok(Self {
            gl,
            array,
            buffers: HashMap::new(),
            textures: HashMap::new(),
            samplers: HashMap::new(),
            pipelines: HashMap::new(),
            uniforms: HashMap::new(),
            pending: Vec::new(),
            submitted: Vec::new(),
            uploads: Vec::new(),
            upload_capacity: Vec::new(),
        })
    }

    /// Retires uploads whose sole CPU owner is this cache, before admitting another frame.
    pub(crate) fn collect(&mut self) -> Result<(), String> {
        self.complete()?;
        let mut failure = false;
        self.submitted.retain(|batch| {
            // SAFETY: The plugin selects this context before preparation/collection. A zero
            // timeout observes completion without treating public weak expiry as completion.
            match unsafe { self.gl.client_wait_sync(batch.fence, 0, 0) } {
                glow::ALREADY_SIGNALED | glow::CONDITION_SATISFIED => {
                    unsafe { self.gl.delete_sync(batch.fence) };
                    false
                }
                glow::TIMEOUT_EXPIRED => true,
                _ => {
                    failure = true;
                    true
                }
            }
        });
        self.buffers.retain(|_, entry| Rc::strong_count(&entry.source) > 1);
        self.textures.retain(|_, entry| entry.source.is_alive());
        self.samplers.retain(|_, entry| Rc::strong_count(&entry.source) > 1);
        self.pipelines.retain(|_, entry| Rc::strong_count(&entry.source) > 1);
        self.uniforms.retain(|_, entry| Rc::strong_count(&entry.source) > 1);
        if failure {
            Err("failed to observe generic graphics completion".into())
        } else {
            Ok(())
        }
    }

    /// Seals issued immediate work on explicit completion or before the next preparation.
    /// Abandonment keeps pending owners here; it never issues a native call from a frame destructor.
    pub(crate) fn complete(&mut self) -> Result<(), String> {
        if self.pending.is_empty() && self.uploads.is_empty() {
            return Ok(());
        }
        self.submitted.try_reserve(1).map_err(|error| error.to_string())?;
        // SAFETY: The owning context is current. Flushing makes the fence progress without
        // requiring another window frame, which also covers texture-only rendering sessions.
        let fence = unsafe { self.gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0)? };
        unsafe { self.gl.flush() };
        self.submitted.push(Submitted {
            fence,
            _blocks: std::mem::take(&mut self.pending),
            _uploads: std::mem::take(&mut self.uploads),
            _capacity: std::mem::take(&mut self.upload_capacity),
        });
        Ok(())
    }

    /// Creates every driver allocation before the host acquires/clears its native target.
    pub(crate) fn prepare(
        &mut self,
        block: &Block,
        offscreen: bool,
        images: &HashMap<neocogi_render_core::TextureId, Rc<super::Image>>,
    ) -> Result<PreparedBlock, String> {
        self.collect()?;
        let capacity = block.retain_capacity();
        self.upload_capacity.try_reserve(capacity.len()).map_err(|error| error.to_string())?;
        let before = self.uploads.len();
        let result = self.prepare_inner(block, offscreen, images);
        if self.uploads.len() > before {
            self.upload_capacity.extend(capacity);
        }
        result
    }

    /// Partial native preparation has the same upload-completion lifetime as successful blocks.
    fn prepare_inner(
        &mut self,
        block: &Block,
        offscreen: bool,
        images: &HashMap<neocogi_render_core::TextureId, Rc<super::Image>>,
    ) -> Result<PreparedBlock, String> {
        let mut commands = Vec::new();
        commands.try_reserve_exact(block.commands.len()).map_err(|error| error.to_string())?;
        for command in &block.commands {
            commands.push(match command {
                Command::ClearColor { rgba, clip, .. } => PreparedCommand::Color(*rgba, *clip),
                Command::ClearDepth { depth, clip, .. } => PreparedCommand::Depth(*depth, *clip),
                Command::Draw(draw) => PreparedCommand::Draw(self.prepare_draw(draw, offscreen, images)?),
            });
        }
        self.check()?;
        Ok(PreparedBlock {
            data: Rc::new(PreparedData {
                commands,
                _capacity: block.retain_capacity(),
            }),
        })
    }

    /// Realizes exactly the resources consumed by this captured pipeline.
    fn prepare_draw(
        &mut self,
        draw: &Draw,
        offscreen: bool,
        images: &HashMap<neocogi_render_core::TextureId, Rc<super::Image>>,
    ) -> Result<PreparedDraw, String> {
        let pipeline = self.pipeline(draw.pipeline.clone(), offscreen)?;
        let mut vertices = Vec::new();
        for binding in &draw.vertex_buffers {
            vertices.push((binding.slot, self.buffer(binding.buffer.clone())?, binding.offset));
        }
        let index = draw
            .index
            .as_ref()
            .map(|binding| self.buffer(binding.buffer.clone()).map(|buffer| (buffer, binding.format, binding.offset)))
            .transpose()?;
        let mut textures = Vec::new();
        for binding in &draw.textures {
            textures.push((
                unit(binding.stage, binding.slot),
                self.texture(binding.texture.clone(), images)?,
                self.sampler(binding.sampler.clone())?,
            ));
        }
        let mut uniforms = Vec::new();
        for binding in &draw.uniforms {
            uniforms.push((unit(binding.stage, binding.slot), self.uniform(binding.data.clone())?));
        }
        Ok(PreparedDraw {
            pipeline,
            vertices,
            index,
            textures,
            uniforms,
            range: draw.range,
            viewport: draw.viewport,
            clip: draw.clip,
        })
    }

    /// Uploads a complete immutable buffer using its permanent portable role on first binding.
    fn buffer(&mut self, source: Rc<BufferVersion>) -> Result<Rc<Buffer>, String> {
        if let Some(buffer) = self.buffers.get(&source.id) {
            return Ok(buffer.clone());
        }
        self.uploads.try_reserve(1).map_err(|error| error.to_string())?;
        self.uploads.push(Upload::Buffer { _source: source.clone() });
        // SAFETY: Source bytes and role were checked by admission; private VAO owns index binding.
        let buffer = unsafe {
            let raw = self.gl.create_buffer()?;
            let buffer = Rc::new(Buffer { gl: self.gl.clone(), source, raw });
            self.gl.bind_vertex_array(Some(self.array));
            let target = if buffer.source.desc.role == GpuBufferRole::Index {
                glow::ELEMENT_ARRAY_BUFFER
            } else {
                glow::ARRAY_BUFFER
            };
            self.gl.bind_buffer(target, Some(raw));
            self.gl.buffer_data_u8_slice(target, &buffer.source.bytes, glow::STATIC_DRAW);
            self.gl.bind_vertex_array(None);
            self.check()?;
            buffer
        };
        self.buffers.insert(buffer.source.id, buffer.clone());
        Ok(buffer)
    }

    /// Private atlas preparation also runs without a target; uploaded backing stays completion-owned.
    pub(crate) fn prepare_texture(
        &mut self,
        source: Rc<TextureVersion>,
        images: &HashMap<neocogi_render_core::TextureId, Rc<super::Image>>,
    ) -> Result<(), String> {
        self.collect()?;
        self.texture(source, images).map(|_| ())
    }

    /// Upload pixels or capture an authenticated immutable host image without retaining its COM lease.
    fn texture(&mut self, source: Rc<TextureVersion>, images: &HashMap<neocogi_render_core::TextureId, Rc<super::Image>>) -> Result<Rc<Texture>, String> {
        if let Some(texture) = self.textures.get(&source.id) {
            return Ok(texture.clone());
        }
        let id = source.id;
        if let Some(import) = source.imported_id() {
            let image = images.get(&import).ok_or("imported UI image is unavailable on this device")?.clone();
            let texture = Rc::new(Texture { image, source: source.receipt() });
            self.textures.insert(id, texture.clone());
            return Ok(texture);
        }
        let pixels = source.pixels().ok_or("missing texture pixel storage")?;
        self.uploads.try_reserve(1).map_err(|error| error.to_string())?;
        self.uploads.push(Upload::Texture { _source: source.clone() });
        // SAFETY: Source dimensions and complete tightly packed RGBA bytes were admitted before capture.
        let texture = unsafe {
            let image = Rc::new(super::Image {
                gl: self.gl.clone(),
                raw: self.gl.create_texture()?,
            });
            let texture = Rc::new(Texture { image, source: source.receipt() });
            super::GLRenderer::prepare_upload(&self.gl);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture.image.raw));
            let desc = source.desc;
            let format = if desc.format == GpuTextureFormat::Rgba8Srgb {
                glow::SRGB8_ALPHA8
            } else {
                glow::RGBA8
            };
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                format as i32,
                desc.size.width,
                desc.size.height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(pixels)),
            );
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_BASE_LEVEL, 0);
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, 0);
            self.check()?;
            texture
        };
        self.textures.insert(id, texture.clone());
        Ok(texture)
    }

    /// Creates immutable filter/wrap state independent of image storage.
    fn sampler(&mut self, source: Rc<SamplerVersion>) -> Result<Rc<Sampler>, String> {
        if let Some(sampler) = self.samplers.get(&source.id) {
            return Ok(sampler.clone());
        }
        // SAFETY: All descriptor tags were checked before publication.
        let sampler = unsafe {
            let raw = self.gl.create_sampler()?;
            let sampler = Rc::new(Sampler { gl: self.gl.clone(), source, raw });
            let desc = sampler.source.desc;
            self.gl.sampler_parameter_i32(raw, glow::TEXTURE_MIN_FILTER, filter(desc.min_filter) as i32);
            self.gl.sampler_parameter_i32(raw, glow::TEXTURE_MAG_FILTER, filter(desc.mag_filter) as i32);
            self.gl.sampler_parameter_i32(raw, glow::TEXTURE_WRAP_S, address(desc.address_u) as i32);
            self.gl.sampler_parameter_i32(raw, glow::TEXTURE_WRAP_T, address(desc.address_v) as i32);
            self.check()?;
            sampler
        };
        self.samplers.insert(sampler.source.id, sampler.clone());
        Ok(sampler)
    }

    /// Copies portable uniform bytes into a separate immutable allocation padded only at its end.
    fn uniform(&mut self, source: Rc<UniformData>) -> Result<Rc<Uniform>, String> {
        let key = Rc::as_ptr(&source) as usize;
        if let Some(uniform) = self.uniforms.get(&key) {
            return Ok(uniform.clone());
        }
        self.uploads.try_reserve(1).map_err(|error| error.to_string())?;
        self.uploads.push(Upload::Uniform { _source: source.clone() });
        // SAFETY: Reflected std140-compatible layout and exact source length were checked by draw.
        let uniform = unsafe {
            let raw = self.gl.create_buffer()?;
            let uniform = Rc::new(Uniform { gl: self.gl.clone(), source, raw });
            let mut bytes = uniform.source.bytes.clone();
            bytes.resize(bytes.len().next_multiple_of(16), 0);
            self.gl.bind_buffer(glow::UNIFORM_BUFFER, Some(raw));
            self.gl.buffer_data_u8_slice(glow::UNIFORM_BUFFER, &bytes, glow::STATIC_DRAW);
            self.check()?;
            uniform
        };
        self.uniforms.insert(key, uniform.clone());
        Ok(uniform)
    }

    /// Links validated translated stages and assigns reflected private binding names.
    fn pipeline(&mut self, source: Rc<PipelineVersion>, offscreen: bool) -> Result<Rc<Pipeline>, String> {
        let key = (source.id, offscreen);
        if let Some(pipeline) = self.pipelines.get(&key) {
            return Ok(pipeline.clone());
        }
        // Naga's GL transform targets top-left offscreen storage. Direct presentation needs the
        // inverse Y transform; normalized depth conversion stays identical in both variants.
        let vertex = if offscreen {
            source.vertex.artifact.glsl.clone()
        } else {
            source.vertex.artifact.glsl.replace(
                "gl_Position.yz = vec2(-gl_Position.y, gl_Position.z * 2.0 - gl_Position.w);",
                "gl_Position.z = gl_Position.z * 2.0 - gl_Position.w;",
            )
        };
        let (fragment, height_name) = fragment_source(&source.fragment.artifact.glsl, offscreen)?;
        let raw = super::create_program(&self.gl, &vertex, &fragment).map_err(|error| error.to_string())?;
        // SAFETY: The new program is linked in this current context; the generated name is unique.
        let target_height = height_name.and_then(|name| unsafe { self.gl.get_uniform_location(raw, &name) });
        let pipeline = Rc::new(Pipeline {
            gl: self.gl.clone(),
            source,
            raw,
            target_height,
        });
        // SAFETY: Reflection comes from the same validated stages linked into this owned program.
        unsafe {
            self.gl.use_program(Some(raw));
            for shader in [&pipeline.source.vertex.artifact, &pipeline.source.fragment.artifact] {
                for uniform in &shader.uniforms {
                    if let Some(index) = self.gl.get_uniform_block_index(raw, &uniform.gl_block) {
                        self.gl.uniform_block_binding(raw, index, unit(shader.stage, uniform.slot));
                    }
                }
                for texture in &shader.textures {
                    let location = self.gl.get_uniform_location(raw, &texture.gl_name);
                    self.gl.uniform_1_i32(location.as_ref(), unit(shader.stage, texture.slot) as i32);
                }
            }
            self.gl.use_program(None);
        }
        self.check()?;
        self.pipelines.insert(key, pipeline.clone());
        Ok(pipeline)
    }

    /// Reports the first driver error before target acquisition or final successful completion.
    fn check(&self) -> Result<(), String> {
        // SAFETY: The owning context is current for every executor operation.
        let error = unsafe { self.gl.get_error() };
        if error == glow::NO_ERROR {
            Ok(())
        } else {
            Err(format!("generic graphics GL error 0x{error:04x}"))
        }
    }

    /// Replays only prepared native objects; the caller has already bound its authorized target.
    pub(crate) fn execute(&mut self, block: &PreparedBlock, logical: Size, physical: Size, offscreen: bool) -> Result<(), String> {
        // Retain before the first native command, including partial failure after an earlier draw.
        self.pending.try_reserve(1).map_err(|error| error.to_string())?;
        self.pending.push(block.clone());
        let gl = &self.gl;
        for command in &block.data.commands {
            let clip = match command {
                PreparedCommand::Draw(draw) => draw.clip,
                PreparedCommand::Color(_, clip) | PreparedCommand::Depth(_, clip) => *clip,
            };
            let Some(clip) = physical_rect(clip, logical, physical).map_err(|error| error.to_string())? else {
                continue;
            };
            let y = if offscreen { clip.y } else { physical.height as u32 - clip.y - clip.height };
            // SAFETY: Checked physical bounds fit the live attachment. No command selects a target.
            unsafe {
                gl.enable(glow::SCISSOR_TEST);
                gl.scissor(clip.x as i32, y as i32, clip.width as i32, clip.height as i32);
                gl.disable(glow::RASTERIZER_DISCARD);
                gl.disable(glow::STENCIL_TEST);
                gl.disable(glow::DITHER);
                gl.disable(glow::SAMPLE_COVERAGE);
                gl.disable(glow::SAMPLE_ALPHA_TO_COVERAGE);
                gl.disable(glow::POLYGON_OFFSET_FILL);
                match command {
                    PreparedCommand::Color(rgba, _) => {
                        let [r, g, b, a] = rgba.to_le_bytes().map(|byte| f32::from(byte) / 255.0);
                        gl.color_mask(true, true, true, true);
                        gl.clear_color(r, g, b, a);
                        gl.clear(glow::COLOR_BUFFER_BIT);
                    }
                    PreparedCommand::Depth(depth, _) => {
                        gl.depth_mask(true);
                        gl.clear_depth_f32(*depth);
                        gl.clear(glow::DEPTH_BUFFER_BIT);
                    }
                    PreparedCommand::Draw(draw) => self.draw(draw, logical, physical, offscreen)?,
                }
            }
        }
        // SAFETY: Unbind only this executor's private layout/program, while the context is current.
        unsafe {
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
        self.check()
    }

    /// Establishes complete fixed/binding state and issues one checked direct triangle command.
    fn draw(&self, draw: &PreparedDraw, logical: Size, physical: Size, offscreen: bool) -> Result<(), String> {
        let Some(view) = physical_rect(draw.viewport, logical, physical).map_err(|error| error.to_string())? else {
            return Ok(());
        };
        let gl = &self.gl;
        let info = draw.pipeline.source.info;
        let y = if offscreen { view.y } else { physical.height as u32 - view.y - view.height };
        // SAFETY: Prepared resources belong to this context; every byte range and layout is checked
        // by immutable draw admission. CPU snapshots cannot be changed during this replay.
        unsafe {
            gl.viewport(view.x as i32, y as i32, view.width as i32, view.height as i32);
            gl.depth_range_f32(0.0, 1.0);
            gl.depth_mask(info.depth_write);
            if info.depth_compare == GpuCompare::Always && !info.depth_write {
                gl.disable(glow::DEPTH_TEST);
            } else {
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(compare(info.depth_compare));
            }
            gl.color_mask(
                info.color_write & 1 != 0,
                info.color_write & 2 != 0,
                info.color_write & 4 != 0,
                info.color_write & 8 != 0,
            );
            if info.blend.enabled {
                gl.enable(glow::BLEND);
                gl.blend_func_separate(
                    factor(info.blend.src_color),
                    factor(info.blend.dst_color),
                    factor(info.blend.src_alpha),
                    factor(info.blend.dst_alpha),
                );
                gl.blend_equation_separate(blend_op(info.blend.color_op), blend_op(info.blend.alpha_op));
            } else {
                gl.disable(glow::BLEND);
            }
            if info.cull == GpuCullMode::None {
                gl.disable(glow::CULL_FACE);
            } else {
                gl.enable(glow::CULL_FACE);
                gl.cull_face(if info.cull == GpuCullMode::Front { glow::FRONT } else { glow::BACK });
            }
            let ccw = (info.front_face == GpuFrontFace::Ccw) != offscreen;
            gl.front_face(if ccw { glow::CCW } else { glow::CW });
            gl.use_program(Some(draw.pipeline.raw));
            gl.uniform_1_f32(draw.pipeline.target_height.as_ref(), physical.height as f32);
            gl.bind_vertex_array(Some(self.array));
            for location in 0..16 {
                gl.disable_vertex_attrib_array(location);
                gl.vertex_attrib_divisor(location, 0);
            }
            for attribute in &draw.pipeline.source.attributes {
                let (slot, buffer, offset) = draw
                    .vertices
                    .iter()
                    .find(|(slot, _, _)| *slot == attribute.slot)
                    .ok_or("prepared vertex slot missing")?;
                let layout = draw
                    .pipeline
                    .source
                    .vertex_buffers
                    .iter()
                    .find(|layout| layout.slot == *slot)
                    .ok_or("prepared vertex layout missing")?;
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer.raw));
                gl.enable_vertex_attrib_array(attribute.location);
                let (components, kind, normalized) = vertex_format(attribute.format);
                let offset = (*offset + u64::from(attribute.offset)) as i32;
                if kind == glow::UNSIGNED_INT {
                    gl.vertex_attrib_pointer_i32(attribute.location, components, kind, layout.stride as i32, offset);
                } else {
                    gl.vertex_attrib_pointer_f32(attribute.location, components, kind, normalized, layout.stride as i32, offset);
                }
                gl.vertex_attrib_divisor(attribute.location, if layout.step == GpuStepMode::Instance { 1 } else { 0 });
            }
            for (unit, texture, sampler) in &draw.textures {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture.image.raw));
                gl.bind_sampler(*unit, Some(sampler.raw));
            }
            for (unit, uniform) in &draw.uniforms {
                gl.bind_buffer_base(glow::UNIFORM_BUFFER, *unit, Some(uniform.raw));
            }
            match draw.range {
                DrawRange::Vertices { first, count, instances } => gl.draw_arrays_instanced(glow::TRIANGLES, first as i32, count as i32, instances as i32),
                DrawRange::Indices { first, count, instances } => {
                    let (buffer, format, offset) = draw.index.as_ref().ok_or("prepared index binding missing")?;
                    let (kind, size) = if *format == GpuIndexFormat::U16 {
                        (glow::UNSIGNED_SHORT, 2)
                    } else {
                        (glow::UNSIGNED_INT, 4)
                    };
                    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(buffer.raw));
                    gl.draw_elements_instanced(glow::TRIANGLES, count as i32, kind, (offset + u64::from(first) * size) as i32, instances as i32);
                }
            }
        }
        Ok(())
    }
}

impl Drop for Executor {
    /// Waits for final immediate uses before releasing accounting and deleting private objects.
    fn drop(&mut self) {
        // SAFETY: The plugin selects this context before destroying its renderer.
        unsafe {
            self.gl.finish();
            for batch in self.submitted.drain(..) {
                self.gl.delete_sync(batch.fence);
            }
            self.pending.clear();
            self.gl.delete_vertex_array(self.array);
        }
    }
}

/// Flattens the bounded stage-local resource namespace into native GL binding indices.
fn unit(stage: GpuShaderStage, slot: u32) -> u32 {
    if stage == GpuShaderStage::Vertex { slot } else { 4 + slot }
}
/// Maps one admitted non-mip filter.
fn filter(value: GpuFilter) -> u32 {
    if value == GpuFilter::Nearest { glow::NEAREST } else { glow::LINEAR }
}
/// Maps one admitted address policy.
fn address(value: GpuAddressMode) -> u32 {
    match value {
        GpuAddressMode::Clamp => glow::CLAMP_TO_EDGE,
        GpuAddressMode::Repeat => glow::REPEAT,
        _ => glow::MIRRORED_REPEAT,
    }
}

/// Maps an admitted complete depth comparison.
fn compare(value: GpuCompare) -> u32 {
    match value {
        GpuCompare::Never => glow::NEVER,
        GpuCompare::Less => glow::LESS,
        GpuCompare::Equal => glow::EQUAL,
        GpuCompare::LessEqual => glow::LEQUAL,
        GpuCompare::Greater => glow::GREATER,
        GpuCompare::NotEqual => glow::NOTEQUAL,
        GpuCompare::GreaterEqual => glow::GEQUAL,
        _ => glow::ALWAYS,
    }
}
/// Maps admitted separate color/alpha blend factors.
fn factor(value: GpuBlendFactor) -> u32 {
    match value {
        GpuBlendFactor::Zero => glow::ZERO,
        GpuBlendFactor::One => glow::ONE,
        GpuBlendFactor::SrcColor => glow::SRC_COLOR,
        GpuBlendFactor::OneMinusSrcColor => glow::ONE_MINUS_SRC_COLOR,
        GpuBlendFactor::DstColor => glow::DST_COLOR,
        GpuBlendFactor::OneMinusDstColor => glow::ONE_MINUS_DST_COLOR,
        GpuBlendFactor::SrcAlpha => glow::SRC_ALPHA,
        GpuBlendFactor::OneMinusSrcAlpha => glow::ONE_MINUS_SRC_ALPHA,
        GpuBlendFactor::DstAlpha => glow::DST_ALPHA,
        _ => glow::ONE_MINUS_DST_ALPHA,
    }
}
/// Maps an admitted unblended/min/max/add/subtract equation.
fn blend_op(value: GpuBlendOp) -> u32 {
    match value {
        GpuBlendOp::Add => glow::FUNC_ADD,
        GpuBlendOp::Subtract => glow::FUNC_SUBTRACT,
        GpuBlendOp::ReverseSubtract => glow::FUNC_REVERSE_SUBTRACT,
        GpuBlendOp::Min => glow::MIN,
        _ => glow::MAX,
    }
}
/// Interprets portable encoded attributes; integer inputs never use float conversion.
fn vertex_format(value: GpuVertexFormat) -> (i32, u32, bool) {
    match value {
        GpuVertexFormat::Float32 => (1, glow::FLOAT, false),
        GpuVertexFormat::Float32x2 => (2, glow::FLOAT, false),
        GpuVertexFormat::Float32x3 => (3, glow::FLOAT, false),
        GpuVertexFormat::Float32x4 => (4, glow::FLOAT, false),
        GpuVertexFormat::Uint32 => (1, glow::UNSIGNED_INT, false),
        GpuVertexFormat::Uint32x2 => (2, glow::UNSIGNED_INT, false),
        GpuVertexFormat::Uint32x3 => (3, glow::UNSIGNED_INT, false),
        GpuVertexFormat::Uint32x4 => (4, glow::UNSIGNED_INT, false),
        _ => (4, glow::UNSIGNED_BYTE, true),
    }
}

/// Normalizes generated fragment position for direct presentation without exposing native names.
fn fragment_source(source: &str, offscreen: bool) -> Result<(String, Option<String>), String> {
    if offscreen {
        return Ok((source.into(), None));
    }
    // Generated GLSL identifiers are ASCII. A legal client identifier may contain the
    // builtin spelling; normalize only complete identifier tokens, never those substrings.
    let identifier = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let builtin = "gl_FragCoord";
    let occurrences: Vec<_> = source
        .match_indices(builtin)
        .filter_map(|(start, _)| {
            let end = start + builtin.len();
            let before = start > 0 && identifier(source.as_bytes()[start - 1]);
            let after = end < source.len() && identifier(source.as_bytes()[end]);
            (!before && !after).then_some(start)
        })
        .collect();
    if occurrences.is_empty() {
        return Ok((source.into(), None));
    }
    // Choose a fresh identifier against the complete compiler output so a legal client local
    // cannot shadow the injected uniform. Only canonical translated GLSL reaches this boundary.
    let mut ordinal = 0u32;
    let name = loop {
        let name = format!("nc_target_height_{ordinal}");
        if !source.contains(&name) {
            break name;
        }
        ordinal = ordinal.checked_add(1).ok_or("fragment private name space exhausted")?;
    };
    let expression = format!("vec4(gl_FragCoord.x, {name} - gl_FragCoord.y, gl_FragCoord.zw)");
    let mut result = String::new();
    let mut first = 0;
    for start in occurrences {
        result.push_str(&source[first..start]);
        result.push_str(&expression);
        first = start + builtin.len();
    }
    result.push_str(&source[first..]);
    // Naga emits all extension directives before precision declarations. Inserting here retains
    // the required directive ordering and declares the uniform before any generated function.
    let position = result.find("precision ").ok_or("missing canonical GLSL precision preamble")?;
    result.insert_str(position, &format!("uniform highp float {name};\n"));
    Ok((result, Some(name)))
}
