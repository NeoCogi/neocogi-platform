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

/// Converts a Rust optional shared source to the generated checked-value representation.
/// Initial data remains portable; it does not acquire driver identity or native ownership.
pub fn initial(source: Option<ComPtr<IGpuDataSourceAbi>>) -> InitialGpuData {
    match source {
        None => InitialGpuData::None,
        Some(source) => InitialGpuData::Some(source),
    }
}

/// Acquires a shared source for callers that currently hold only a borrowed interface.
/// AddRef is a foreign call and may fail; callers must propagate that failure.
pub fn supplied(source: neocogi_com::ComRef<'_, IGpuDataSourceAbi>) -> Result<InitialGpuData, NcStatus> {
    source.try_add_ref().map(|source| initial(Some(source)))
}

impl InitialGpuData {
    /// Borrows the generated wire representation without assuming Rust Option layout.
    pub fn borrow(&self) -> Result<neocogi_com::NcValueRef<'_, Self>, NcStatus> {
        neocogi_com::NcValueRef::new(self)
    }
}

/// Copies supplied initial bytes in bounded chunks, or represents absent zero data by None.
/// The receiver's owned source and its final foreign Release end before this returns.
/// Callers validate shape/reserve quotas first, then recheck admission before publication.
pub fn read_initial(source: neocogi_com::NcValueRef<'_, InitialGpuData>, len: usize) -> Result<Option<Vec<u8>>, NcStatus> {
    match source.to_owned()? {
        InitialGpuData::None => Ok(None),
        InitialGpuData::Some(source) => {
            if source.byte_count()? != len as u64 {
                return Err(NC_E_INVALID_ARG);
            }
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(len).map_err(|_| neocogi_com::NC_E_OUT_OF_MEMORY)?;
            let mut output = neocogi_com::NcOutputBuffer::new();
            for offset in (0..len).step_by(64 * 1024) {
                let wanted = (len - offset).min(64 * 1024);
                source.read(output.output(offset as u64, wanted))?;
                if output.len() != wanted {
                    return Err(NC_E_INVALID_ARG);
                }
                bytes.extend_from_slice(output.as_slice());
            }
            if source.byte_count()? != len as u64 {
                return Err(NC_E_INVALID_ARG);
            }
            Ok(Some(bytes))
        }
    }
}

#[com_object(interfaces(IRenderDriverIdentity))]
/// Metadata token containing no native core, driver, factory or window ownership.
struct DriverIdentity;
impl IRenderDriverIdentity for DriverIdentity {}

/// Mints one process-local identity to retain on a newly created native core.
/// Pointer identity is unique while retained, including across independently loaded providers.
pub fn new_driver_identity() -> ComPtr<IRenderDriverIdentityAbi> {
    DriverIdentity::from_value(DriverIdentity).query().expect("driver identity interface")
}

/// Transfers an already owned initialized byte vector into an optional upload source.
pub fn initial_bytes(bytes: Vec<u8>) -> InitialGpuData {
    initial(Some(Bytes::source(bytes)))
}
