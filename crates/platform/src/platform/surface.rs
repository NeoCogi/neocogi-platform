//! Authenticated framebuffer views and independent image-rendering targets on one device.
//!
//! COM methods receive shared references, so the image session uses a RefCell to
//! detach its owned state during execution. The device's separate short-lived
//! registry borrows authenticate public target identities. Neither borrow spans
//! foreign calls; execution mutates owned state through ordinary mutable references.

use super::*;
use super::resources::{publish_texture, retain_texture};

/// Owned state detached while a surface prepares and submits one complete frame.
struct SurfaceState {
    /// Requested drawable size; zero dimensions suspend rendering.
    size: Size,
    /// Last published immutable frame, retained through failed frames and resize.
    completed: Option<ComPtr<ITextureAbi>>,
}

#[com_object(interfaces(IRenderSurface, IRenderer))]
/// One independently submitted image target; completed images are immutable snapshots.
struct ImageSurface {
    /// Shared typed device keeps the native context alive through final texture retirement.
    record: Rc<SurfaceRecord>,
    /// COM shared-reference boundary; None rejects reentry while ordinary owned mutation runs.
    state: RefCell<Option<SurfaceState>>,
}

/// Restores detached session state on success, failure, and unwinding.
struct SurfaceAccess<'a> {
    /// Restoration slot is borrowed only before and after execution.
    owner: &'a ImageSurface,
    /// Exclusive session state used through mutable Rust references during execution.
    state: Option<SurfaceState>,
}

impl ImageSurface {
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

/// Construction-proven target metadata. The public capability alone owns its registry entry.
/// The device holds only a weak record, avoiding a surface/device cycle. Groups retain `target`
/// separately so releasing a surface retires its canonical address even while recorded groups live.
pub(super) struct SurfaceRecord {
    /// Native facade retained while the surface can independently submit or observe its framebuffer.
    device: Rc<Device>,
    /// Resource groups retain only this device/target identity, not this public surface object.
    pub(super) target: Rc<neocogi_platform::gpu::target::SurfaceTarget<gpu::Backend>>,
    /// Canonical identity installed before publication; removed before address reuse.
    /// The COM allocation determines its address, requiring one post-construction write before return.
    key: Cell<usize>,
}
impl Drop for SurfaceRecord {
    /// Remove canonical identity before the public capability's storage can be reused.
    fn drop(&mut self) {
        self.device.surfaces.borrow_mut().remove(&self.key.get());
    }
}

#[com_object(interfaces(IRenderSurface))]
/// View of the window framebuffer. Recording joins the enclosing frame; no renderer/presenter is exposed.
/// It owns no pixels, size setting or completed-image slot. Independent groups retain the same target
/// metadata, while only the outer window renderer may acquire, clear and present the whole frame.
struct FramebufferSurface {
    /// The view retains the real device and construction-proven target metadata.
    record: Rc<SurfaceRecord>,
}
impl IRenderSurface for FramebufferSurface {
    /// Returns the construction-selected kind without consulting caller metadata.
    fn kind(&self) -> Result<RenderSurfaceKind, NcStatus> {
        Ok(RenderSurfaceKind::Framebuffer)
    }
    /// Observes current physical pixels; the view owns no independently resizable attachment.
    fn size(&self) -> Result<Size, NcStatus> {
        self.record.device.drawable_size()
    }
    /// Only the owning window may resize the framebuffer.
    fn resize(&self, _: Size) -> Result<(), NcStatus> {
        Err(NC_E_INVALID_ARG)
    }
    /// A framebuffer view publishes no immutable output; only image surfaces do.
    fn completed_image(&self) -> Result<ComPtr<ITextureAbi>, NcStatus> {
        Err(neocogi_com::NC_E_NOT_FOUND)
    }
}
impl IRenderDriverBound for FramebufferSurface {
    /// Shares device identity without granting access to its window or renderer.
    fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
        Ok(self.record.device.logical.driver_identity())
    }
}

/// Publishes either concrete capability after all descriptor validation and registry reservation.
/// Only images carry extents; they may suspend but must remain bounded.
/// The local canonical identity is registered before return and never inferred from public metadata.
pub(super) fn create(device: &Rc<Device>, desc: RenderSurfaceDesc) -> Result<ComPtr<IRenderSurfaceAbi>, NcStatus> {
    let kind = match desc {
        RenderSurfaceDesc::Framebuffer => RenderSurfaceKind::Framebuffer,
        RenderSurfaceDesc::Image(size) => {
            validate_size(size)?;
            RenderSurfaceKind::Image
        }
    };
    let _access = device.access()?;
    let target = device.logical.surface_target(kind)?;
    device.surfaces.borrow_mut().try_reserve(1).map_err(|_| neocogi_com::NC_E_OUT_OF_MEMORY)?;
    let record = Rc::new(SurfaceRecord {
        device: device.clone(),
        target,
        key: Cell::new(0),
    });
    let surface: ComPtr<IRenderSurfaceAbi> = match desc {
        RenderSurfaceDesc::Framebuffer => FramebufferSurface::from_value(FramebufferSurface { record: record.clone() }).query()?,
        RenderSurfaceDesc::Image(size) => ImageSurface::from_value(ImageSurface {
            record: record.clone(),
            state: RefCell::new(Some(SurfaceState { size, completed: None })),
        })
        .query()?,
    };
    let key = identity(&surface)?;
    record.key.set(key);
    device.surfaces.borrow_mut().insert(key, Rc::downgrade(&record));
    Ok(surface)
}

impl IRenderSurfaceFactory for WindowObject {
    /// Publishes target metadata on this window's device, without acquiring a frame.
    fn create_surface(&self, desc: RenderSurfaceDesc) -> Result<ComPtr<IRenderSurfaceAbi>, NcStatus> {
        create(&self.device, desc)
    }
}
impl IRenderSurfaceFactory for GraphicsHost {
    /// Trusted graphics assembly can select targets without receiving OS/window interfaces.
    fn create_surface(&self, desc: RenderSurfaceDesc) -> Result<ComPtr<IRenderSurfaceAbi>, NcStatus> {
        create(&self.device, desc)
    }
}

impl Device {
    /// Authenticates canonical surface identity, without calling its kind/size/driver metadata methods.
    /// The caller holds device access across this owned parameter's foreign QI and final Release.
    /// A foreign or expired registration returns `RENDER_E_OWNER`, even if public metadata would match.
    pub(super) fn surface(&self, surface: ComPtr<IRenderSurfaceAbi>) -> Result<Rc<SurfaceRecord>, NcStatus> {
        let key = identity(&surface)?;
        let record = self.surfaces.try_borrow().map_err(|_| RENDER_E_BUSY)?.get(&key).and_then(Weak::upgrade);
        record.ok_or(RENDER_E_OWNER)
    }

    /// Window pixel dimensions remain observable without acquiring a second native frame.
    fn drawable_size(&self) -> Result<Size, NcStatus> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Ok(gpu::pixel_size(&self.window))
        }
        #[cfg(target_arch = "wasm32")]
        {
            Ok(Size {
                width: self.window.width() as i32,
                height: self.window.height() as i32,
            })
        }
    }
}

impl IRenderSurface for ImageSurface {
    /// Image target identity is immutable, even while dimensions and completed output change.
    fn kind(&self) -> Result<RenderSurfaceKind, NcStatus> {
        Ok(RenderSurfaceKind::Image)
    }
    /// Reads committed geometry without exposing pending session state.
    fn size(&self) -> Result<Size, NcStatus> {
        Ok(self.state.try_borrow().map_err(|_| RENDER_E_BUSY)?.as_ref().ok_or(RENDER_E_BUSY)?.size)
    }

    /// Changes future frame geometry while retaining every completed image unchanged.
    fn resize(&self, size: Size) -> Result<(), NcStatus> {
        validate_size(size)?;
        let _device = self.record.device.access()?;
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

impl IRenderer for ImageSurface {
    /// Validates all commands, renders to fresh private storage, and then publishes atomically.
    fn render_result(&self, info: RenderInfo, stream: ComRef<'_, IRenderStreamAbi>) -> Result<ComPtr<IRenderOutcomeAbi>, NcStatus> {
        let result = (|| -> Result<(), RenderError> {
            let mut session = self.access()?;
            let session = session.state();
            if info.size != session.size || info.size.width <= 0 || info.size.height <= 0 || info.clear.to_le_bytes()[3] != 255 {
                return Err(NC_E_INVALID_ARG.into());
            }
            let image = self.record.device.execute(|state| {
                let _operation = self.record.device.logical.operation()?;
                state
                    .prepared
                    .read(info, stream, self.record.device.as_ref())
                    .map_err(|status| RenderError::from(status).context("frame preparation"))?;
                // Foreign callbacks finish before the final closure/target checks and allocation.
                for block in state.prepared.blocks() {
                    block.admit_target(&self.record.target)?;
                }
                state
                    .gpu
                    .render_surface(info, &state.prepared)
                    .map_err(|error| self.record.device.failure(error))
            })?;
            session.completed = Some(publish_texture(retain_texture(&self.record.device, image)));
            Ok(())
        })();
        Ok(self.record.device.complete(result, "render image"))
    }
}

impl IRenderDriverBound for ImageSurface {
    /// Authenticates imports and groups against the same physical device as the output.
    fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
        Ok(self.record.device.logical.driver_identity())
    }
}
