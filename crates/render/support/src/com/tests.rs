//! Real COM streams exercise generic provenance, bounded intake, closure and cleanup.
use super::*;
use neocogi_com::{com_object, ComBox, ComPtr, NC_E_DROPPED};
use neocogi_render_gpu::GpuDevice;
use std::cell::{Cell, RefCell};

/// Assembly resolves only canonical blocks registered on this exact logical device.
struct Resolver(GpuDevice);
impl Resources for Resolver {
    fn view(&self, block: WeakComBox<IRenderViewBlockAbi>) -> Result<Rc<Block>, NcStatus> {
        self.0.block_snapshot(block.as_com_ref())
    }
}
#[com_object(interfaces(IRenderStream))]
/// A foreign source may fail or close an earlier domain from a later accessor.
struct Stream {
    /// Exclusive owners stay alive only until this source is released.
    blocks: Vec<ComBox<IRenderViewBlockAbi>>,
    /// Independent invocation trace detects callbacks before count validation.
    calls: Rc<Cell<u32>>,
    /// Optional controller is closed while retrieving the second block.
    close: RefCell<Option<ComBox<IRenderClientDomainAbi>>>,
}
impl IRenderStream for Stream {
    fn view(&self, invocation: u64, index: u32) -> Result<WeakComBox<IRenderViewBlockAbi>, NcStatus> {
        assert_eq!(invocation, 1);
        self.calls.set(self.calls.get() + 1);
        if index == 1 {
            drop(self.close.borrow_mut().take());
        }
        self.blocks.get(index as usize).map(ComBox::downgrade).ok_or(NC_E_INVALID_ARG)
    }
}
#[com_object(interfaces(IRenderViewBlock), ownership=exclusive)]
/// Matching public interface metadata cannot manufacture canonical renderer provenance.
struct Forged;
impl IRenderViewBlock for Forged {}

/// Capture a real bounded contribution through generated recording calls.
fn block(device: &GpuDevice, resources: &ComPtr<IRenderResourcesAbi>, target: Rect) -> ComBox<IRenderViewBlockAbi> {
    let mut frame = device.broker().begin_frame().unwrap();
    let client = Rect {
        x: 0,
        y: 0,
        width: target.width,
        height: target.height,
    };
    let mut session = frame.begin_view(RenderViewRoot { client, target }, resources.as_com_ref()).unwrap();
    session.view().unwrap().finish().unwrap();
    let block = session.take_block().unwrap();
    frame.finish().unwrap();
    block
}
/// Publish owned blocks with independently observed foreign callback counts.
fn source(blocks: Vec<ComBox<IRenderViewBlockAbi>>, close: Option<ComBox<IRenderClientDomainAbi>>) -> (ComPtr<IRenderStreamAbi>, Rc<Cell<u32>>) {
    let calls = Rc::new(Cell::new(0));
    let stream = Stream::from_value(Stream {
        blocks,
        calls: calls.clone(),
        close: RefCell::new(close),
    })
    .query()
    .unwrap();
    (stream, calls)
}
/// A ten-pixel output admits an empty clear frame as well as contained contributions.
fn info(block_count: u32) -> RenderInfo {
    RenderInfo {
        size: Size { width: 10, height: 10 },
        clear: 0xff000000,
        invocation: 1,
        block_count,
    }
}
const ROOT: Rect = Rect { x: 0, y: 0, width: 10, height: 10 };

/// Preparation owns real snapshots after public block expiry, until receiver cleanup.
#[test]
fn prepared_blocks_preserve_order_and_release_resources_on_clear() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let domain = device.create_client(Default::default()).unwrap();
    let resources = domain.resources().unwrap();
    let first = block(&device, &resources, ROOT);
    let observer = first.downgrade();
    let second_root = Rect { x: 2, y: 3, width: 4, height: 5 };
    let second = block(&device, &resources, second_root);
    let (stream, calls) = source(vec![first, second], None);
    let mut frame = PreparedFrame::default();
    frame.read(info(2), stream.as_com_ref(), &Resolver(device.clone())).unwrap();
    assert_eq!(calls.get(), 2);
    assert_eq!(frame.blocks().iter().map(|block| block.root().target).collect::<Vec<_>>(), [ROOT, second_root]);
    drop(stream);
    assert!(device.block_snapshot(observer.as_com_ref()).is_err());
    assert!(device.recording_usage(resources.as_com_ref()).unwrap().0 > 0);
    frame.clear();
    assert_eq!(device.recording_usage(resources.as_com_ref()).unwrap(), (0, 0));
}

/// Invalid envelopes reject before the first callback or allocation driven by an unbounded count.
#[test]
fn envelope_limits_precede_stream_callbacks() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let (stream, calls) = source(vec![], None);
    let mut frame = PreparedFrame::default();
    for bad in [
        RenderInfo { invocation: 0, ..info(0) },
        RenderInfo {
            size: Size { width: 0, height: 10 },
            ..info(0)
        },
        info(MAX_RENDER_BLOCKS + 1),
    ] {
        assert_eq!(frame.read(bad, stream.as_com_ref(), &Resolver(device.clone())), Err(NC_E_INVALID_ARG));
        assert!(frame.blocks().is_empty());
    }
    assert_eq!(calls.get(), 0);
    frame.read(info(0), stream.as_com_ref(), &Resolver(device)).unwrap();
}

/// Later failure cannot leave an earlier valid prefix ready for native execution.
#[test]
fn short_foreign_and_outside_blocks_clear_the_entire_preparation() {
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
    let domain = device.create_client(Default::default()).unwrap();
    let alien = other.create_client(Default::default()).unwrap();
    let resources = domain.resources().unwrap();
    let mut frame = PreparedFrame::default();
    let (short, _) = source(vec![block(&device, &resources, ROOT)], None);
    assert_eq!(frame.read(info(2), short.as_com_ref(), &Resolver(device.clone())), Err(NC_E_INVALID_ARG));
    assert!(frame.blocks().is_empty());
    for bad in [
        block(&other, &alien.resources().unwrap(), ROOT),
        Forged::into_box(Forged).query().unwrap(),
        block(&device, &resources, Rect { x: 9, ..ROOT }),
    ] {
        let (stream, _) = source(vec![block(&device, &resources, ROOT), bad], None);
        assert!(frame.read(info(2), stream.as_com_ref(), &Resolver(device.clone())).is_err());
        assert!(frame.blocks().is_empty());
    }
}

/// Closing the first controller in a later accessor must fail the final admission gate.
#[test]
fn all_domains_are_rechecked_after_the_last_foreign_call() {
    let device = GpuDevice::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let first = device.create_client(Default::default()).unwrap();
    let second = device.create_client(Default::default()).unwrap();
    let a = block(&device, &first.resources().unwrap(), ROOT);
    let b = block(&device, &second.resources().unwrap(), ROOT);
    let (stream, calls) = source(vec![a, b], Some(first));
    let mut frame = PreparedFrame::default();
    assert_eq!(frame.read(info(2), stream.as_com_ref(), &Resolver(device)), Err(NC_E_DROPPED));
    assert_eq!(calls.get(), 2);
    assert!(frame.blocks().is_empty());
}
