//! Authenticated image capabilities share the native window/device lifetime.
use super::*;

/// Native allocation retained independently of its public COM editing/ownership object.
pub(super) struct TextureRecord {
    /// Actual backend map key; never accepted from foreign code as proof of ownership.
    pub(super) id: TextureId,
    /// Native window and context must outlive deletion of this allocation.
    device: Rc<Device>,
    /// Registered canonical COM identity, installed before publication and removed on COM final release.
    key: Cell<usize>,
}
#[com_object(interfaces(ITexture))]
/// Immutable image capability; one allocation has one canonical identity in its native registry.
struct Texture {
    /// A prepared draw can keep the native image alive after this COM allocation is released.
    record: Rc<TextureRecord>,
}
impl ITexture for Texture {
    /// Immutable dimensions come from the same validated key used by the native backend upload.
    fn size(&self) -> Result<Size, NcStatus> {
        Ok(Size {
            width: self.record.id.width(),
            height: self.record.id.height(),
        })
    }
}
impl Drop for Texture {
    /// Remove identity before the COM address can be recycled, even if a frame still retains the native image.
    fn drop(&mut self) {
        self.record.device.textures.borrow_mut().remove(&self.record.key.get());
    }
}
impl Drop for TextureRecord {
    /// A final release during preparation queues deletion until the ownership transaction exits.
    fn drop(&mut self) {
        self.device.retire(self.id);
    }
}
impl ITextureFactory for WindowObject {
    /// Validates exact RGBA length through the common image policy, uploads, then publishes one capability.
    fn create_texture(&self, size: Size, pixels: NcCopyFromBufferOffset<'_, u8>) -> Result<ComPtr<ITextureAbi>, NcStatus> {
        if pixels.offset() != 0 {
            return Err(NC_E_INVALID_ARG);
        }
        let id = TextureId::for_upload(size.width, size.height, pixels.as_slice()).map_err(|_| NC_E_INVALID_ARG)?;
        let mut access = self.device.access()?;
        access
            .state()
            .gpu
            .create_texture(id, pixels.as_slice())
            .map_err(|error| self.device.failure(error))?;
        Ok(publish_texture(retain_texture(self.device.clone(), id)))
    }
}

/// Retains a successfully allocated native image through the existing retirement path.
pub(super) fn retain_texture(device: Rc<Device>, id: TextureId) -> Rc<TextureRecord> {
    Rc::new(TextureRecord { id, device, key: Cell::new(0) })
}

/// Publishes one local native image only after upload or offscreen rendering succeeds.
pub(super) fn publish_texture(record: Rc<TextureRecord>) -> ComPtr<ITextureAbi> {
    // The identity is queried from our own closed implementation, never foreign code.
    let texture: ComPtr<ITextureAbi> = Texture::from_value(Texture { record: record.clone() })
        .query()
        .expect("Texture declares ITexture");
    let key = identity(&texture).expect("local texture declares canonical IUnknown identity");
    record.key.set(key);
    record.device.textures.borrow_mut().insert(key, Rc::downgrade(&record));
    texture
}

impl Resources for Device {
    /// Authenticates construction-proven generic blocks before any target is acquired.
    fn view(&self, view: neocogi_com::WeakComBox<IRenderViewBlockAbi>) -> Result<Rc<neocogi_render_gpu::record::Block>, NcStatus> {
        self.logical.block_snapshot(view.as_com_ref())
    }
}

/// Window and offscreen sessions authenticate resources in the same concrete device registry.
impl Resources for WindowObject {
    /// Uses construction-proven exclusive identity, never a caller-provided device number.
    fn view(&self, view: neocogi_com::WeakComBox<IRenderViewBlockAbi>) -> Result<Rc<neocogi_render_gpu::record::Block>, NcStatus> {
        self.device.logical.block_snapshot(view.as_com_ref())
    }
}

impl Device {
    /// Resolve ordinary immutable image identity only at trusted UI import, never during native execution.
    pub(super) fn texture(&self, texture: ComPtr<ITextureAbi>) -> Result<Rc<TextureRecord>, NcStatus> {
        let key = identity(&texture)?;
        let registry = self.textures.try_borrow().map_err(|_| RENDER_E_BUSY)?;
        let resolved = registry.get(&key).and_then(Weak::upgrade);
        drop(registry);
        resolved.ok_or(RENDER_E_OWNER)
    }
}
