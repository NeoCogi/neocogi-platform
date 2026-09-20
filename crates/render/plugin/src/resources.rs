//! Authenticated image and scene capabilities share the native window/device lifetime.
use super::*;
use rs_math3d::Mat4f;

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

/// Application scene storage contains concrete mesh data, with no native handle passed through COM.
pub(super) struct SceneRecord {
    /// Native lifetime and ownership authority shared with the renderer and its images.
    device: Rc<Device>,
    /// Identity is removed on COM final release, before a native snapshot can outlive a recycled address.
    key: Cell<usize>,
    /// Mesh and matrices are atomically replaced and cheaply copied into prepared commands.
    submission: RefCell<MeshSubmission>,
}
#[com_object(interfaces(IMeshScene))]
/// One COM allocation exposes IMeshScene and its inherited IRenderItem capability.
struct Scene {
    /// Immutable mesh allocations are shared with native frame commands through Arc.
    record: Rc<SceneRecord>,
}
impl SceneRecord {
    /// Editing during preparation/execution is rejected so one complete frame uses a coherent scene state.
    fn writable(&self) -> Result<(), NcStatus> {
        if self.device.render.try_borrow().map_err(|_| RENDER_E_BUSY)?.is_none() {
            Err(RENDER_E_BUSY)
        } else {
            Ok(())
        }
    }
}
impl IRenderItem for Scene {}
impl IMeshScene for Scene {
    /// Checks the complete candidate before changing any live mesh allocation.
    fn set_mesh(&self, vertices: NcCopyFromBufferOffset<'_, MeshVertex>, indices: NcCopyFromBufferOffset<'_, u32>) -> Result<(), NcStatus> {
        self.record.writable()?;
        let (source_vertices, source_indices) = (vertices.as_slice(), indices.as_slice());
        if vertices.offset() != 0
            || indices.offset() != 0
            || source_vertices.len() > 4 * 1024 * 1024
            || source_indices.len() > 12 * 1024 * 1024
            || !source_indices.len().is_multiple_of(3)
            || source_vertices.is_empty() != source_indices.is_empty()
            || source_indices.iter().any(|index| *index as usize >= source_vertices.len())
            || source_vertices.iter().any(|v| {
                [v.position.x, v.position.y, v.position.z, v.normal.x, v.normal.y, v.normal.z, v.uv.x, v.uv.y]
                    .iter()
                    .any(|x| !x.is_finite())
            })
        {
            return Err(NC_E_INVALID_ARG);
        }
        let mut owned_vertices = Vec::new();
        let mut owned_indices = Vec::new();
        owned_vertices
            .try_reserve_exact(source_vertices.len())
            .map_err(|_| neocogi_com::NC_E_OUT_OF_MEMORY)?;
        owned_indices
            .try_reserve_exact(source_indices.len())
            .map_err(|_| neocogi_com::NC_E_OUT_OF_MEMORY)?;
        owned_vertices.extend_from_slice(source_vertices);
        owned_indices.extend_from_slice(source_indices);
        self.record.submission.borrow_mut().mesh = MeshBuffers::from_vecs(owned_vertices, owned_indices);
        Ok(())
    }
    /// Matrix replacement is one copied transaction and never changes the retained widget's geometry.
    fn set_transforms(&self, transforms: SceneTransforms) -> Result<(), NcStatus> {
        self.record.writable()?;
        // The contract owns its portable matrix layout. Convert once at this
        // implementation boundary so renderer internals remain native math values.
        let pvm: Mat4f = transforms.pvm.into();
        let view_model: Mat4f = transforms.view_model.into();
        if pvm
            .col
            .iter()
            .chain(&view_model.col)
            .any(|v| [v.x, v.y, v.z, v.w].iter().any(|x| !x.is_finite()))
        {
            return Err(NC_E_INVALID_ARG);
        }
        let mut submission = self.record.submission.borrow_mut();
        submission.pvm = pvm;
        submission.view_model = view_model;
        Ok(())
    }
}
impl Drop for Scene {
    /// Remove the COM identity even when immutable command snapshots remain alive in a native frame.
    fn drop(&mut self) {
        self.record.device.scenes.borrow_mut().remove(&self.record.key.get());
    }
}
impl IDemoScenes for WindowObject {
    /// Creates the actual empty application scene in this renderer's ownership domain.
    fn create_scene(&self) -> Result<ComPtr<IMeshSceneAbi>, NcStatus> {
        let _access = self.device.access()?;
        let record = Rc::new(SceneRecord {
            device: self.device.clone(),
            key: Cell::new(0),
            submission: RefCell::new(MeshSubmission {
                mesh: MeshBuffers::from_vecs(Vec::new(), Vec::new()),
                pvm: Mat4f::identity(),
                view_model: Mat4f::identity(),
            }),
        });
        let scene: ComPtr<IMeshSceneAbi> = Scene::from_value(Scene { record: record.clone() }).query().expect("Scene declares IMeshScene");
        let key = identity(&scene)?;
        record.key.set(key);
        self.device.scenes.borrow_mut().insert(key, Rc::downgrade(&record));
        Ok(scene)
    }
}
impl Resources for Device {
    type Texture = Rc<TextureRecord>;
    type Item = MeshSubmission;
    /// Authentication never calls a resource-reported owner getter and never downcasts erased data.
    fn texture(&self, texture: ComPtr<ITextureAbi>) -> Result<Self::Texture, NcStatus> {
        let key = identity(&texture)?;
        let registry = self.textures.try_borrow().map_err(|_| RENDER_E_BUSY)?;
        let resolved = registry.get(&key).and_then(Weak::upgrade);
        drop(registry);
        resolved.ok_or(RENDER_E_OWNER)
    }
    /// Snapshots the same application data used by its concrete native renderer, before a GPU frame exists.
    fn item(&self, item: ComPtr<IRenderItemAbi>) -> Result<Self::Item, NcStatus> {
        let key = identity(&item)?;
        let scene = self
            .scenes
            .try_borrow()
            .map_err(|_| RENDER_E_BUSY)?
            .get(&key)
            .and_then(Weak::upgrade)
            .ok_or(RENDER_E_OWNER)?;
        Ok(scene.submission.borrow().clone())
    }
}

/// Window and offscreen sessions authenticate resources in the same concrete device registry.
impl Resources for WindowObject {
    type Texture = Rc<TextureRecord>;
    type Item = MeshSubmission;

    /// Delegates authentication without exposing or trusting a caller-provided device identifier.
    fn texture(&self, texture: ComPtr<ITextureAbi>) -> Result<Self::Texture, NcStatus> {
        self.device.texture(texture)
    }

    /// Retains the existing native-window application scene support.
    fn item(&self, item: ComPtr<IRenderItemAbi>) -> Result<Self::Item, NcStatus> {
        self.device.item(item)
    }
}
