//! Independent UI sessions sharing a concrete device and its authenticated textures.
//!
//! COM methods receive shared references, so one documented RefCell gates the
//! session. Execution takes ownership of its state and thereafter uses ordinary
//! mutable references; no borrow guard survives foreign image or stream calls.

use super::*;
use super::resources::{publish_texture, retain_texture};
use neocogi_render_support::com::Batch;

/// Atlas pixels uploaded once for a surface, independent of the native window atlas.
struct SurfaceAtlas {
    /// Source identity makes repeated preparation of the same immutable image cheap.
    source: ComPtr<IRgbaImageAbi>,
    /// Private GPU allocation retires with its last session reference.
    image: Rc<TextureRecord>,
}

/// Owned state detached while a surface prepares and submits one complete frame.
struct SurfaceState {
    /// Requested drawable size; zero dimensions suspend rendering.
    size: Size,
    /// Current atlas changes only after a candidate upload succeeds.
    atlas: SurfaceAtlas,
    /// Last published immutable frame, retained through failed frames and resize.
    completed: Option<ComPtr<ITextureAbi>>,
}

#[com_object(interfaces(IRenderSurface, IRenderer, IRenderDiagnostics))]
/// One UI rendering session; it never polls input or presents the native window.
struct Surface {
    /// Shared typed device keeps the native context alive through final texture retirement.
    device: Rc<Device>,
    /// COM shared-reference boundary; None rejects reentry while ordinary owned mutation runs.
    state: RefCell<Option<SurfaceState>>,
}

/// Restores detached session state on success, failure, and unwinding.
struct SurfaceAccess<'a> {
    /// Restoration slot is borrowed only before and after execution.
    owner: &'a Surface,
    /// Exclusive session state used through mutable Rust references during execution.
    state: Option<SurfaceState>,
}

impl Surface {
    /// Takes session ownership after the device has rejected recursive GPU execution.
    fn access(&self) -> Result<SurfaceAccess<'_>, NcStatus> {
        let state = self.state.try_borrow_mut().map_err(|_| RENDER_E_BUSY)?.take().ok_or(RENDER_E_BUSY)?;
        Ok(SurfaceAccess { owner: self, state: Some(state) })
    }
}

impl SurfaceAccess<'_> {
    /// Borrows only the local state owned by this execution transaction.
    fn state(&mut self) -> &mut SurfaceState {
        self.state.as_mut().expect("live surface transaction")
    }
}

impl Drop for SurfaceAccess<'_> {
    /// Restores exactly once without invoking foreign methods under a borrow guard.
    fn drop(&mut self) {
        *self.owner.state.borrow_mut() = self.state.take();
    }
}

/// Checks bounded extents without allocating pixels, including suspended layouts.
fn validate_size(size: Size) -> Result<(), NcStatus> {
    if size.width < 0 || size.height < 0 {
        return Err(NC_E_INVALID_ARG);
    }
    // Validate a minimal positive counterpart so even a suspended dimension cannot
    // smuggle an unbounded extent into later allocation.
    TextureId::for_render_target(size.width.max(1), size.height.max(1)).map_err(|_| NC_E_INVALID_ARG)?;
    Ok(())
}

impl SurfaceAtlas {
    /// Uploads through the same device as the output, with no native atlas replacement.
    fn upload(device: &Rc<Device>, gpu: &mut gpu::Gpu, source: ComRef<'_, IRgbaImageAbi>) -> Result<Self, NcStatus> {
        let retained = source.try_add_ref()?;
        let pixels = AtlasPixels::read(source)?;
        let image = pixels.image();
        let id = TextureId::for_upload(image.width, image.height, image.pixels).map_err(|_| NC_E_INVALID_ARG)?;
        gpu.create_texture(id, image.pixels).map_err(|error| device.failure(error))?;
        Ok(Self {
            source: retained,
            image: retain_texture(device.clone(), id),
        })
    }

    /// Replaces the session binding only after candidate pixels upload successfully.
    fn prepare(&mut self, device: &Rc<Device>, gpu: &mut gpu::Gpu, source: ComRef<'_, IRgbaImageAbi>) -> Result<(), NcStatus> {
        if self.source.as_raw().object != source.as_raw().object {
            *self = Self::upload(device, gpu, source)?;
        }
        Ok(())
    }
}

impl IRenderSurfaceFactory for WindowObject {
    /// Publishes an independent session after validating size and uploading its private atlas.
    fn create_surface(&self, size: Size, atlas: ComRef<'_, IRgbaImageAbi>) -> Result<ComPtr<IRenderSurfaceAbi>, NcStatus> {
        validate_size(size)?;
        let mut access = self.device.access()?;
        let gpu = &mut access.state().gpu;
        if !gpu.supports_surfaces() {
            return Err(self.device.failure("offscreen UI surfaces are unavailable on this backend"));
        }
        let atlas = SurfaceAtlas::upload(&self.device, gpu, atlas)?;
        Surface::from_value(Surface {
            device: self.device.clone(),
            state: RefCell::new(Some(SurfaceState { size, atlas, completed: None })),
        })
        .query()
    }
}

impl IRenderSurface for Surface {
    /// Reads committed geometry without exposing pending session state.
    fn size(&self) -> Result<Size, NcStatus> {
        Ok(self.state.try_borrow().map_err(|_| RENDER_E_BUSY)?.as_ref().ok_or(RENDER_E_BUSY)?.size)
    }

    /// Changes future frame geometry while retaining every completed image unchanged.
    fn resize(&self, size: Size) -> Result<(), NcStatus> {
        validate_size(size)?;
        let _device = self.device.access()?;
        self.access()?.state().size = size;
        Ok(())
    }

    /// Retains an immutable completed result; initial absence is an explicit status.
    fn completed_image(&self) -> Result<ComPtr<ITextureAbi>, NcStatus> {
        self.state
            .try_borrow()
            .map_err(|_| RENDER_E_BUSY)?
            .as_ref()
            .ok_or(RENDER_E_BUSY)?
            .completed
            .clone()
            .ok_or(neocogi_com::NC_E_NOT_FOUND)
    }
}

impl IRenderer for Surface {
    /// Changes this session's atlas without altering the editor's resident image.
    fn prepare_atlas(&self, atlas: ComRef<'_, IRgbaImageAbi>) -> Result<(), NcStatus> {
        let mut device = self.device.access()?;
        let mut session = self.access()?;
        session.state().atlas.prepare(&self.device, &mut device.state().gpu, atlas)
    }

    /// Validates all commands, renders to fresh private storage, and then publishes atomically.
    fn render(&self, info: RenderInfo, atlas: ComRef<'_, IRgbaImageAbi>, stream: ComRef<'_, IRenderStreamAbi>) -> Result<(), NcStatus> {
        let mut device = self.device.access()?;
        let mut session = self.access()?;
        let session = session.state();
        if info.size != session.size || info.size.width <= 0 || info.size.height <= 0 || info.clear.to_le_bytes()[3] != 255 {
            return Err(NC_E_INVALID_ARG);
        }
        let state = device.state();
        state.prepared.read(info, stream, self.device.as_ref())?;
        if state.prepared.batches().iter().any(|batch| matches!(batch, Batch::Item { .. })) {
            return Err(NC_E_INVALID_ARG);
        }
        session.atlas.prepare(&self.device, &mut state.gpu, atlas)?;
        let id = TextureId::for_render_target(info.size.width, info.size.height).map_err(|_| NC_E_INVALID_ARG)?;
        state.gpu.create_render_texture(id).map_err(|error| self.device.failure(error))?;
        // This guard retires failed output as well as successful images. Previously
        // published allocations are never selected as this frame's draw target.
        let output = retain_texture(self.device.clone(), id);
        state
            .gpu
            .render_surface(info, &state.prepared, id, session.atlas.image.id)
            .map_err(|error| self.device.failure(error))?;
        session.completed = Some(publish_texture(output));
        Ok(())
    }
}

impl IRenderDiagnostics for Surface {
    /// Copies the shared device diagnostic independently of later failures.
    fn diagnostic(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        Ok(NcString::from_utf8_str(&self.device.diagnostic.borrow()))
    }
}
