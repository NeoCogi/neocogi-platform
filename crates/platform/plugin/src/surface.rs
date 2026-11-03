//! Independent UI sessions sharing a concrete device and its authenticated textures.
//!
//! COM methods receive shared references, so one documented RefCell gates the
//! session. Execution takes ownership of its state and thereafter uses ordinary
//! mutable references; no borrow guard survives foreign image or stream calls.

use super::*;
use super::resources::{publish_texture, retain_texture};

/// Owned state detached while a surface prepares and submits one complete frame.
struct SurfaceState {
    /// Requested drawable size; zero dimensions suspend rendering.
    size: Size,
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
    rgba_bytes(Size {
        width: size.width.max(1),
        height: size.height.max(1),
    })?;
    Ok(())
}

impl IRenderSurfaceFactory for WindowObject {
    /// Publishes an asset-independent session after validating its requested size.
    fn create_surface(&self, size: Size) -> Result<ComPtr<IRenderSurfaceAbi>, NcStatus> {
        validate_size(size)?;
        let mut access = self.device.access()?;
        let gpu = &mut access.state().gpu;
        if !gpu.supports_surfaces() {
            return Err(self.device.failure("offscreen UI surfaces are unavailable on this backend"));
        }
        Surface::from_value(Surface {
            device: self.device.clone(),
            state: RefCell::new(Some(SurfaceState { size, completed: None })),
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
    /// Validates all commands, renders to fresh private storage, and then publishes atomically.
    fn render(&self, info: RenderInfo, stream: ComRef<'_, IRenderStreamAbi>) -> Result<(), NcStatus> {
        let mut device = self.device.access()?;
        let mut session = self.access()?;
        let session = session.state();
        if info.size != session.size || info.size.width <= 0 || info.size.height <= 0 || info.clear.to_le_bytes()[3] != 255 {
            return Err(NC_E_INVALID_ARG);
        }
        let state = device.state();
        let _operation = self.device.logical.operation()?;
        state.prepared.read(info, stream, self.device.as_ref())?;
        if state.prepared.blocks().iter().any(|block| !block.is_ui()) {
            return Err(NC_E_INVALID_ARG);
        }
        let image = state.gpu.render_surface(info, &state.prepared).map_err(|error| self.device.failure(error))?;
        let output = retain_texture(&self.device, image);
        session.completed = Some(publish_texture(output));
        Ok(())
    }
}

impl IRenderDiagnostics for Surface {
    /// Copies the shared device diagnostic independently of later failures.
    fn diagnostic(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        Ok(NcString::from_utf8_str(&self.device.logical.diagnostic()))
    }
}

impl IRenderDriverBound for Surface {
    fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
        Ok(self.device.logical.driver_identity())
    }
}
