//! Real generated COM calls exercise quota, provenance, reentry and owner/version lifetime.
use super::*;
use neocogi_com::{ComInlineValue, NcOutputWriter};

/// Deliberate provider behavior at a foreign-source boundary.
enum ReadMode {
    /// Initialize every requested byte.
    Exact,
    /// Return success with an incomplete initialized prefix.
    Short,
    /// Return a normal provider error.
    Fail,
    /// Change the total header between admission and final validation.
    Changing,
    /// Close the domain during a foreign read.
    Close(WeakComBox<IRenderClientDomainAbi>),
    /// Destroy a resource owner while an update is copying outside its local gate.
    DropBuffer(Rc<RefCell<Option<ComBox<IGpuBufferAbi>>>>),
    /// Attempt factory reentry while the outer source transaction is detached.
    Reenter(ComPtr<IRenderResourcesAbi>, Rc<Cell<Option<NcStatus>>>),
}

#[com_object(interfaces(IGpuDataSource))]
/// Typed CPU provider; all probes use ordinary valid output storage.
struct Bytes {
    /// Producer-owned immutable input bytes.
    bytes: Vec<u8>,
    /// Which adversarial boundary this source exercises.
    mode: ReadMode,
    /// Header-call count proves changing metadata is checked.
    headers: Cell<u32>,
    /// Bulk-call count proves quota rejection precedes copying.
    reads: Rc<Cell<usize>>,
    /// Maximum requested capacity proves reads remain bounded.
    largest: Rc<Cell<usize>>,
}
impl IGpuDataSource for Bytes {
    /// Header access is intentionally independent from output prefix publication.
    fn byte_count(&self) -> Result<u64, NcStatus> {
        let previous = self.headers.replace(self.headers.get() + 1);
        Ok(self.bytes.len() as u64 + u64::from(matches!(self.mode, ReadMode::Changing) && previous > 0))
    }
    /// Provider callbacks run before initialization, while the renderer holds no resource gate.
    fn read(&self, mut output: NcOutputWriter<'_, u8>) -> Result<(), NcStatus> {
        self.reads.set(self.reads.get() + 1);
        self.largest.set(self.largest.get().max(output.capacity()));
        match &self.mode {
            ReadMode::Fail => return Err(NC_E_INVALID_ARG),
            ReadMode::Close(controller) => controller.clone().close()?,
            ReadMode::DropBuffer(owner) => {
                let removed = owner.borrow_mut().take();
                drop(removed);
            }
            ReadMode::Reenter(factory, result) => {
                result.set(Some(factory.create_sampler(GpuSamplerDesc::default()).unwrap_err()));
            }
            ReadMode::Exact | ReadMode::Short | ReadMode::Changing => {}
        }
        let start = usize::try_from(output.offset()).map_err(|_| NC_E_OUT_OF_RANGE)?;
        let count = if matches!(self.mode, ReadMode::Short) {
            output.capacity().saturating_sub(1)
        } else {
            output.capacity()
        };
        let end = start.checked_add(count).ok_or(NC_E_OUT_OF_RANGE)?;
        output.extend_from_slice(self.bytes.get(start..end).ok_or(NC_E_OUT_OF_RANGE)?)
    }
}

/// Builds a real provider plus independent observations of its bulk-read behavior.
fn source(bytes: Vec<u8>, mode: ReadMode) -> (ComPtr<IGpuDataSourceAbi>, Rc<Cell<usize>>, Rc<Cell<usize>>) {
    let reads = Rc::new(Cell::new(0));
    let largest = Rc::new(Cell::new(0));
    let value = Bytes::from_value(Bytes {
        bytes,
        mode,
        headers: Cell::new(0),
        reads: reads.clone(),
        largest: largest.clone(),
    });
    (value.query().unwrap(), reads, largest)
}

/// Keeps the exclusive host controller alive while handing a shareable factory to test clients.
fn client(device: &GpuDevice, limits: RenderClientLimits) -> (ComBox<IRenderClientDomainAbi>, ComPtr<IRenderResourcesAbi>) {
    let owner = device.create_client(limits).unwrap();
    let factory = owner.resources().unwrap();
    (owner, factory)
}

#[com_object(interfaces(ITexture))]
/// Imported-image lease reports its release independently of private native accounting.
struct ImageLease {
    /// Local drop observation contains no native device or domain edge.
    dropped: Rc<Cell<u32>>,
}
impl neocogi_render_core::abi::IRenderDriverBound for ImageLease {
    fn driver_identity(&self) -> Result<neocogi_com::ComPtr<neocogi_render_core::abi::IRenderDriverIdentityAbi>, neocogi_com::NcStatus> {
        Err(neocogi_com::NC_E_NOINTERFACE)
    }
}
impl ITexture for ImageLease {
    /// Trusted native assembly already knows the dimensions; the bridge must not consult this getter.
    fn size(&self) -> Result<Size, NcStatus> {
        panic!("trusted native keys establish image dimensions")
    }
}
impl Drop for ImageLease {
    /// A native receipt must not keep this COM capability or its hypothetical device facade alive.
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}

/// UI import is host-only, immutable, same-domain authenticated and stripped before native retention.
#[test]
fn ui_image_import_separates_com_lifetime_from_native_byte_receipts() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_client, ordinary) = client(&device, RenderClientLimits::default());
    let ui = device.create_ui_client(RenderClientLimits::default(), None).unwrap();
    let factory = ui.resources().unwrap();
    let dropped = Rc::new(Cell::new(0));
    let image: ComPtr<ITextureAbi> = ImageLease::from_value(ImageLease { dropped: dropped.clone() }).query().unwrap();
    let id = TextureId::for_upload(1, 1, &[255; 4]).unwrap();
    assert!(matches!(
        device.import_ui_texture(ordinary.as_com_ref(), id, image.clone()),
        Err(RENDER_E_OWNER)
    ));
    let imported = device.import_ui_texture(factory.as_com_ref(), id, image.clone()).unwrap();
    assert!(imported.query_ref::<ITextureAbi>().is_err());
    assert!(imported.query_ref::<IRenderHostAbi>().is_err());
    assert!(matches!(
        device.ui_texture_snapshot(ordinary.as_com_ref(), imported.as_com_ref()),
        Err(RENDER_E_OWNER)
    ));
    let snapshot = device.ui_texture_snapshot(factory.as_com_ref(), imported.as_com_ref()).unwrap();
    assert_eq!(snapshot.imported_id(), Some(id));
    assert!(snapshot.pixels().is_none());
    let (update, _, _) = source(vec![0; 4], ReadMode::Exact);
    assert_eq!(
        factory.update_texture(imported.as_com_ref(), Rect { x: 0, y: 0, width: 1, height: 1 }, update.as_com_ref()),
        Err(NC_E_INVALID_ARG)
    );
    let receipt = snapshot.receipt();
    drop((image, imported, snapshot));
    assert_eq!(dropped.get(), 1, "native accounting must not retain the imported COM/device lease");
    assert!(!receipt.is_alive());
    assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 4);
    drop(receipt);
    assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 0);
}

/// Complete buffer descriptor for a supplied byte payload.
fn buffer(factory: &ComPtr<IRenderResourcesAbi>, bytes: &[u8]) -> ComBox<IGpuBufferAbi> {
    let (source, _, _) = source(bytes.to_vec(), ReadMode::Exact);
    factory
        .create_buffer(
            GpuBufferDesc {
                role: GpuBufferRole::Vertex,
                byte_len: bytes.len() as u64,
            },
            neocogi_render_core::source::supplied(source.as_com_ref()).unwrap().borrow().unwrap(),
        )
        .unwrap()
}

/// Large byte payloads avoid semantic-vertex codec limits and use bounded output chunks.
#[test]
fn exclusive_buffer_creation_reads_bounded_chunks_and_exposes_no_renderer() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_owner, factory) = client(&device, RenderClientLimits::default());
    let bytes: Vec<_> = (0..SOURCE_CHUNK * 2 + 17).map(|n| n as u8).collect();
    let (source, reads, largest) = source(bytes.clone(), ReadMode::Exact);
    let owner = factory
        .create_buffer(
            GpuBufferDesc {
                role: GpuBufferRole::Vertex,
                byte_len: bytes.len() as u64,
            },
            neocogi_render_core::source::supplied(source.as_com_ref()).unwrap().borrow().unwrap(),
        )
        .unwrap();
    assert_eq!(reads.get(), 3);
    assert_eq!(largest.get(), SOURCE_CHUNK);
    assert!(owner.as_com_ref().try_add_ref().is_err());
    assert!(owner.query_ref::<IRendererAbi>().is_err());
    assert!(owner.query_ref::<IRenderResourcesAbi>().is_err());
    assert!(factory.query::<IRenderClientDomainAbi>().is_err());
    assert_eq!(
        device
            .buffer_snapshot(factory.as_com_ref(), owner.as_com_ref())
            .unwrap()
            .bytes
            .as_slice()
            .unwrap(),
        bytes
    );
}

/// No public owner promotion occurs; captured A/B versions determine actual byte retirement.
#[test]
fn weak_expiry_does_not_invalidate_snapshots_or_refund_retained_versions() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let owner = buffer(&factory, &[1, 2, 3, 4]);
    let weak = owner.downgrade();
    let a = device.buffer_snapshot(factory.as_com_ref(), owner.as_com_ref()).unwrap();
    let (update, _, _) = source(vec![9, 8], ReadMode::Exact);
    factory.update_buffer(owner.as_com_ref(), 1, update.as_com_ref()).unwrap();
    let b = device.buffer_snapshot(factory.as_com_ref(), weak.as_com_ref()).unwrap();
    assert_eq!(a.bytes.as_slice().unwrap(), [1, 2, 3, 4]);
    assert_eq!(b.bytes.as_slice().unwrap(), [1, 9, 8, 4]);
    assert_ne!(a.id, b.id);
    assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 8);
    drop(owner);
    assert!(!weak.is_alive());
    assert!(weak.query::<IGpuBufferAbi>().is_ok()); // Metadata is deliberately still available.
    assert!(matches!(device.buffer_snapshot(factory.as_com_ref(), weak.as_com_ref()), Err(NC_E_DROPPED)));
    assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 8);
    drop(a);
    assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 4);
    drop(b);
    assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 0);
}

/// Owner/resource limits reject before foreign bulk reads, and controller clones cannot reset them.
#[test]
fn quota_admission_precedes_bulk_copy_and_retained_versions_keep_their_charge() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let limits = RenderClientLimits {
        resources: 1,
        live_bytes: 8,
        ..RenderClientLimits::default()
    };
    let (_controller, factory) = client(&device, limits);
    let owner = buffer(&factory, &[1, 2, 3, 4]);
    let (input, reads, _) = source(vec![5, 6, 7, 8], ReadMode::Exact);
    assert!(matches!(
        factory.clone().create_buffer(
            GpuBufferDesc { role: GpuBufferRole::Index, byte_len: 4 },
            neocogi_render_core::source::supplied(input.as_com_ref()).unwrap().borrow().unwrap()
        ),
        Err(NC_E_OUT_OF_RANGE)
    ));
    assert_eq!(reads.get(), 0);
    let a = device.buffer_snapshot(factory.as_com_ref(), owner.as_com_ref()).unwrap();
    factory.update_buffer(owner.as_com_ref(), 0, input.as_com_ref()).unwrap();
    let b = device.buffer_snapshot(factory.as_com_ref(), owner.as_com_ref()).unwrap();
    reads.set(0);
    assert!(matches!(
        factory.update_buffer(owner.as_com_ref(), 0, input.as_com_ref()),
        Err(NC_E_OUT_OF_RANGE)
    ));
    assert_eq!(reads.get(), 0);
    drop(a);
    drop(b);
    factory.update_buffer(owner.as_com_ref(), 0, input.as_com_ref()).unwrap();
}

/// Short prefixes/errors/changing headers roll back the entire unpublished candidate.
#[test]
fn failed_sources_preserve_current_buffer_and_release_candidate_charges() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let owner = buffer(&factory, &[1, 2, 3, 4]);
    let a = device.buffer_snapshot(factory.as_com_ref(), owner.as_com_ref()).unwrap();
    for mode in [ReadMode::Short, ReadMode::Fail, ReadMode::Changing] {
        let (input, _, _) = source(vec![9; 4], mode);
        assert!(factory.update_buffer(owner.as_com_ref(), 0, input.as_com_ref()).is_err());
        let after = device.buffer_snapshot(factory.as_com_ref(), owner.as_com_ref()).unwrap();
        assert_eq!(after.id, a.id);
        assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 4);
    }
}

/// Identical interface shapes cannot substitute another renderer's or domain's registered object.
#[test]
fn different_domains_devices_and_forged_buffers_reject() {
    #[com_object(interfaces(IGpuBuffer), ownership = exclusive)]
    /// Valid foreign COM implementation with no renderer provenance.
    struct Forged;
    impl neocogi_render_core::abi::IRenderDriverBound for Forged {
        fn driver_identity(&self) -> Result<neocogi_com::ComPtr<neocogi_render_core::abi::IRenderDriverIdentityAbi>, neocogi_com::NcStatus> {
            Err(neocogi_com::NC_E_NOINTERFACE)
        }
    }
    impl IGpuBuffer for Forged {}
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let other = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_a, factory) = client(&device, RenderClientLimits::default());
    let (_b, foreign_factory) = client(&other, RenderClientLimits::default());
    let (_c, domain_factory) = client(&device, RenderClientLimits::default());
    let owner = buffer(&factory, &[0; 4]);
    let forged: ComBox<IGpuBufferAbi> = Forged::into_box(Forged).query().unwrap();
    let (input, _, _) = source(vec![1; 4], ReadMode::Exact);
    assert!(matches!(
        foreign_factory.update_buffer(owner.as_com_ref(), 0, input.as_com_ref()),
        Err(RENDER_E_OWNER)
    ));
    assert!(matches!(
        domain_factory.update_buffer(owner.as_com_ref(), 0, input.as_com_ref()),
        Err(RENDER_E_OWNER)
    ));
    assert!(matches!(factory.update_buffer(forged.as_com_ref(), 0, input.as_com_ref()), Err(RENDER_E_OWNER)));
}

/// Closing can happen inside a source callback; publication rechecks it with no borrowed account.
#[test]
fn close_during_source_read_rejects_publication_and_drop_closes_shared_factories() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (controller, factory) = client(&device, RenderClientLimits::default());
    let account = device.domain(factory.as_com_ref()).unwrap().account.clone();
    let (input, _, _) = source(vec![1; 4], ReadMode::Close(controller.downgrade()));
    assert!(matches!(
        factory.create_buffer(
            GpuBufferDesc { role: GpuBufferRole::Vertex, byte_len: 4 },
            neocogi_render_core::source::supplied(input.as_com_ref()).unwrap().borrow().unwrap()
        ),
        Err(NC_E_DROPPED)
    ));
    assert_eq!(account.live_bytes(), 0);
    let clone = factory.clone();
    drop(controller);
    assert!(matches!(clone.create_sampler(GpuSamplerDesc::default()), Err(NC_E_DROPPED)));
    // Operation ownership was restored even though the original domain closed.
    let (_next, next_factory) = client(&device, RenderClientLimits::default());
    next_factory.create_sampler(GpuSamplerDesc::default()).unwrap();
}

/// The resource reader/writer gate is never retained through a foreign read that removes its owner.
#[test]
fn owner_removal_during_update_rejects_final_commit_without_pinning_public_value() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let owner = buffer(&factory, &[1; 4]);
    let weak = owner.downgrade();
    let captured = device.buffer_snapshot(factory.as_com_ref(), weak.as_com_ref()).unwrap();
    let owner = Rc::new(RefCell::new(Some(owner)));
    let (input, _, _) = source(vec![2; 4], ReadMode::DropBuffer(owner.clone()));
    assert!(matches!(factory.update_buffer(weak.as_com_ref(), 0, input.as_com_ref()), Err(NC_E_DROPPED)));
    assert!(owner.borrow().is_none());
    assert!(!weak.is_alive());
    assert_eq!(captured.bytes.as_slice().unwrap(), [1; 4]);
    assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 4);
}

/// CPU input methods cannot borrow native/device preparation state recursively.
#[test]
fn factory_reentry_returns_busy_and_the_outer_copy_remains_valid() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let status = Rc::new(Cell::new(None));
    let (input, _, _) = source(vec![7; 4], ReadMode::Reenter(factory.clone(), status.clone()));
    let owner = factory
        .create_buffer(
            GpuBufferDesc { role: GpuBufferRole::Vertex, byte_len: 4 },
            neocogi_render_core::source::supplied(input.as_com_ref()).unwrap().borrow().unwrap(),
        )
        .unwrap();
    assert_eq!(status.get(), Some(RENDER_E_BUSY));
    assert_eq!(
        device
            .buffer_snapshot(factory.as_com_ref(), owner.as_com_ref())
            .unwrap()
            .bytes
            .as_slice()
            .unwrap(),
        [7; 4]
    );
}

/// Texture patches can cross output-chunk and pixel-row boundaries without touching other texels.
#[test]
fn texture_update_captures_old_pixels_and_copies_only_the_admitted_patch() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let (pixels, _, _) = source(vec![7; 256 * 256 * 4], ReadMode::Exact);
    let owner = factory
        .create_texture(
            GpuTextureDesc {
                size: Size { width: 256, height: 256 },
                format: GpuTextureFormat::Rgba8Unorm,
                mip_count: 1,
            },
            neocogi_render_core::source::supplied(pixels.as_com_ref()).unwrap().borrow().unwrap(),
        )
        .unwrap();
    assert!(owner.query_ref::<ITextureAbi>().is_err());
    let a = device.texture_snapshot(factory.as_com_ref(), owner.as_com_ref()).unwrap();
    let patch: Vec<_> = (0..201 * 201 * 4).map(|n| n as u8).collect();
    let (input, reads, largest) = source(patch.clone(), ReadMode::Exact);
    factory
        .update_texture(owner.as_com_ref(), Rect { x: 5, y: 7, width: 201, height: 201 }, input.as_com_ref())
        .unwrap();
    let b = device.texture_snapshot(factory.as_com_ref(), owner.as_com_ref()).unwrap();
    let mut expected = vec![7; 256 * 256 * 4];
    for row in 0..201 {
        expected[((row + 7) * 256 + 5) * 4..((row + 7) * 256 + 206) * 4].copy_from_slice(&patch[row * 201 * 4..(row + 1) * 201 * 4]);
    }
    assert_eq!(b.pixels().unwrap().as_slice().unwrap(), expected);
    assert!(a.pixels().unwrap().as_slice().unwrap().iter().all(|byte| *byte == 7));
    assert_eq!(reads.get(), 3);
    assert_eq!(largest.get(), SOURCE_CHUNK);
}

/// All pointer-free descriptors still reject unknown open-enum and inconsistent size values.
#[test]
fn malformed_descriptors_and_overflowing_updates_reject_before_bulk_reads() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let (input, reads, _) = source(vec![1; 4], ReadMode::Exact);
    assert!(
        factory
            .create_buffer(
                GpuBufferDesc {
                    role: GpuBufferRole::from_raw(255),
                    byte_len: 4
                },
                neocogi_render_core::source::supplied(input.as_com_ref()).unwrap().borrow().unwrap()
            )
            .is_err()
    );
    assert!(
        factory
            .create_texture(
                GpuTextureDesc {
                    size: Size { width: i32::MAX, height: i32::MAX },
                    format: GpuTextureFormat::Rgba8Unorm,
                    mip_count: 1
                },
                neocogi_render_core::source::supplied(input.as_com_ref()).unwrap().borrow().unwrap()
            )
            .is_err()
    );
    assert!(
        factory
            .create_sampler(GpuSamplerDesc {
                min_filter: GpuFilter::from_raw(255),
                ..GpuSamplerDesc::default()
            })
            .is_err()
    );
    assert_eq!(reads.get(), 0);
    let owner = buffer(&factory, &[0; 4]);
    assert!(factory.update_buffer(owner.as_com_ref(), u64::MAX, input.as_com_ref()).is_err());
    assert_eq!(reads.get(), 0);
}

/// Weak registry headers and device-free version accounts cannot keep a native host lease alive.
#[test]
fn native_lifetime_token_retires_independently_from_cached_cpu_versions() {
    #[com_object(interfaces(IRenderDeviceLease))]
    /// A closed marker representing host native lifetime, with no getters or renderer QI.
    struct Lease {
        /// Destruction receipt is owned by the test, not by any renderer object.
        dropped: Rc<Cell<bool>>,
    }
    impl IRenderDeviceLease for Lease {}
    impl Drop for Lease {
        /// Models native host release without introducing an execution callback.
        fn drop(&mut self) {
            self.dropped.set(true);
        }
    }
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let dropped = Rc::new(Cell::new(false));
    let lease: ComPtr<IRenderDeviceLeaseAbi> = Lease::from_value(Lease { dropped: dropped.clone() }).query().unwrap();
    let controller = device.create_client_with_lease(RenderClientLimits::default(), lease).unwrap();
    let factory = controller.resources().unwrap();
    let owner = factory.create_sampler(GpuSamplerDesc::default()).unwrap();
    let weak = owner.downgrade();
    let captured = device.sampler_snapshot(factory.as_com_ref(), weak.as_com_ref()).unwrap();
    drop(controller);
    assert!(!dropped.get()); // Remaining public factory/resource intentionally retain the domain.
    drop(factory);
    drop(owner);
    assert!(!weak.is_alive());
    assert!(dropped.get()); // Only weak entries and device-free CPU backing remain.
    assert_eq!(captured.desc.min_filter, GpuFilter::Nearest);
}

#[com_object(interfaces(IGpuPipelineSource))]
/// Complete independently callable pipeline descriptor source.
struct PipelineSource {
    /// Exact copied header.
    info: GpuPipelineInfo,
    /// Explicit stream layouts.
    layouts: Vec<GpuVertexBufferLayout>,
    /// Explicit location footprints.
    attributes: Vec<GpuVertexAttribute>,
    /// Reflected stage resource expectations.
    bindings: Vec<GpuShaderBinding>,
}
/// Uses the output protocol rather than returning caller-owned vectors across COM.
fn copy<T: ComInlineValue>(values: &[T], mut output: NcOutputWriter<'_, T>) -> Result<(), NcStatus> {
    let start = usize::try_from(output.offset()).map_err(|_| NC_E_OUT_OF_RANGE)?;
    let end = start.checked_add(output.capacity()).ok_or(NC_E_OUT_OF_RANGE)?;
    output.extend_from_slice(values.get(start..end).ok_or(NC_E_OUT_OF_RANGE)?)
}
impl IGpuPipelineSource for PipelineSource {
    /// Header is immutable across all descriptor callbacks.
    fn info(&self) -> Result<GpuPipelineInfo, NcStatus> {
        Ok(self.info)
    }
    /// Copies the requested stream prefix exactly.
    fn vertex_buffers(&self, output: NcOutputWriter<'_, GpuVertexBufferLayout>) -> Result<(), NcStatus> {
        copy(&self.layouts, output)
    }
    /// Copies the requested location prefix exactly.
    fn attributes(&self, output: NcOutputWriter<'_, GpuVertexAttribute>) -> Result<(), NcStatus> {
        copy(&self.attributes, output)
    }
    /// Copies the requested binding prefix exactly.
    fn bindings(&self, output: NcOutputWriter<'_, GpuShaderBinding>) -> Result<(), NcStatus> {
        copy(&self.bindings, output)
    }
}

/// Shader source used by a generic float3 position pipeline, with no scene object.
const GRAPHICS: &str = r#"
@vertex fn vs(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> { return vec4<f32>(position, 1.0); }
@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(0.2, 0.4, 0.8, 1.0); }
"#;

/// Creates an actual typed shader owner through the public factory ABI.
fn shader(factory: &ComPtr<IRenderResourcesAbi>, stage: GpuShaderStage, entry: &[u8]) -> ComBox<IGpuShaderAbi> {
    let (code, _, _) = source(GRAPHICS.as_bytes().to_vec(), ReadMode::Exact);
    factory.create_shader(stage, code.as_com_ref(), NcCopyFromBufferOffset::new(0, entry)).unwrap()
}

/// The native pipeline owns immutable artifacts, not shared promotions of its shader inputs.
#[test]
fn pipeline_retains_shader_artifacts_after_exclusive_shader_owners_expire() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let vertex = shader(&factory, GpuShaderStage::Vertex, b"vs");
    let fragment = shader(&factory, GpuShaderStage::Fragment, b"fs");
    let weak_vertex = vertex.downgrade();
    let layout: ComPtr<IGpuPipelineSourceAbi> = PipelineSource::from_value(PipelineSource {
        info: GpuPipelineInfo {
            vertex_buffer_count: 1,
            attribute_count: 1,
            ..GpuPipelineInfo::default()
        },
        layouts: vec![GpuVertexBufferLayout {
            slot: 0,
            stride: 12,
            step: GpuStepMode::Vertex,
        }],
        attributes: vec![GpuVertexAttribute {
            location: 0,
            slot: 0,
            format: GpuVertexFormat::Float32x3,
            offset: 0,
        }],
        bindings: Vec::new(),
    })
    .query()
    .unwrap();
    let pipeline = factory
        .create_pipeline(layout.as_com_ref(), vertex.as_com_ref(), fragment.as_com_ref())
        .unwrap();
    drop(vertex);
    drop(fragment);
    assert!(!weak_vertex.is_alive());
    let captured = device.pipeline_snapshot(factory.as_com_ref(), pipeline.as_com_ref()).unwrap();
    assert_eq!(captured.vertex.artifact.entry, "vs");
    assert_eq!(captured.fragment.artifact.entry, "fs");
    assert_eq!(captured.attributes[0].format, GpuVertexFormat::Float32x3);
    assert_eq!(captured.vertex.artifact.spirv[0], 0x0723_0203);
    assert!(
        factory
            .create_pipeline(layout.as_com_ref(), weak_vertex.as_com_ref(), weak_vertex.as_com_ref())
            .is_err()
    );
}

/// Correctly typed but inconsistent descriptors cannot defer validation to a native driver.
#[test]
fn incompatible_attribute_footprints_and_shader_shapes_reject() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let vertex = shader(&factory, GpuShaderStage::Vertex, b"vs");
    let fragment = shader(&factory, GpuShaderStage::Fragment, b"fs");
    for (stride, format, offset) in [
        (8, GpuVertexFormat::Float32x3, 0),
        (12, GpuVertexFormat::Uint32x3, 0),
        (12, GpuVertexFormat::Float32x3, 1),
        (256, GpuVertexFormat::Float32x3, 0),
    ] {
        let layout: ComPtr<IGpuPipelineSourceAbi> = PipelineSource::from_value(PipelineSource {
            info: GpuPipelineInfo {
                vertex_buffer_count: 1,
                attribute_count: 1,
                ..GpuPipelineInfo::default()
            },
            layouts: vec![GpuVertexBufferLayout {
                slot: 0,
                stride,
                step: GpuStepMode::Vertex,
            }],
            attributes: vec![GpuVertexAttribute { location: 0, slot: 0, format, offset }],
            bindings: Vec::new(),
        })
        .query()
        .unwrap();
        assert!(
            factory
                .create_pipeline(layout.as_com_ref(), vertex.as_com_ref(), fragment.as_com_ref())
                .is_err()
        );
    }
}

/// Even byte-normalized four-channel formats require portable four-byte attribute alignment.
#[test]
fn packed_color_footprint_uses_webgpu_alignment() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    let code = r#"
@vertex fn vs(@location(0) color: vec4<f32>) -> @builtin(position) vec4<f32> { return vec4<f32>(color.xy, 0.0, 1.0); }
@fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }
"#;
    let (input, _, _) = source(code.as_bytes().to_vec(), ReadMode::Exact);
    let vertex = factory
        .create_shader(GpuShaderStage::Vertex, input.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fragment = factory
        .create_shader(GpuShaderStage::Fragment, input.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    for offset in [0, 1] {
        let layout: ComPtr<IGpuPipelineSourceAbi> = PipelineSource::from_value(PipelineSource {
            info: GpuPipelineInfo {
                vertex_buffer_count: 1,
                attribute_count: 1,
                ..GpuPipelineInfo::default()
            },
            layouts: vec![GpuVertexBufferLayout {
                slot: 0,
                stride: 8,
                step: GpuStepMode::Vertex,
            }],
            attributes: vec![GpuVertexAttribute {
                location: 0,
                slot: 0,
                format: GpuVertexFormat::Unorm8x4,
                offset,
            }],
            bindings: Vec::new(),
        })
        .query()
        .unwrap();
        assert_eq!(
            factory.create_pipeline(layout.as_com_ref(), vertex.as_com_ref(), fragment.as_com_ref()).is_ok(),
            offset == 0
        );
    }
}

/// Equal varying shapes with conflicting interpolation reject before any native driver linking.
#[test]
fn independent_shader_stages_require_matching_interpolation() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_controller, factory) = client(&device, RenderClientLimits::default());
    for (vertex_interpolation, expected) in [("@interpolate(flat)", false), ("", true)] {
        let code = format!(
            r#"
struct V {{ @builtin(position) p:vec4<f32>, @location(0) {vertex_interpolation} color:vec4<f32> }}
@vertex fn vs() -> V {{ return V(vec4<f32>(0.0,0.0,0.5,1.0),vec4<f32>(1.0)); }}
@fragment fn fs(@location(0) color:vec4<f32>) -> @location(0) vec4<f32> {{ return color; }}
"#
        );
        let input = crate::source::Bytes::source(code.into_bytes());
        let vertex = factory
            .create_shader(GpuShaderStage::Vertex, input.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
            .unwrap();
        let fragment = factory
            .create_shader(GpuShaderStage::Fragment, input.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
            .unwrap();
        let desc = crate::source::PipelineDesc::default().source().unwrap();
        assert_eq!(
            factory.create_pipeline(desc.as_com_ref(), vertex.as_com_ref(), fragment.as_com_ref()).is_ok(),
            expected
        );
    }
}

/// Independently valid logical devices can feed one native cache without aliasing.
#[test]
fn version_identity_spans_devices_and_survives_owner_release_and_update() {
    let mut identities = std::collections::HashSet::new();
    let mut retained = Vec::new();
    for pixel in [17, 233] {
        let device = GpuDevice::new(
            nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
                .query()
                .unwrap(),
        );
        let (_owner, factory) = client(&device, Default::default());
        let (bytes, _, _) = source(vec![pixel; 4], ReadMode::Exact);
        let texture = factory
            .create_texture(
                GpuTextureDesc {
                    size: Size { width: 1, height: 1 },
                    format: GpuTextureFormat::Rgba8Unorm,
                    mip_count: 1,
                },
                neocogi_render_core::source::supplied(bytes.as_com_ref()).unwrap().borrow().unwrap(),
            )
            .unwrap();
        let before = device.texture_snapshot(factory.as_com_ref(), texture.as_com_ref()).unwrap();
        assert!(identities.insert(before.id()));
        let (replacement, _, _) = source(vec![pixel + 1; 4], ReadMode::Exact);
        factory
            .update_texture(texture.as_com_ref(), Rect { x: 0, y: 0, width: 1, height: 1 }, replacement.as_com_ref())
            .unwrap();
        let after = device.texture_snapshot(factory.as_com_ref(), texture.as_com_ref()).unwrap();
        assert!(identities.insert(after.id()));
        let sampler = factory.create_sampler(Default::default()).unwrap();
        assert!(identities.insert(device.sampler_snapshot(factory.as_com_ref(), sampler.as_com_ref()).unwrap().id()));
        drop(texture);
        assert_eq!(before.pixels().unwrap().as_slice().unwrap(), [pixel; 4]);
        assert_eq!(after.pixels().unwrap().as_slice().unwrap(), [pixel + 1; 4]);
        retained.extend([before, after]);
    }
    assert_eq!(identities.len(), 6);
    assert_eq!(retained.len(), 4);
}

/// A failed terminal allocation never wraps, publishes zero or reuses an ID.
#[test]
fn version_identity_exhaustion_is_sticky_without_global_reset() {
    let sequence = AtomicU64::new(u64::MAX - 1);
    assert_eq!(allocate_version_id(&sequence), Ok(u64::MAX - 1));
    assert_eq!(allocate_version_id(&sequence), Err(NC_E_OUT_OF_RANGE));
    assert_eq!(allocate_version_id(&sequence), Err(NC_E_OUT_OF_RANGE));
    assert_eq!(sequence.load(Ordering::Relaxed), u64::MAX);
}

/// None reserves the descriptor extent but owns no CPU zero vector; updates preserve old snapshots.
#[test]
fn absent_initial_contents_are_zero_and_updates_are_immutable() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_owner, factory) = client(&device, RenderClientLimits::default());
    let none = neocogi_render_core::source::initial(None);
    let buffer = factory
        .create_buffer(GpuBufferDesc { role: GpuBufferRole::Index, byte_len: 4 }, none.borrow().unwrap())
        .unwrap();
    let before = device.buffer_snapshot(factory.as_com_ref(), buffer.as_com_ref()).unwrap();
    assert!(matches!(before.bytes(), VersionBytes::Zeroed { len: 4 }));
    let mut output = [7; 4];
    before.bytes().read(0, &mut output).unwrap();
    assert_eq!(output, [0; 4]);
    let patch = neocogi_render_core::source::Bytes::source(vec![9, 8]);
    factory.update_buffer(buffer.as_com_ref(), 1, patch.as_com_ref()).unwrap();
    let after = device.buffer_snapshot(factory.as_com_ref(), buffer.as_com_ref()).unwrap();
    after.bytes().read(0, &mut output).unwrap();
    assert_eq!(output, [0, 9, 8, 0]);
    before.bytes().read(0, &mut output).unwrap();
    assert_eq!(output, [0; 4]);
    assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 8);
    drop(before);
    assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 4);
    drop((after, buffer));
    assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 0);
    let image = factory
        .create_texture(
            GpuTextureDesc {
                size: Size { width: 2, height: 2 },
                format: GpuTextureFormat::Rgba8Unorm,
                mip_count: 1,
            },
            none.borrow().unwrap(),
        )
        .unwrap();
    let old = device.texture_snapshot(factory.as_com_ref(), image.as_com_ref()).unwrap();
    assert!(matches!(old.pixels().unwrap(), VersionBytes::Zeroed { len: 16 }));
    let patch = neocogi_render_core::source::Bytes::source(vec![1, 2, 3, 4]);
    factory
        .update_texture(image.as_com_ref(), Rect { x: 1, y: 1, width: 1, height: 1 }, patch.as_com_ref())
        .unwrap();
    let new = device.texture_snapshot(factory.as_com_ref(), image.as_com_ref()).unwrap();
    let mut pixels = [8; 16];
    new.pixels().unwrap().copy_into(&mut pixels).unwrap();
    assert_eq!(&pixels[..12], &[0; 12]);
    assert_eq!(&pixels[12..], &[1, 2, 3, 4]);
    old.pixels().unwrap().copy_into(&mut pixels).unwrap();
    assert_eq!(pixels, [0; 16]);
}

/// Optional-tag rejection uses valid aligned initialized wire, not arbitrary foreign pointers.
#[test]
fn optional_wire_tags_and_supplied_lengths_are_checked_before_publication() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_owner, factory) = client(&device, RenderClientLimits::default());
    let none = neocogi_render_core::source::initial(None);
    let valid = none.borrow().unwrap();
    // SAFETY: The generated producer owns a complete initialized wire for this exact closed type.
    let mut wire = unsafe { valid.as_ptr().read() };
    wire.tag = 3;
    // SAFETY: Storage is aligned/initialized and its valid payload stays alive; only the tag is invalid.
    let invalid = unsafe { neocogi_com::NcValueRef::<InitialGpuData>::from_raw(&wire) }.unwrap();
    assert!(matches!(
        factory.create_buffer(GpuBufferDesc { role: GpuBufferRole::Vertex, byte_len: 4 }, invalid),
        Err(neocogi_com::NC_E_INVALID_TAG)
    ));
    for len in [0, 3, 5] {
        let (source, reads, _) = source(vec![4; len], ReadMode::Exact);
        let supplied = neocogi_render_core::source::initial(Some(source));
        assert!(matches!(
            factory.create_buffer(GpuBufferDesc { role: GpuBufferRole::Vertex, byte_len: 4 }, supplied.borrow().unwrap()),
            Err(NC_E_INVALID_ARG)
        ));
        assert!(matches!(
            factory.create_texture(
                GpuTextureDesc {
                    size: Size { width: 1, height: 1 },
                    format: GpuTextureFormat::Rgba8Unorm,
                    mip_count: 1
                },
                supplied.borrow().unwrap()
            ),
            Err(NC_E_INVALID_ARG)
        ));
        assert_eq!(reads.get(), 0);
        assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 0);
    }
}

/// Compatibility metadata is retained, distinct per device and cannot keep driver authority alive.
#[test]
fn driver_identity_is_stable_without_owning_the_driver() {
    let compiler: ComPtr<neocogi_shader_core::abi::ShaderCompilerAbi> = nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
        .query()
        .unwrap();
    let first = GpuDevice::new(compiler.clone());
    let second = GpuDevice::new(compiler);
    let weak = Rc::downgrade(&first.inner);
    let first_id = first.driver_identity();
    let second_id = second.driver_identity();
    let (_owner, factory) = client(&first, RenderClientLimits::default());
    let none = neocogi_render_core::source::initial(None);
    let resource = factory
        .create_buffer(GpuBufferDesc { role: GpuBufferRole::Vertex, byte_len: 4 }, none.borrow().unwrap())
        .unwrap();
    assert_eq!(first_id.as_raw().object, factory.driver_identity().unwrap().as_raw().object);
    assert_eq!(first_id.as_raw().object, resource.driver_identity().unwrap().as_raw().object);
    assert_ne!(first_id.as_raw().object, second_id.as_raw().object);
    assert!(first_id.query::<IRenderHostAbi>().is_err());
    assert!(first_id.query::<IRendererAbi>().is_err());
    drop((resource, factory, _owner, first));
    assert!(weak.upgrade().is_none());
    assert!(first_id.query::<IRenderDriverIdentityAbi>().is_ok());
}

/// The pre-release v1 numeral does not bypass the generated structural signature.
#[test]
fn version_one_keeps_structural_query_admission() {
    struct StaleShape;
    // SAFETY: Only QueryInterface uses this marker. The readable vtable prefix matches,
    // and no method is dispatched under the deliberately incompatible signature.
    unsafe impl ComInterface for StaleShape {
        const INTERFACE_ID: neocogi_com::NcInterfaceId = neocogi_com::NcInterfaceId {
            iid: IRenderResourcesAbi::INTERFACE_ID.iid,
            abi_signature: ITextureFactoryAbi::INTERFACE_ID.abi_signature,
        };
        type VTable = <IRenderResourcesAbi as ComInterface>::VTable;
        type Threading = ThreadLocal;
    }
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (_owner, factory) = client(&device, RenderClientLimits::default());
    assert_eq!(factory.query::<StaleShape>().unwrap_err(), neocogi_com::NC_E_ABI_VERSION);
    assert!(factory.query::<IRenderResourcesAbi>().is_ok());
}
