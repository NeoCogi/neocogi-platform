//! Real COM streams exercise generic provenance, bounded intake, closure and cleanup.
use super::*;
use neocogi_com_sdk::{com_object, ComBox, ComPtr, NC_E_DROPPED};
type GpuDevice = neocogi_platform::gpu::GpuDevice<neocogi_platform::gpu::testing::Backend>;
use std::cell::{Cell, RefCell};

/// Assembly resolves only canonical blocks registered on this exact logical device.
struct Resolver(GpuDevice);
impl Resources for Resolver {
    type Backend = neocogi_platform::gpu::testing::Backend;
    fn view(&self, block: WeakComBox<IRenderViewBlockAbi>) -> Result<Rc<Block<neocogi_platform::gpu::testing::Backend>>, NcStatus> {
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
    /// Optional controller is closed while retrieving the last block.
    close: RefCell<Option<ComBox<IRenderResourceGroupAbi>>>,
}
impl IRenderStream for Stream {
    fn view(&self, invocation: u64, index: u32) -> Result<WeakComBox<IRenderViewBlockAbi>, NcStatus> {
        assert_eq!(invocation, 1);
        self.calls.set(self.calls.get() + 1);
        if index as usize + 1 == self.blocks.len() {
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
    let local = Rect {
        x: 0,
        y: 0,
        width: target.width,
        height: target.height,
    };
    let mut session = frame.begin_view(RenderViewRoot { local, target }, resources.as_com_ref()).unwrap();
    session.view().unwrap().finish().unwrap();
    let block = session.take_block().unwrap();
    frame.finish().unwrap();
    block
}
/// Publish owned blocks with independently observed foreign callback counts.
fn source(blocks: Vec<ComBox<IRenderViewBlockAbi>>, close: Option<ComBox<IRenderResourceGroupAbi>>) -> (ComPtr<IRenderStreamAbi>, Rc<Cell<u32>>) {
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
    let device = neocogi_platform::gpu::testing::device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let domain = device
        .create_group(
            Default::default(),
            device.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderGroupPermissions::default(),
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    let first = block(&device, &resources, ROOT);
    let observer = first.downgrade();
    let second_root = Rect { x: 2, y: 3, width: 4, height: 5 };
    let second = block(&device, &resources, second_root);
    let (stream, calls) = source(vec![first, second], None);
    let mut frame = PreparedFrame::default();
    frame
        .read(info(2), stream.as_com_ref(), &Resolver(device.clone()), SubmissionTarget::Framebuffer)
        .unwrap();
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
    let device = neocogi_platform::gpu::testing::device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
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
        assert_eq!(
            frame
                .read(bad, stream.as_com_ref(), &Resolver(device.clone()), SubmissionTarget::Framebuffer)
                .map_err(|error| error.status),
            Err(NC_E_INVALID_ARG)
        );
        assert!(frame.blocks().is_empty());
    }
    assert_eq!(calls.get(), 0);
    frame
        .read(info(0), stream.as_com_ref(), &Resolver(device), SubmissionTarget::Framebuffer)
        .unwrap();
}

/// Later failure cannot leave an earlier valid prefix ready for native execution.
#[test]
fn short_foreign_and_outside_blocks_clear_the_entire_preparation() {
    let device = neocogi_platform::gpu::testing::device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let other = neocogi_platform::gpu::testing::device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let domain = device
        .create_group(
            Default::default(),
            device.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderGroupPermissions::default(),
        )
        .unwrap();
    let alien = other
        .create_group(
            Default::default(),
            other.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderGroupPermissions::default(),
        )
        .unwrap();
    let resources = domain.resources().unwrap();
    let mut frame = PreparedFrame::default();
    let (short, _) = source(vec![block(&device, &resources, ROOT)], None);
    assert_eq!(
        frame
            .read(info(2), short.as_com_ref(), &Resolver(device.clone()), SubmissionTarget::Framebuffer)
            .map_err(|error| error.status),
        Err(NC_E_INVALID_ARG)
    );
    assert!(frame.blocks().is_empty());
    for bad in [
        block(&other, &alien.resources().unwrap(), ROOT),
        Forged::into_box(Forged).query().unwrap(),
        block(&device, &resources, Rect { x: 9, ..ROOT }),
    ] {
        let (stream, _) = source(vec![block(&device, &resources, ROOT), bad], None);
        assert!(
            frame
                .read(info(2), stream.as_com_ref(), &Resolver(device.clone()), SubmissionTarget::Framebuffer)
                .is_err()
        );
        assert!(frame.blocks().is_empty());
    }
}

/// Closing the first controller in a later accessor must fail the final admission gate.
#[test]
fn all_domains_are_rechecked_after_the_last_foreign_call() {
    let device = neocogi_platform::gpu::testing::device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let first = device
        .create_group(
            Default::default(),
            device.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderGroupPermissions::default(),
        )
        .unwrap();
    let second = device
        .create_group(
            Default::default(),
            device.surface_target(neocogi_platform::render::abi::RenderSurfaceKind::Framebuffer).unwrap(),
            neocogi_platform::render::abi::RenderGroupPermissions::default(),
        )
        .unwrap();
    let a = block(&device, &first.resources().unwrap(), ROOT);
    let b = block(&device, &second.resources().unwrap(), ROOT);
    let (stream, calls) = source(vec![a, b], Some(first));
    let mut frame = PreparedFrame::default();
    assert_eq!(
        frame
            .read(info(2), stream.as_com_ref(), &Resolver(device), SubmissionTarget::Framebuffer)
            .map_err(|error| error.status),
        Err(NC_E_DROPPED)
    );
    assert_eq!(calls.get(), 2);
    assert!(frame.blocks().is_empty());
}

/// An early mismatch yields to a later group closure during the final accessor.
/// Both target selections keep the status, diagnostic phase, and full cleanup.
#[test]
fn callback_closed_group_takes_precedence_over_earlier_target_mismatch() {
    let device = neocogi_platform::gpu::testing::device(
        neocogi_platform::render::compiler::CompilerObject::from_value(
            neocogi_platform::render::compiler::CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap(),
        )
        .query()
        .unwrap(),
    );
    let resolver = Resolver(device.clone());
    for image in [false, true] {
        let kind = if image { RenderSurfaceKind::Image } else { RenderSurfaceKind::Framebuffer };
        let target = device.surface_target(kind).unwrap();
        let other = device.surface_target(RenderSurfaceKind::Image).unwrap();
        let select = || {
            if image {
                SubmissionTarget::Exact(&target)
            } else {
                SubmissionTarget::Framebuffer
            }
        };
        let group = |target| device.create_group(Default::default(), target, RenderGroupPermissions::none()).unwrap();
        let wrong = group(other);
        let closing = group(target.clone());
        let open = group(target.clone());
        let wrong_resources = wrong.resources().unwrap();
        let closing_resources = closing.resources().unwrap();
        let open_resources = open.resources().unwrap();
        let blocks = || {
            vec![
                block(&device, &wrong_resources, ROOT),
                block(&device, &closing_resources, ROOT),
                block(&device, &open_resources, ROOT),
            ]
        };
        let mut frame = PreparedFrame::default();
        let (control, calls) = source(blocks(), None);
        let error = frame.read(info(3), control.as_com_ref(), &resolver, select()).unwrap_err();
        assert_eq!(error.status, RENDER_E_OWNER);
        assert!(!error.message.contains("frame preparation"));
        assert_eq!(calls.get(), 3);
        assert!(frame.blocks().is_empty());
        drop(control);

        let (mixed, calls) = source(blocks(), Some(closing));
        let error = frame.read(info(3), mixed.as_com_ref(), &resolver, select()).unwrap_err();
        assert_eq!(error.status, NC_E_DROPPED);
        assert!(error.message.contains("frame preparation"));
        assert_eq!(calls.get(), 3);
        assert!(frame.blocks().is_empty());
    }
}
