//! Authenticated immutable images retain native pixels and physical lifetime, without a driver facade.
use super::*;

/// Native allocation retained independently of its public COM editing/ownership object.
pub(super) struct TextureRecord {
    /// Exact initialized native allocation, without a facade, public COM image, or CPU version.
    pub(super) image: gpu::Image,
    /// Weak provenance table; retaining pixels cannot retain the renderer or event host.
    registry: Weak<RefCell<HashMap<usize, Weak<TextureRecord>>>>,
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
    /// Immutable dimensions come from the actual admitted native allocation.
    fn size(&self) -> Result<Size, NcStatus> {
        Ok(<gpu::Backend as neocogi_render_gpu::native::NativeResources>::image_size(&self.record.image))
    }
}
impl Drop for Texture {
    /// Remove identity before the COM address can be recycled, even if a frame still retains the native image.
    fn drop(&mut self) {
        if let Some(registry) = self.record.registry.upgrade() {
            registry.borrow_mut().remove(&self.record.key.get());
        }
    }
}
impl ITextureFactory for WindowObject {
    /// Validates exact RGBA length through the common image policy, uploads, then publishes one capability.
    fn create_texture_result(
        &self,
        size: Size,
        pixels: neocogi_com::NcValueRef<'_, InitialGpuData>,
    ) -> Result<neocogi_com::ComBox<ITextureCreationResultAbi>, NcStatus> {
        let result = (|| -> Result<_, RenderError> {
            let len = rgba_bytes(size)?;
            let image = self.device.execute(|state| {
                let pixels = neocogi_render_core::source::read_initial(pixels, len)?;
                state.gpu.create_texture(size, pixels.as_deref()).map_err(|error| self.device.failure(error))
            })?;
            Ok(publish_texture(retain_texture(&self.device, image)))
        })();
        Ok(texture_creation_result(result.map_err(|error| error.context("create native texture"))))
    }
}

/// Retains a successfully allocated native image through the existing retirement path.
pub(super) fn retain_texture(device: &Device, image: gpu::Image) -> Rc<TextureRecord> {
    Rc::new(TextureRecord {
        image,
        registry: Rc::downgrade(&device.textures),
        key: Cell::new(0),
    })
}

/// Publishes one local native image only after upload or offscreen rendering succeeds.
pub(super) fn publish_texture(record: Rc<TextureRecord>) -> ComPtr<ITextureAbi> {
    // The identity is queried from our own closed implementation, never foreign code.
    let texture: ComPtr<ITextureAbi> = Texture::from_value(Texture { record: record.clone() })
        .query()
        .expect("Texture declares ITexture");
    let key = identity(&texture).expect("local texture declares canonical IUnknown identity");
    record.key.set(key);
    if let Some(registry) = record.registry.upgrade() {
        registry.borrow_mut().insert(key, Rc::downgrade(&record));
    }
    texture
}

impl Resources for Device {
    type Backend = gpu::Backend;
    /// Authenticates construction-proven generic blocks before any target is acquired.
    fn view(&self, view: neocogi_com::WeakComBox<IRenderViewBlockAbi>) -> Result<Rc<neocogi_render_gpu::record::Block<gpu::Backend>>, NcStatus> {
        self.logical.block_snapshot(view.as_com_ref())
    }
}

/// Window and offscreen sessions authenticate resources in the same concrete device registry.
impl Resources for WindowObject {
    type Backend = gpu::Backend;
    /// Uses construction-proven exclusive identity, never a caller-provided device number.
    fn view(&self, view: neocogi_com::WeakComBox<IRenderViewBlockAbi>) -> Result<Rc<neocogi_render_gpu::record::Block<gpu::Backend>>, NcStatus> {
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

impl IRenderDriverBound for Texture {
    fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
        Ok(
            <gpu::Backend as neocogi_render_gpu::native::NativeResources>::identity(<gpu::Backend as neocogi_render_gpu::native::NativeResources>::image_core(
                &self.record.image,
            ))
            .clone(),
        )
    }
}
