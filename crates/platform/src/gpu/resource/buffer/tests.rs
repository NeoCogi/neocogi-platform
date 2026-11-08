//! Actual generated calls cover fixed storage, completion ownership and typed authoring.
use super::*;
use crate::gpu::testing::{self, Backend, Preparation};
use neocogi_platform::render::buffer::{GpuElement, TypedGpuBufferDesc, TypedGpuBufferFactory};

/// Creates one explicitly selected virtual family and ordinary resource group.
fn setup() -> (GpuDevice<Backend>, ComBox<IRenderClientDomainAbi>, ComPtr<IRenderResourcesAbi>) {
    let gpu = testing::device(
        neocogi_platform::compiler::CompilerObject::from_value(neocogi_platform::compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let domain = gpu
        .create_client(
            Default::default(),
            gpu.surface_target(RenderSurfaceKind::Framebuffer).unwrap(),
            Default::default(),
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    (gpu, domain, resources)
}

/// Byte offsets address the resource; input slice indices always begin at zero.
#[test]
fn copied_transfers_preserve_dynamic_bytes_and_publish_exact_read_prefixes() {
    let (_gpu, _domain, resources) = setup();
    let mut buffer = resources.create_dynamic_buffer(GpuBufferDesc::Vertex { byte_len: 8 }).unwrap();
    let mut source = [1, 2, 3, 4];
    buffer.upload(NcCopyFromBufferOffset::new(2, &source)).unwrap();
    source.fill(99); // The call retained no caller memory.
    let mut output = NcOutputBuffer::new();
    buffer.read(output.output(0, 8)).unwrap();
    assert_eq!(output.as_slice(), &[0, 0, 1, 2, 3, 4, 0, 0]);
    buffer.read(output.output(3, 2)).unwrap();
    assert_eq!(output.as_slice(), &[2, 3]);
    for (offset, len) in [(7, 2), (u64::MAX, 1), (9, 0)] {
        assert_eq!(buffer.read(output.output(offset, len)), Err(NC_E_OUT_OF_RANGE));
        assert!(output.is_empty(), "failed outputs publish no readable prefix");
        assert_eq!(buffer.upload(NcCopyFromBufferOffset::new(offset, &vec![7; len])), Err(NC_E_OUT_OF_RANGE));
    }
    buffer.read(output.output(8, 0)).unwrap();
    buffer.upload(NcCopyFromBufferOffset::new(8, &[])).unwrap();
    buffer.read(output.output(0, 8)).unwrap();
    assert_eq!(output.as_slice(), &[0, 0, 1, 2, 3, 4, 0, 0]);
}

/// Read results belong to the caller, with no resource/core/lease retention.
#[test]
fn copied_output_survives_owner_domain_and_driver_drop_without_retaining_storage() {
    let (gpu, domain, resources) = setup();
    let account = gpu.domain(resources.as_com_ref()).unwrap().account.clone();
    let authority = Rc::downgrade(&gpu.inner);
    let mut buffer = resources.create_dynamic_buffer(GpuBufferDesc::Vertex { byte_len: 8 }).unwrap();
    buffer.upload(NcCopyFromBufferOffset::new(0, &[17; 8])).unwrap();
    let mut output = NcOutputBuffer::new();
    buffer.read(output.output(0, 8)).unwrap();
    drop((buffer, resources, domain, gpu));
    assert!(authority.upgrade().is_none());
    assert_eq!(account.live_bytes(), 0);
    assert_eq!(output.as_slice(), &[17; 8]);
}

#[test]
fn streamed_exhaustion_is_busy_and_refunds_no_occupied_capacity() {
    let (gpu, _domain, resources) = setup();
    let mut buffer = resources.create_streamed_buffer(GpuBufferDesc::Vertex { byte_len: 4 }).unwrap();
    let mut captures = Vec::new();
    for value in 1..=STREAM_SLOTS {
        testing::write_buffer(buffer.as_com_ref(), 0, &[value as u8; 4]).unwrap();
        captures.push(gpu.buffer_snapshot(resources.as_com_ref(), buffer.as_com_ref()).unwrap());
    }
    let charge = gpu.live_bytes(resources.as_com_ref()).unwrap();
    assert_eq!(charge, (STREAM_SLOTS * 4) as u64);
    assert_eq!(buffer.upload(NcCopyFromBufferOffset::new(0, &[9; 4])), Err(RENDER_E_BUSY));
    assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), charge);
    for (index, capture) in captures.iter().enumerate() {
        assert_eq!(capture.bytes().as_slice(), &[index as u8 + 1; 4]);
    }
    drop(captures.remove(0));
    buffer.upload(NcCopyFromBufferOffset::new(0, &[9; 4])).unwrap();
    for (index, capture) in captures.iter().enumerate() {
        assert_eq!(capture.bytes().as_slice(), &[index as u8 + 2; 4]);
    }
    assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), charge);
}

#[test]
fn native_owners_block_rewrite_and_completed_writes_reuse_the_same_allocation() {
    let (gpu, _domain, resources) = setup();
    let core = gpu.inner.core.clone();
    let mut buffer = resources.create_dynamic_buffer(GpuBufferDesc::Uniform { byte_len: 16 }).unwrap();
    let capture = gpu.buffer_snapshot(resources.as_com_ref(), buffer.as_com_ref()).unwrap();
    let native = capture.native(&mut Preparation::new(&core)).unwrap();
    let handle = Rc::as_ptr(&native);
    drop(capture);
    assert_eq!(buffer.upload(NcCopyFromBufferOffset::new(4, &[0xA5; 4])), Err(RENDER_E_BUSY));
    drop(native);
    buffer.upload(NcCopyFromBufferOffset::new(4, &[0xA5; 4])).unwrap();
    for _ in 0..5 {
        let capture = gpu.buffer_snapshot(resources.as_com_ref(), buffer.as_com_ref()).unwrap();
        let native = capture.native(&mut Preparation::new(&core)).unwrap();
        assert_eq!(Rc::as_ptr(&native), handle);
        assert_eq!(core.created(), 1);
        assert_eq!(&capture.bytes().as_slice()[4..8], &[0xA5; 4]);
        drop((capture, native));
        testing::write_buffer(buffer.as_com_ref(), 4, &[0xA5; 4]).unwrap();
    }
}

#[test]
fn busy_upload_keeps_old_contents_readable_and_capturable() {
    let (gpu, _domain, resources) = setup();
    let mut buffer = resources.create_dynamic_buffer(GpuBufferDesc::Index { byte_len: 4 }).unwrap();
    let capture = gpu.buffer_snapshot(resources.as_com_ref(), buffer.as_com_ref()).unwrap();
    assert_eq!(buffer.upload(NcCopyFromBufferOffset::new(0, &[9; 4])), Err(RENDER_E_BUSY));
    let second = gpu.buffer_snapshot(resources.as_com_ref(), buffer.as_com_ref()).unwrap();
    assert!(Rc::ptr_eq(&capture, &second));
    let mut output = NcOutputBuffer::new();
    buffer.read(output.output(0, 4)).unwrap();
    assert_eq!(output.as_slice(), &[0; 4]);
    // Empty transfers perform no write and therefore need no idle slot.
    buffer.upload(NcCopyFromBufferOffset::new(4, &[])).unwrap();
    drop((capture, second));
    buffer.upload(NcCopyFromBufferOffset::new(0, &[9; 4])).unwrap();
}

#[test]
fn streamed_replacement_clears_shorter_tails_without_mutating_older_captures() {
    let (gpu, _domain, resources) = setup();
    let buffer = resources.create_streamed_buffer(GpuBufferDesc::Vertex { byte_len: 16 }).unwrap();
    testing::write_buffer(buffer.as_com_ref(), 0, &[0xA5; 16]).unwrap();
    let old = gpu.buffer_snapshot(resources.as_com_ref(), buffer.as_com_ref()).unwrap();
    testing::write_buffer(buffer.as_com_ref(), 4, &[0xB6; 4]).unwrap();
    let new = gpu.buffer_snapshot(resources.as_com_ref(), buffer.as_com_ref()).unwrap();
    assert_eq!(old.bytes().as_slice(), &[0xA5; 16]);
    assert_eq!(&new.bytes().as_slice()[..4], &[0; 4]);
    assert_eq!(&new.bytes().as_slice()[4..8], &[0xB6; 4]);
    assert_eq!(&new.bytes().as_slice()[8..], &[0; 8]);
}

#[test]
fn texture_transfers_use_flat_row_major_bytes_without_native_padding() {
    let (gpu, _domain, resources) = setup();
    let mut texture = resources
        .create_dynamic_buffer(GpuBufferDesc::Texture {
            size: Size { width: 3, height: 2 },
            format: GpuTextureFormat::Rgba8Unorm,
            mip_count: 1,
        })
        .unwrap();
    texture.upload(NcCopyFromBufferOffset::new(4, &[1, 2, 3, 4])).unwrap();
    texture.upload(NcCopyFromBufferOffset::new(16, &[5, 6, 7, 8])).unwrap();
    let mut output = NcOutputBuffer::new();
    texture.read(output.output(16, 4)).unwrap();
    assert_eq!(output.as_slice(), &[5, 6, 7, 8]);
    let capture = gpu.texture_snapshot(resources.as_com_ref(), texture.as_com_ref()).unwrap();
    assert_eq!(
        capture.pixels().unwrap().as_slice(),
        &[0, 0, 0, 0, 1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0, 5, 6, 7, 8, 0, 0, 0, 0]
    );
}

#[repr(C)]
#[derive(Clone, Copy)]
/// Application-owned layout; the driver and NCIDL schema never define this vertex.
struct Vertex {
    x: f32,
    y: f32,
}
// SAFETY: Two little-endian f32 values have initialized unrestricted bits and no
// padding. This fixture's explicit vertex interpretation has stride eight.
unsafe impl GpuElement for Vertex {}

#[test]
fn generic_factory_wraps_original_driver_buffers_without_vertex_interfaces() {
    let (gpu, _domain, resources) = setup();
    let factory = TypedGpuBufferFactory::new(resources.clone());
    let mut buffer = factory.create_streamed::<Vertex>(TypedGpuBufferDesc::Vertex { element_count: 2 }).unwrap();
    let identity = buffer.as_gpu_buffer().as_raw().object;
    assert_eq!(buffer.element_count(), 2);
    assert_eq!(buffer.element_stride(), 8);
    for value in 0..10 {
        buffer.upload(0, &[Vertex { x: value as f32, y: 2.0 }, Vertex { x: 3.0, y: 4.0 }]).unwrap();
        assert_eq!(buffer.as_gpu_buffer().as_raw().object, identity);
        let capture = gpu.buffer_snapshot(resources.as_com_ref(), buffer.as_gpu_buffer()).unwrap();
        assert_eq!(&capture.bytes().as_slice()[..4], &(value as f32).to_le_bytes());
    }
    assert_eq!(buffer.into_gpu_buffer().as_raw().object, identity);
}

#[test]
fn typed_pixel_transfers_use_element_offsets_and_validate_before_allocation() {
    let (gpu, _domain, resources) = setup();
    let factory = TypedGpuBufferFactory::new(resources.clone());
    assert!(factory.create_dynamic::<f32>(TypedGpuBufferDesc::Index { element_count: 2 }).is_err());
    assert!(factory.create_dynamic::<[u8; 0]>(TypedGpuBufferDesc::Vertex { element_count: 2 }).is_err());
    assert!(factory.create_dynamic::<u32>(TypedGpuBufferDesc::Vertex { element_count: u64::MAX }).is_err());
    assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
    let mut texture = factory
        .create_dynamic::<[u8; 4]>(TypedGpuBufferDesc::Texture {
            size: Size { width: 2, height: 2 },
            format: GpuTextureFormat::Rgba8Unorm,
            mip_count: 1,
        })
        .unwrap();
    texture.upload(0, &[[1; 4], [2; 4], [3; 4], [4; 4]]).unwrap();
    let mut row = [[0; 4]; 2];
    texture.read(2, &mut row).unwrap();
    assert_eq!(row, [[3; 4], [4; 4]]);
    assert_eq!(texture.upload(u64::MAX, &[[5; 4]]), Err(NC_E_OUT_OF_RANGE));
    assert_eq!(texture.read(3, &mut row), Err(NC_E_OUT_OF_RANGE));
    let pixels = gpu.texture_snapshot(resources.as_com_ref(), texture.as_gpu_buffer()).unwrap();
    assert_eq!(pixels.pixels().unwrap().as_slice(), &[1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4]);
}

#[test]
fn observed_native_loss_rejects_transfers() {
    let (gpu, _domain, resources) = setup();
    let mut buffer = resources.create_dynamic_buffer(GpuBufferDesc::Vertex { byte_len: 4 }).unwrap();
    gpu.inner.core.lose();
    assert_eq!(buffer.upload(NcCopyFromBufferOffset::new(0, &[1; 4])), Err(RENDER_E_BACKEND));
}

/// Texture capacity is charged exactly once per slot, with no extra mapping scratch.
#[test]
fn texture_capacity_is_fixed_and_failed_creation_refunds_every_slot() {
    let (gpu, _domain, resources) = setup();
    let desc = GpuBufferDesc::Texture {
        size: Size { width: 2, height: 2 },
        format: GpuTextureFormat::Rgba8Unorm,
        mip_count: 1,
    };
    let texture = resources.create_dynamic_buffer(desc).unwrap();
    let account = gpu.domain(resources.as_com_ref()).unwrap().account.clone();
    assert_eq!(account.live_bytes(), 16);
    drop(texture);
    assert_eq!(account.live_bytes(), 0);
    let limited = gpu
        .create_client(
            RenderClientLimits { live_bytes: 47, ..Default::default() },
            gpu.surface_target(RenderSurfaceKind::Framebuffer).unwrap(),
            Default::default(),
        )
        .unwrap();
    let resources = limited.resources().unwrap();
    assert!(matches!(resources.create_streamed_buffer(desc), Err(NC_E_OUT_OF_RANGE)));
    assert_eq!(gpu.live_bytes(resources.as_com_ref()).unwrap(), 0);
}

/// Static resources expose their exact initial CPU contents without becoming writable.
#[test]
fn static_reads_and_reentrant_or_closed_transfers_obey_existing_authority() {
    let (gpu, domain, resources) = setup();
    let source = neocogi_platform::render::source::Bytes::source(vec![4, 5, 6, 7]);
    let mut buffer = resources
        .create_static_buffer(GpuBufferDesc::Vertex { byte_len: 4 }, source.as_com_ref())
        .unwrap();
    let mut output = NcOutputBuffer::new();
    buffer.read(output.output(1, 2)).unwrap();
    assert_eq!(output.as_slice(), &[5, 6]);
    assert_eq!(buffer.upload(NcCopyFromBufferOffset::new(0, &[9])), Err(RENDER_E_PERMISSION));
    assert_eq!(buffer.upload(NcCopyFromBufferOffset::new(4, &[])), Err(RENDER_E_PERMISSION));
    let mut mutable = resources.create_dynamic_buffer(GpuBufferDesc::Vertex { byte_len: 4 }).unwrap();
    let operation = gpu.operation().unwrap();
    assert_eq!(mutable.upload(NcCopyFromBufferOffset::new(0, &[9])), Err(RENDER_E_BUSY));
    assert_eq!(buffer.read(output.output(0, 1)), Err(RENDER_E_BUSY));
    assert!(output.is_empty());
    drop(operation);
    drop(domain);
    assert_eq!(mutable.upload(NcCopyFromBufferOffset::new(0, &[9])), Err(NC_E_DROPPED));
    assert_eq!(buffer.read(output.output(0, 1)), Err(NC_E_DROPPED));
    assert!(output.is_empty());
}
