//! Bounded import of immutable compiler exports. Callers retain their provider
//! charge across construction, copying, errors and final destruction of this reader.
use crate::shader::{abi::*, *};
use neocogi_com::{ComPtr, INcStringAbi, INcBytesAbi, INcStringComPtrExt, INcBytesComPtrExt, NcStatus, NC_E_INVALID_ARG, NC_E_OUT_OF_RANGE, NC_E_OUT_OF_MEMORY};

/// Owned foreign exports admitted before receiving code allocation. Metadata is
/// separately bounded and deliberately excluded from the code-payload receipt.
pub struct PreparedShader {
    /// Retains the immutable report until all post-copy agreement checks finish.
    report: ComPtr<IShaderCompilationAbi>,
    /// Initial immutable summary, checked against object results.
    info: ShaderInfo,
    /// Selected entry export.
    entry: ComPtr<INcStringAbi>,
    /// Validated canonical source export.
    wgsl: ComPtr<INcStringAbi>,
    /// ES300 export.
    glsl: ComPtr<INcStringAbi>,
    /// Little-endian SPIR-V export.
    spirv: ComPtr<INcBytesAbi>,
    /// Copied bounded metadata, with no remaining foreign item owners.
    inputs: Vec<IoLocation>,
    /// Copied bounded output metadata.
    outputs: Vec<IoLocation>,
    /// Copied bounded block names and layouts.
    uniforms: Vec<Uniform>,
    /// Copied bounded combined sampler names.
    textures: Vec<SampledTexture>,
    /// Expected entry kept in fixed scratch; no uncharged receiving code allocation.
    expected_entry: [u8; 128],
    /// Initialized entry scratch prefix.
    expected_len: usize,
    /// Exact checked code-payload bytes.
    bytes: usize,
}
impl PreparedShader {
    /// Acquires and bounds each export. Summary is checked, never used as authority
    /// in place of a collection/object. Failure releases all acquired foreign owners.
    pub fn new(report: ComPtr<IShaderCompilationAbi>, stage: GpuShaderStage, entry: &[u8], cap: usize) -> Result<Self, NcStatus> {
        if entry.is_empty() || entry.len() > 128 {
            return Err(NC_E_INVALID_ARG);
        }
        let status = NcStatus::from_code(report.status()?);
        if status != neocogi_com::NC_OK {
            return Err(if status.is_err() { status } else { NC_E_INVALID_ARG });
        }
        let info = report.info()?;
        let entry_object = report.entry_point()?;
        let wgsl = report.wgsl()?;
        let glsl = report.glsl()?;
        let spirv = report.spirv()?;
        let entry_len = entry_object.byte_len()?;
        let wgsl_len = wgsl.byte_len()?;
        let glsl_len = glsl.byte_len()?;
        let spv_len = spirv.len()?;
        let bytes = entry_len
            .checked_add(wgsl_len)
            .and_then(|v| v.checked_add(glsl_len))
            .and_then(|v| v.checked_add(spv_len))
            .filter(|v| *v <= cap.min(MAX_SHADER_ARTIFACT_BYTES) as u64)
            .ok_or(NC_E_OUT_OF_RANGE)? as usize;
        if info.stage != stage
            || entry_len != entry.len() as u64
            || wgsl_len == 0
            || wgsl_len > MAX_SHADER_BYTES as u64
            || glsl_len == 0
            || spv_len < 20
            || spv_len % 4 != 0
            || info.entry_point_byte_count != entry_len
            || info.wgsl_byte_count != wgsl_len
            || info.glsl_byte_count != glsl_len
            || info.spirv_word_count != spv_len / 4
        {
            return Err(NC_E_INVALID_ARG);
        }
        let (inputs, outputs, uniforms, textures) = reflection(&report, stage, glsl_len as usize)?;
        if info.input_count != inputs.len() as u32
            || info.output_count != outputs.len() as u32
            || info.uniform_count != uniforms.len() as u32
            || info.sampled_texture_count != textures.len() as u32
        {
            return Err(NC_E_INVALID_ARG);
        }
        let mut expected_entry = [0; 128];
        expected_entry[..entry.len()].copy_from_slice(entry);
        Ok(Self {
            report,
            info,
            entry: entry_object,
            wgsl,
            glsl,
            spirv,
            inputs,
            outputs,
            uniforms,
            textures,
            expected_entry,
            expected_len: entry.len(),
            bytes,
        })
    }
    /// Receiving code allocations may start only after the caller reserves this charge.
    pub fn byte_len(&self) -> usize {
        self.bytes
    }
    /// Copies a complete plain snapshot. Foreign exports never enter native caches.
    /// The caller must drop this reader before refunding its provider reservation.
    pub fn copy(&self) -> Result<Shader, NcStatus> {
        let entry = string(&self.entry, self.info.entry_point_byte_count as usize)?;
        if entry.as_bytes() != &self.expected_entry[..self.expected_len] {
            return Err(NC_E_INVALID_ARG);
        }
        let wgsl = string(&self.wgsl, self.info.wgsl_byte_count as usize)?;
        let glsl = string(&self.glsl, self.info.glsl_byte_count as usize)?;
        let spirv = spirv_words(&self.spirv, self.info.spirv_word_count as usize)?;
        let metadata = reflection(&self.report, self.info.stage, self.info.glsl_byte_count as usize)?;
        if !same_info(self.info, self.report.info()?)
            || metadata.0 != self.inputs
            || metadata.1 != self.outputs
            || metadata.2 != self.uniforms
            || metadata.3 != self.textures
            || self.entry.byte_len()? != self.info.entry_point_byte_count
            || self.wgsl.byte_len()? != self.info.wgsl_byte_count
            || self.glsl.byte_len()? != self.info.glsl_byte_count
            || self.spirv.len()? != self.info.spirv_word_count * 4
        {
            return Err(NC_E_INVALID_ARG);
        }
        Ok(Shader {
            stage: self.info.stage,
            entry,
            wgsl,
            glsl,
            spirv,
            inputs: metadata.0,
            outputs: metadata.1,
            uniforms: metadata.2,
            textures: metadata.3,
        })
    }
}
/// Generated record has no Eq contract; compare every wire field explicitly.
fn same_info(a: ShaderInfo, b: ShaderInfo) -> bool {
    a.stage == b.stage
        && a.input_count == b.input_count
        && a.output_count == b.output_count
        && a.uniform_count == b.uniform_count
        && a.sampled_texture_count == b.sampled_texture_count
        && a.entry_point_byte_count == b.entry_point_byte_count
        && a.wgsl_byte_count == b.wgsl_byte_count
        && a.spirv_word_count == b.spirv_word_count
        && a.glsl_byte_count == b.glsl_byte_count
}
/// Reads exactly the previously admitted immutable UTF-8 length and detects length changes.
fn string(value: &ComPtr<INcStringAbi>, len: usize) -> Result<String, NcStatus> {
    if value.byte_len()? != len as u64 {
        return Err(NC_E_INVALID_ARG);
    }
    let result = value.to_string_bounded(len)?;
    if result.len() != len || value.byte_len()? != len as u64 {
        return Err(NC_E_INVALID_ARG);
    }
    Ok(result)
}
/// Decodes LE words into aligned storage with bounded stack scratch, never a second
/// full byte allocation. Exact-write SDK calls must initialize the complete output.
fn spirv_words(value: &ComPtr<INcBytesAbi>, words: usize) -> Result<Vec<u32>, NcStatus> {
    let len = words.checked_mul(4).ok_or(NC_E_OUT_OF_RANGE)?;
    if words < 5 || value.len()? != len as u64 {
        return Err(NC_E_INVALID_ARG);
    }
    let mut output = Vec::new();
    output.try_reserve_exact(words).map_err(|_| NC_E_OUT_OF_MEMORY)?;
    let mut scratch = [0u8; 4096];
    for offset in (0..len).step_by(scratch.len()) {
        let count = (len - offset).min(scratch.len());
        value.copy_to(offset as u64, &mut scratch[..count])?;
        for word in scratch[..count].as_chunks::<4>().0 {
            output.push(u32::from_le_bytes(*word));
        }
    }
    if output[0] != 0x07230203 || value.len()? != len as u64 {
        return Err(NC_E_INVALID_ARG);
    }
    Ok(output)
}
/// Metadata is bounded separately from code, and contains no opaque implementation types.
type Reflection = (Vec<IoLocation>, Vec<IoLocation>, Vec<Uniform>, Vec<SampledTexture>);
/// Copies independently accessible items and verifies finite qualifiers and uniqueness.
fn reflection(report: &ComPtr<IShaderCompilationAbi>, stage: GpuShaderStage, name_limit: usize) -> Result<Reflection, NcStatus> {
    let inputs = locations(report.inputs()?, stage, ShaderDirection::Input)?;
    let outputs = locations(report.outputs()?, stage, ShaderDirection::Output)?;
    if stage == GpuShaderStage::Fragment
        && (outputs.len() != 1
            || outputs[0].location != 0
            || outputs[0].shape
                != (IoShape {
                    kind: ShaderScalar::Float32,
                    components: 4,
                }))
    {
        return Err(NC_E_INVALID_ARG);
    }
    let list = report.uniforms()?;
    let count = list.len()?;
    if count > 4 {
        return Err(NC_E_OUT_OF_RANGE);
    }
    let mut uniforms = Vec::<Uniform>::new();
    let mut names = 0usize;
    for index in 0..count {
        let item = list.get(index)?;
        let info = item.info()?;
        if info.stage != stage || info.slot >= 4 || info.byte_len == 0 || info.byte_len > 16384 || uniforms.iter().any(|v| v.slot == info.slot) {
            return Err(NC_E_INVALID_ARG);
        }
        let name = item.glsl_block_name()?;
        let gl_block = native_name(&name, name_limit, &mut names)?;
        let again = item.info()?;
        if info.stage != again.stage || info.slot != again.slot || info.byte_len != again.byte_len {
            return Err(NC_E_INVALID_ARG);
        }
        uniforms.push(Uniform {
            slot: info.slot,
            byte_len: info.byte_len,
            gl_block,
        });
    }
    if list.len()? != count {
        return Err(NC_E_INVALID_ARG);
    }
    let list = report.sampled_textures()?;
    let count = list.len()?;
    if count > 4 {
        return Err(NC_E_OUT_OF_RANGE);
    }
    let mut textures = Vec::<SampledTexture>::new();
    for index in 0..count {
        let item = list.get(index)?;
        let info = item.info()?;
        if info.stage != stage || info.slot >= 4 || textures.iter().any(|v| v.slot == info.slot) {
            return Err(NC_E_INVALID_ARG);
        }
        let name = item.glsl_name()?;
        let gl_name = native_name(&name, name_limit, &mut names)?;
        let again = item.info()?;
        if info.stage != again.stage || info.slot != again.slot {
            return Err(NC_E_INVALID_ARG);
        }
        textures.push(SampledTexture { slot: info.slot, gl_name });
    }
    if list.len()? != count {
        return Err(NC_E_INVALID_ARG);
    }
    Ok((inputs, outputs, uniforms, textures))
}
/// Native identifiers are bounded by admitted GLSL length, retaining long valid names.
fn native_name(value: &ComPtr<INcStringAbi>, limit: usize, total: &mut usize) -> Result<String, NcStatus> {
    let len = usize::try_from(value.byte_len()?).map_err(|_| NC_E_OUT_OF_RANGE)?;
    if len == 0 || len > limit {
        return Err(NC_E_OUT_OF_RANGE);
    }
    *total = total.checked_add(len).filter(|v| *v <= limit.saturating_mul(8)).ok_or(NC_E_OUT_OF_RANGE)?;
    let name = string(value, len)?;
    if !name
        .bytes()
        .enumerate()
        .all(|(i, c)| c.is_ascii_alphabetic() || c == b'_' || (i > 0 && c.is_ascii_digit()))
    {
        return Err(NC_E_INVALID_ARG);
    }
    Ok(name)
}
/// Checks each location independently of aggregate report counts.
fn locations(list: ComPtr<IShaderLocationsAbi>, stage: GpuShaderStage, direction: ShaderDirection) -> Result<Vec<IoLocation>, NcStatus> {
    let count = list.len()?;
    if count > 16 {
        return Err(NC_E_OUT_OF_RANGE);
    }
    let mut values = Vec::<IoLocation>::new();
    for index in 0..count {
        let item = list.get(index)?;
        let v = item.info()?;
        if v.stage != stage
            || v.direction != direction
            || v.location >= 16
            || !(1..=4).contains(&v.components)
            || values.iter().any(|old| old.location == v.location)
            || !matches!(v.scalar, ShaderScalar::Float32 | ShaderScalar::Uint32)
            || !matches!(
                v.interpolation,
                ShaderInterpolation::None | ShaderInterpolation::Perspective | ShaderInterpolation::Linear | ShaderInterpolation::Flat
            )
            || !matches!(
                v.sampling,
                ShaderSampling::None
                    | ShaderSampling::Center
                    | ShaderSampling::Centroid
                    | ShaderSampling::Sample
                    | ShaderSampling::First
                    | ShaderSampling::Either
            )
        {
            return Err(NC_E_INVALID_ARG);
        }
        let again = item.info()?;
        if v.stage != again.stage
            || v.direction != again.direction
            || v.location != again.location
            || v.scalar != again.scalar
            || v.components != again.components
            || v.interpolation != again.interpolation
            || v.sampling != again.sampling
        {
            return Err(NC_E_INVALID_ARG);
        }
        values.push(IoLocation {
            location: v.location,
            shape: IoShape { kind: v.scalar, components: v.components },
            interpolation: v.interpolation,
            sampling: v.sampling,
        });
    }
    if list.len()? != count {
        return Err(NC_E_INVALID_ARG);
    }
    values.sort_by_key(|v| v.location);
    Ok(values)
}
