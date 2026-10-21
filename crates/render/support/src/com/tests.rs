//! Boundary tests use real generated COM calls and concrete resource values, without requiring a GPU.
use super::*;
use neocogi_com::{com_object, NcOutputWriter};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

/// Independent observations survive source/resource destruction.
#[derive(Default)]
struct Probe {
    /// Counts complete bulk copies, proving the boundary does not call once per primitive.
    copies: Cell<u32>,
    /// Tracks owner resolution in source painter order.
    resolved: RefCell<Vec<&'static str>>,
    /// Injects a later failure after one resource has already been retained.
    reject_item: Cell<bool>,
    /// Makes a stream report a truncated copy while returning otherwise valid values.
    short: Cell<bool>,
    /// Final resource releases prove complete cleanup after finish/error/abandonment.
    releases: Rc<Cell<u32>>,
}
#[com_object(interfaces(ITexture, IRenderItem))]
/// One fake resource exposes both capabilities solely to count its actual COM lifetime.
struct Resource {
    /// Shared final-release observation.
    releases: Rc<Cell<u32>>,
}
impl ITexture for Resource {
    /// The validator authenticates through its resolver; no size query is required per frame.
    fn size(&self) -> Result<Size, NcStatus> {
        Ok(Size { width: 2, height: 2 })
    }
}
impl IRenderItem for Resource {}
impl Drop for Resource {
    /// Destruction is counted once for the canonical allocation, independent of interface count.
    fn drop(&mut self) {
        self.releases.set(self.releases.get() + 1);
    }
}
#[com_object(interfaces(IRenderStream))]
/// Foreign-style source owns its complete buffers and capabilities.
struct Source {
    /// Geometry copied in one bulk operation.
    vertices: Vec<RenderVertex>,
    /// Native records can carry invalid geometry but never an invalid enum tag.
    records: Vec<RenderBatch>,
    /// Owned image returned only at a matching batch index.
    texture: ComPtr<ITextureAbi>,
    /// Same canonical resource through its custom-item projection.
    item: ComPtr<IRenderItemAbi>,
    /// Independent observations and failure injection.
    probe: Rc<Probe>,
}
impl IRenderStream for Source {
    /// Copies at offset zero and optionally simulates a short producer read.
    fn vertices(&self, invocation: u64, mut output: NcOutputWriter<'_, RenderVertex>) -> Result<u32, NcStatus> {
        assert_eq!(invocation, 1);
        assert_eq!(output.offset(), 0);
        self.probe.copies.set(self.probe.copies.get() + 1);
        output.extend_from_slice(&self.vertices)?;
        Ok((self.vertices.len() as u32).saturating_sub(u32::from(self.probe.short.get())))
    }
    /// Copies the complete ordered ABI records with no per-operation foreign call.
    fn batches(&self, invocation: u64, mut output: NcOutputWriter<'_, RenderBatch>) -> Result<u32, NcStatus> {
        assert_eq!(invocation, 1);
        assert_eq!(output.offset(), 0);
        self.probe.copies.set(self.probe.copies.get() + 1);
        output.extend_from_slice(&self.records)?;
        Ok(self.records.len() as u32)
    }
    /// A mismatch would expose a batch-index bug in the reader.
    fn texture(&self, _: u64, index: u32) -> Result<ComPtr<ITextureAbi>, NcStatus> {
        assert!(matches!(self.records[index as usize], RenderBatch::Texture { .. }));
        Ok(self.texture.clone())
    }
    /// The index is the overall batch position, not a separate custom-resource index.
    fn item(&self, _: u64, index: u32) -> Result<ComPtr<IRenderItemAbi>, NcStatus> {
        assert!(matches!(self.records[index as usize], RenderBatch::Item { .. }));
        Ok(self.item.clone())
    }
}
impl Resources for Probe {
    type Texture = ComPtr<ITextureAbi>;
    type Item = ComPtr<IRenderItemAbi>;
    /// Keeps an owned capability as the fake native resource, allowing precise cleanup assertions.
    fn texture(&self, texture: Self::Texture) -> Result<Self::Texture, NcStatus> {
        self.resolved.borrow_mut().push("texture");
        Ok(texture)
    }
    /// Simulates a foreign-owner failure after any preceding texture was resolved.
    fn item(&self, item: Self::Item) -> Result<Self::Item, NcStatus> {
        self.resolved.borrow_mut().push("item");
        if self.reject_item.get() { Err(neocogi_com::NC_E_NOT_FOUND) } else { Ok(item) }
    }
}
/// Fixed viewport leaves room to exercise clipped custom content beyond its origin.
const SIZE: Size = Size { width: 100, height: 80 };
/// Positive content used by the valid custom batch.
const RECT: Rect = Rect { x: -10, y: 0, width: 110, height: 80 };
/// Final clip intersects the negative-origin content with the viewport.
const CLIP: Rect = Rect { x: 0, y: 0, width: 100, height: 80 };
/// Creates one owned source and exact announced counts, optionally containing malformed records.
fn source(records: Vec<RenderBatch>, vertices: Vec<RenderVertex>) -> (ComPtr<IRenderStreamAbi>, RenderInfo, Rc<Probe>) {
    let probe = Rc::new(Probe::default());
    let resource = Resource::from_value(Resource { releases: probe.releases.clone() });
    let info = RenderInfo {
        size: SIZE,
        clear: 0,
        invocation: 1,
        vertex_count: vertices.len() as u32,
        batch_count: records.len() as u32,
    };
    let stream = Source::from_value(Source {
        vertices,
        records,
        texture: resource.query().unwrap(),
        item: resource.query().unwrap(),
        probe: probe.clone(),
    })
    .query()
    .unwrap();
    (stream, info, probe)
}
/// Atlas/image/custom/atlas must remain ordered, use exactly two bulk copies and retain resources until clear.
#[test]
fn ordered_bulk_frame_owns_resources_and_reuses_allocations() {
    let records = vec![
        RenderBatch::Atlas { first: 0, count: 3 },
        RenderBatch::Texture { first: 3 },
        RenderBatch::Item { rect: RECT, clip: CLIP },
        RenderBatch::Atlas { first: 7, count: 3 },
    ];
    let (stream, info, probe) = source(records, vec![RenderVertex::default(); 10]);
    let mut frame = PreparedFrame::default();
    frame.read(info, stream.as_com_ref(), &*probe).unwrap();
    assert!(matches!(&frame.batches()[0], Batch::Atlas(range) if *range == (0..3)));
    assert!(matches!(&frame.batches()[1], Batch::Texture { .. }));
    assert!(matches!(&frame.batches()[2], Batch::Item { rect, clip, .. } if *rect == RECT && *clip == CLIP));
    assert!(matches!(&frame.batches()[3], Batch::Atlas(range) if *range == (7..10)));
    assert_eq!(probe.copies.get(), 2);
    assert_eq!(&*probe.resolved.borrow(), &["texture", "item"]);
    let capacities = (frame.vertices.capacity(), frame.records.capacity(), frame.batches.capacity());
    frame.read(info, stream.as_com_ref(), &*probe).unwrap();
    assert_eq!(capacities, (frame.vertices.capacity(), frame.records.capacity(), frame.batches.capacity()));
    drop(stream);
    assert_eq!(probe.releases.get(), 0);
    frame.clear();
    assert_eq!(probe.releases.get(), 1);
}
/// Invalid later records must prevent even earlier resources from being resolved or queued.
#[test]
fn malformed_ranges_tags_and_clips_reject_before_resource_resolution() {
    let invalid = [
        RenderBatch::Atlas { first: 4, count: 0 },
        RenderBatch::Atlas { first: 3, count: 3 },
        RenderBatch::Atlas { first: 5, count: 3 },
        RenderBatch::Atlas { first: 4, count: 4 },
        RenderBatch::Atlas { first: 4, count: u32::MAX },
        RenderBatch::Item {
            rect: RECT,
            clip: Rect { width: 101, ..CLIP },
        },
        RenderBatch::Item {
            rect: Rect { width: 1, ..RECT },
            clip: CLIP,
        },
        RenderBatch::Item {
            rect: RECT,
            clip: Rect { width: 0, ..CLIP },
        },
    ];
    for bad in invalid {
        let (stream, info, probe) = source(vec![RenderBatch::Texture { first: 0 }, bad], vec![RenderVertex::default(); 7]);
        let mut frame = PreparedFrame::default();
        assert_eq!(frame.read(info, stream.as_com_ref(), &*probe), Err(NC_E_INVALID_ARG));
        assert!(frame.batches().is_empty());
        assert!(probe.resolved.borrow().is_empty());
    }
}
/// Checked COM output decoding rejects unknown enum tags before renderer-level
/// geometry validation. This remains distinct from an invalid range or clip.
#[test]
fn invalid_record_tag_rejects_in_the_generated_buffer_codec() {
    /// Simulates an independently compiled provider returning an unknown raw tag.
    unsafe extern "C" fn malformed_batches(
        _object: *mut core::ffi::c_void,
        _invocation: u64,
        _offset: u64,
        count: u64,
        output: *mut <RenderBatch as neocogi_com::ComValue>::Wire,
        prefix: *mut u64,
        written: *mut u32,
    ) -> NcStatus {
        if count != 2 {
            return NC_E_INVALID_ARG;
        }
        let good = neocogi_com::encode_inline(&RenderBatch::Texture { first: 0 }).unwrap();
        let mut bad = good;
        bad.tag = 255;
        // SAFETY: the generated caller provides two writable wire records and
        // a writable result. No invalid native Rust enum is ever constructed.
        unsafe {
            output.write(good);
            output.add(1).write(bad);
            prefix.write(2);
            written.write(2);
        }
        neocogi_com::NC_OK
    }
    let (stream, info, probe) = source(vec![RenderBatch::Texture { first: 0 }; 2], vec![RenderVertex::default(); 7]);
    let raw = stream.as_raw();
    // SAFETY: vtables contain only immutable metadata and function pointers.
    // The object and our stack-local replacement table outlive the borrowed call.
    let mut table = unsafe { core::ptr::read(raw.vtable) };
    table.batches = malformed_batches;
    let borrowed = unsafe { neocogi_com::ComRef::from_raw_borrowed(neocogi_com::ComRaw { object: raw.object, vtable: &table }) }.unwrap();
    let mut frame = PreparedFrame::default();
    assert_eq!(frame.read(info, borrowed, &*probe), Err(neocogi_com::NC_E_INVALID_TAG));
    assert!(frame.batches().is_empty());
    assert!(probe.resolved.borrow().is_empty());
}
/// Short reads and non-finite components fail before passing owned data to a native backend.
#[test]
fn short_copies_nonfinite_vertices_and_unreferenced_geometry_reject() {
    for component in 0..6 {
        let mut vertex = RenderVertex::default();
        match component {
            0 => vertex.x = f32::NAN,
            1 => vertex.y = f32::INFINITY,
            2 => vertex.u = f32::NEG_INFINITY,
            3 => vertex.v = f32::NAN,
            _ => {}
        }
        let records = if component == 5 {
            Vec::new()
        } else {
            vec![RenderBatch::Atlas { first: 0, count: 3 }]
        };
        let (stream, info, probe) = source(records, vec![vertex; 3]);
        probe.short.set(component == 4);
        let mut frame = PreparedFrame::default();
        assert_eq!(frame.read(info, stream.as_com_ref(), &*probe), Err(NC_E_INVALID_ARG));
        assert!(frame.vertices().is_empty());
    }
}
/// Invalid announced counts and scope reject before allocation or even the first stream read.
#[test]
fn invalid_frame_header_never_calls_the_producer() {
    let (stream, info, probe) = source(Vec::new(), Vec::new());
    for bad in [
        RenderInfo { invocation: 0, ..info },
        RenderInfo {
            vertex_count: MAX_RENDER_VERTICES + 1,
            ..info
        },
        RenderInfo {
            batch_count: MAX_RENDER_BATCHES + 1,
            ..info
        },
        RenderInfo { size: Size { width: 0, ..SIZE }, ..info },
    ] {
        assert_eq!(PreparedFrame::default().read(bad, stream.as_com_ref(), &*probe), Err(NC_E_INVALID_ARG));
    }
    assert_eq!(probe.copies.get(), 0);
    PreparedFrame::default().read(info, stream.as_com_ref(), &*probe).unwrap();
}
/// A later resource failure releases earlier snapshots and permits a clean retry with the same buffers.
#[test]
fn failed_resource_resolution_cleans_partial_work_and_allows_retry() {
    let (stream, info, probe) = source(
        vec![RenderBatch::Texture { first: 0 }, RenderBatch::Item { rect: RECT, clip: CLIP }],
        vec![RenderVertex::default(); 4],
    );
    let mut frame = PreparedFrame::default();
    probe.reject_item.set(true);
    assert_eq!(frame.read(info, stream.as_com_ref(), &*probe), Err(neocogi_com::NC_E_NOT_FOUND));
    assert!(frame.batches().is_empty());
    probe.reject_item.set(false);
    frame.read(info, stream.as_com_ref(), &*probe).unwrap();
    drop(stream);
    assert_eq!(probe.releases.get(), 0);
    drop(frame);
    assert_eq!(probe.releases.get(), 1);
}
