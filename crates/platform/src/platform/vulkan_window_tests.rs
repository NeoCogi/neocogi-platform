//! Actual COM submission across a real SDL Vulkan surface and two advertised formats.
use super::*;
use crate::gpu::{
    backends::vulkan::surface_tests::{NATIVE_SESSION, WINDOW_PROBE, WindowScope},
    testing::sampled,
};
use neocogi_com_sdk::{ComBox, WeakComBox};
use sdl2::{
    event::{Event, WindowEvent},
    keyboard::{Keycode, Mod},
};

#[com_object(interfaces(IInputSink))]
/// Captures normal native COM sink delivery after injecting a lost surface.
struct InputSink {
    /// Each delivery copies its input and releases this short observation borrow.
    events: Rc<RefCell<Vec<UiInput>>>,
}

impl IInputSink for InputSink {
    /// The queued key and focus transitions carry no text.
    fn input(&self, event: UiInput, text: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
        assert!(text.as_slice().is_empty());
        self.events.borrow_mut().push(event);
        Ok(())
    }
}

/// A terminal drawable failure does not consume later non-positional events.
fn check_surface_loss_input(device: &Device, window: &ComPtr<IPlatformWindowAbi>) {
    device.drawable.inject_surface_loss();
    assert_eq!(window.size(), Err(PLATFORM_E_BACKEND));
    let window_id = device.window.id();
    let event = |win_event| Event::Window { timestamp: 0, window_id, win_event };
    {
        let mut queues = device.host.queues.borrow_mut();
        let queue = queues.get_mut(&window_id).unwrap();
        *queue = Default::default();
        for event in [
            Event::MouseMotion {
                timestamp: 0,
                window_id,
                which: 0,
                mousestate: sdl2::mouse::MouseState::from_sdl_state(0),
                x: 2,
                y: 2,
                xrel: 1,
                yrel: 1,
            },
            event(WindowEvent::FocusLost),
            Event::KeyDown {
                timestamp: 0,
                window_id,
                keycode: Some(Keycode::Escape),
                scancode: None,
                keymod: Mod::NOMOD,
                repeat: false,
            },
            event(WindowEvent::Close),
        ] {
            queue.push(event);
        }
    }
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = InputSink::from_value(InputSink { events: events.clone() }).query::<IInputSinkAbi>().unwrap();
    assert_eq!(window.poll(sink.as_com_ref()), Err(PLATFORM_E_BACKEND));
    assert!(events.borrow().is_empty());
    assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Event));
    assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Event));
    assert_eq!(window.poll(sink.as_com_ref()), Ok(PollStatus::Close));
    assert!(matches!(
        events.borrow().as_slice(),
        [UiInput::Cancel, UiInput::Key(KeyStroke { key: KeyCode::Escape, .. })]
    ));
}

/// Resize the actual SDL window from a stream callback and query it through COM.
struct Resize {
    /// SDL needs mutable window access for this test's native resize.
    native: RefCell<gpu::Window>,
    /// Invokes the normal size query while renderer state is detached.
    window: ComPtr<IPlatformWindowAbi>,
    /// Retains the reentrant observation outside the callback.
    observed: Cell<Option<Size>>,
}

#[com_object(interfaces(IRenderStream))]
/// Retains a real draw block while optionally changing the window in its callback.
struct Stream {
    /// Sealed COM block includes shader, texture and draw commands.
    block: ComBox<IRenderViewBlockAbi>,
    /// Only the stale-size regression resizes from inside this callback.
    resize: Option<Rc<Resize>>,
}

impl IRenderStream for Stream {
    /// Reentrant metadata observation happens before returning a valid block.
    fn view(&self, _: u64, index: u32) -> Result<WeakComBox<IRenderViewBlockAbi>, NcStatus> {
        assert_eq!(index, 0);
        if let Some(resize) = &self.resize {
            resize.native.borrow_mut().set_size(80, 64).unwrap();
            resize.observed.set(Some(resize.window.size()?));
        }
        Ok(self.block.downgrade())
    }
}

/// Verifies final size admission, preparation for the selected generation and the
/// different owned outcomes of recoverable rejection and uncertain submission.
#[test]
#[ignore = "requires an SDL native display and Vulkan surface with two advertised color formats"]
fn vulkan_com_window_prepares_the_selected_generation_and_retries_rejected_present() {
    let _native = NATIVE_SESSION.lock().unwrap_or_else(|error| error.into_inner());
    let _scope = WindowScope::start();
    let object = PlatformObject::new()
        .window("Vulkan COM presentation generations", Size { width: 64, height: 64 })
        .unwrap();
    let device = object.device.clone();
    let window: ComPtr<IPlatformWindowAbi> = WindowObject::from_value(object).query().unwrap();
    let renderer: ComPtr<IRendererAbi> = window.query().unwrap();
    let host = window.graphics_host().unwrap();
    let surfaces: ComPtr<IRenderSurfaceFactoryAbi> = host.query().unwrap();
    let surface = surfaces.create_surface(RenderSurfaceDesc::Framebuffer).unwrap();
    let groups: ComPtr<IRenderResourceGroupFactoryAbi> = host.query().unwrap();
    let group = groups
        .create_group(
            Default::default(),
            surface.as_com_ref(),
            RenderGroupPermissions {
                clear_color: true,
                write_color: true,
                write_resources: true,
                import_images: true,
                ..RenderGroupPermissions::none()
            },
        )
        .unwrap();
    let resources = group.resources().unwrap();
    let make_stream = |size: Size, resize| {
        let root = Rect {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height,
        };
        let block = sampled::block(&device.logical, &resources, root, Size { width: 1, height: 1 }, &[255; 4]);
        Stream::from_value(Stream { block, resize }).query::<IRenderStreamAbi>().unwrap()
    };
    let old = window.size().unwrap();
    let previous = make_stream(old, None);
    let info = RenderInfo {
        size: old,
        clear: 0xff111111,
        invocation: 1,
        block_count: 1,
    };
    let queries_before = WINDOW_PROBE.with_borrow(|slot| slot.as_ref().unwrap().capability_queries);
    renderer.render(info, previous.as_com_ref()).unwrap();
    let (mut acquires, format) = WINDOW_PROBE.with_borrow(|slot| {
        let probe = slot.as_ref().unwrap();
        assert_eq!(probe.capability_queries - queries_before, 2, "COM observes before and after callbacks");
        (probe.acquires, *probe.formats.last().unwrap())
    });

    let resize = Rc::new(Resize {
        native: RefCell::new(device.window.clone()),
        window: window.clone(),
        observed: Cell::new(None),
    });
    let stale = renderer
        .render(RenderInfo { invocation: 2, ..info }, make_stream(old, Some(resize.clone())).as_com_ref())
        .unwrap_err();
    let current = window.size().unwrap();
    assert_ne!(old, current);
    assert_eq!(surface.size().unwrap(), current);
    assert_eq!(resize.observed.get(), Some(current));
    assert_eq!(stale.status, NC_E_INVALID_ARG);
    WINDOW_PROBE.with_borrow_mut(|slot| {
        let probe = slot.as_mut().unwrap();
        assert_eq!(probe.acquires, acquires);
        assert!(probe.formats.as_slice() == [format]);
        probe.exclude_format = Some(format);
    });

    let next = make_stream(current, None);
    let info = RenderInfo { size: current, invocation: 3, ..info };
    renderer.render(info, next.as_com_ref()).unwrap();
    WINDOW_PROBE.with_borrow_mut(|slot| {
        let probe = slot.as_mut().unwrap();
        acquires += 1;
        assert_eq!(probe.acquires, acquires);
        assert_eq!(probe.formats.len(), 2);
        assert_ne!(probe.formats[1].as_raw(), format.as_raw());
        probe.reject_present = true;
    });

    let rejected = renderer.render(RenderInfo { invocation: 4, ..info }, next.as_com_ref()).unwrap_err();
    assert_eq!(rejected.status, RENDER_E_WINDOW_RETRY);
    assert!(rejected.message.contains("Vulkan presentation rejected an out-of-date swapchain"));
    WINDOW_PROBE.with_borrow(|slot| assert_eq!(slot.as_ref().unwrap().formats.len(), 3));
    let current = window.size().unwrap();
    let retry = make_stream(current, None);
    // Finish through both slots, including the one used by the rejected call.
    for invocation in [5, 6] {
        renderer.render(RenderInfo { size: current, invocation, ..info }, retry.as_com_ref()).unwrap();
    }
    WINDOW_PROBE.with_borrow(|slot| {
        let probe = slot.as_ref().unwrap();
        assert_eq!(probe.formats.len(), 3);
        assert_eq!(probe.acquires, acquires + 3);
    });
    assert!(rejected.message.contains("Vulkan presentation rejected an out-of-date swapchain"));
    WINDOW_PROBE.with_borrow_mut(|slot| {
        let probe = slot.as_mut().unwrap();
        probe.reject_present = true;
        probe.fail_retirement = true;
    });
    let retired = renderer
        .render(RenderInfo { size: current, invocation: 7, ..info }, retry.as_com_ref())
        .unwrap_err();
    assert_eq!(retired.status, RENDER_E_BACKEND);
    assert!(retired.message.contains("observe graphics completion") && retired.message.contains("ERROR_OUT_OF_HOST_MEMORY"));
    WINDOW_PROBE.with_borrow(|slot| assert_eq!(slot.as_ref().unwrap().retirement_errors, 1));
    renderer
        .render(RenderInfo { size: current, invocation: 8, ..info }, retry.as_com_ref())
        .unwrap();

    let acquires = WINDOW_PROBE.with_borrow_mut(|slot| {
        let probe = slot.as_mut().unwrap();
        probe.reject_acquire = true;
        probe.acquires
    });
    let rejected = renderer
        .render(RenderInfo { size: current, invocation: 9, ..info }, retry.as_com_ref())
        .unwrap_err();
    assert_eq!(rejected.status, RENDER_E_WINDOW_RETRY);
    assert!(rejected.message.contains("Vulkan swapchain changed during frame acquisition"));
    WINDOW_PROBE.with_borrow(|slot| assert_eq!(slot.as_ref().unwrap().acquires, acquires));
    renderer
        .render(RenderInfo { size: current, invocation: 10, ..info }, retry.as_com_ref())
        .unwrap();

    WINDOW_PROBE.with_borrow_mut(|slot| {
        let probe = slot.as_mut().unwrap();
        probe.fail_submit = true;
        probe.fail_retirement = true;
    });
    let terminal = renderer
        .render(RenderInfo { size: current, invocation: 11, ..info }, retry.as_com_ref())
        .unwrap_err();
    assert_eq!(terminal.status, RENDER_E_BACKEND);
    assert!(terminal.message.contains("queue_submit") && terminal.message.contains("ERROR_OUT_OF_DEVICE_MEMORY"));
    let later = renderer
        .render(RenderInfo { size: current, invocation: 12, ..info }, retry.as_com_ref())
        .unwrap_err();
    assert_eq!(later.status, RENDER_E_BACKEND);
    assert!(later.message.contains("unavailable"));
    WINDOW_PROBE.with_borrow(|slot| {
        let probe = slot.as_ref().unwrap();
        assert_eq!(probe.acquires, acquires + 2);
        assert_eq!(probe.retirement_errors, 2);
    });
    check_surface_loss_input(&device, &window);
}
