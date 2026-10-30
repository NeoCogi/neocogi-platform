//! Owned Rust producers for the canonical typed resource upload ABI.
//!
//! These helpers encode no vertex, camera or material convention. Applications choose their
//! byte layout and declare it explicitly in the graphics pipeline descriptor.
use crate::abi::*;
use neocogi_com::{com_object, ComInlineValue, ComPtr, NcOutputWriter, NcStatus, NC_E_INVALID_ARG, NC_E_OUT_OF_RANGE};

#[com_object(interfaces(IGpuDataSource))]
/// Complete immutable byte source with bounded, exact-prefix reads over generated COM.
pub struct Bytes {
    /// Application-encoded initialized bytes, independent of Rust semantic value layout.
    bytes: Vec<u8>,
}
impl Bytes {
    /// Transfers initialized bytes to an independently retainable immutable source.
    pub fn source(bytes: Vec<u8>) -> ComPtr<IGpuDataSourceAbi> {
        Self::from_value(Self { bytes })
            .query()
            .expect("byte producer declares its canonical interface")
    }
}
impl IGpuDataSource for Bytes {
    /// Length remains fixed across every bounded read.
    fn byte_count(&self) -> Result<u64, NcStatus> {
        Ok(self.bytes.len() as u64)
    }
    /// Writes the complete requested interval or fails without exposing an uninitialized suffix.
    fn read(&self, output: NcOutputWriter<'_, u8>) -> Result<(), NcStatus> {
        copy(&self.bytes, output)
    }
}

#[derive(Default)]
#[com_object(interfaces(IGpuPipelineSource))]
/// Immutable owned pipeline description; its counts are derived from the owned arrays.
pub struct PipelineDesc {
    /// Fixed triangle, color, depth and raster state; array counts are replaced by `source`.
    pub state: GpuPipelineInfo,
    /// Explicit byte stepping and per-vertex/per-instance advancement.
    pub vertex_buffers: Vec<GpuVertexBufferLayout>,
    /// Explicit locations, formats and offsets into those streams.
    pub attributes: Vec<GpuVertexAttribute>,
    /// Exact reflected stage-local resource declarations.
    pub bindings: Vec<GpuShaderBinding>,
}
impl PipelineDesc {
    /// Publishes fixed counts after checked conversion; admission validates all descriptor content.
    pub fn source(mut self) -> Result<ComPtr<IGpuPipelineSourceAbi>, NcStatus> {
        self.state.vertex_buffer_count = self.vertex_buffers.len().try_into().map_err(|_| NC_E_OUT_OF_RANGE)?;
        self.state.attribute_count = self.attributes.len().try_into().map_err(|_| NC_E_OUT_OF_RANGE)?;
        self.state.binding_count = self.bindings.len().try_into().map_err(|_| NC_E_OUT_OF_RANGE)?;
        Self::from_value(self).query()
    }
}
impl IGpuPipelineSource for PipelineDesc {
    /// Returns immutable fixed state and exact array extents.
    fn info(&self) -> Result<GpuPipelineInfo, NcStatus> {
        Ok(self.state)
    }
    /// Copies exactly the requested layout interval.
    fn vertex_buffers(&self, output: NcOutputWriter<'_, GpuVertexBufferLayout>) -> Result<(), NcStatus> {
        copy(&self.vertex_buffers, output)
    }
    /// Copies exactly the requested attribute interval.
    fn attributes(&self, output: NcOutputWriter<'_, GpuVertexAttribute>) -> Result<(), NcStatus> {
        copy(&self.attributes, output)
    }
    /// Copies exactly the requested resource declaration interval.
    fn bindings(&self, output: NcOutputWriter<'_, GpuShaderBinding>) -> Result<(), NcStatus> {
        copy(&self.bindings, output)
    }
}

/// Checks all output arithmetic before writing any typed prefix.
fn copy<T: ComInlineValue>(data: &[T], mut output: NcOutputWriter<'_, T>) -> Result<(), NcStatus> {
    let first = usize::try_from(output.offset()).map_err(|_| NC_E_OUT_OF_RANGE)?;
    let end = first.checked_add(output.capacity()).ok_or(NC_E_OUT_OF_RANGE)?;
    output.extend_from_slice(data.get(first..end).ok_or(NC_E_INVALID_ARG)?)
}
