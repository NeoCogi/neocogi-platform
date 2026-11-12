//! Application-owned triangle recording shared by the executable and native consumer regressions.
use neocogi_com::{ComBox, ComPtr, NcCopyFromBufferOffset, NcStatus, com_object};
use neocogi_platform_examples::sdk::render::abi::*;
use neocogi_platform_examples::sdk::render::platform::IPlatformWindowAbi;

/// Initial OS window request; actual drawable pixels may differ through scaling or later resize.
pub const INITIAL_SIZE: Size = Size { width: 320, height: 240 };

#[com_object(interfaces(IRenderStream))]
/// Retains one sealed contribution until the synchronous renderer call finishes.
struct Stream {
    /// The application exclusively owns the immutable command block.
    block: ComBox<IRenderViewBlockAbi>,
}
impl IRenderStream for Stream {
    /// Exposes the single sealed owner only for its valid contribution index.
    fn view(&self, _: u64, index: u32) -> Result<neocogi_com::WeakComBox<IRenderViewBlockAbi>, NcStatus> {
        if index == 0 {
            Ok(self.block.downgrade())
        } else {
            Err(neocogi_com::NC_E_INVALID_ARG)
        }
    }
}

/// Draws into an existing window using the same public COM path for linked and shared-library assembly.
/// The caller retains application bootstrap ownership; this function owns the resource group and sealed work.
/// Query physical pixels before each recording, using that one value for both view bounds and submission.
/// Zero dimensions suspend without consuming a requested frame; negative dimensions reject as invalid metadata.
pub fn render_frames(window: ComPtr<IPlatformWindowAbi>, frames: u32) -> Result<(), RenderError> {
    use neocogi_platform_examples::sdk::render::source::{Bytes, PipelineDesc};
    let renderer: ComPtr<IRendererAbi> = window.query()?;
    let clients: ComPtr<IRenderClientFactoryAbi> = window.query()?;
    // Select the existing window framebuffer. This creates no second frame or attachment; the
    // window renderer below still owns acquisition, the whole-frame clear and presentation.
    let surface = window.query::<IRenderSurfaceFactoryAbi>()?.create_surface(RenderSurfaceDesc::Framebuffer)?;
    let domain = clients.create_client(RenderClientLimits::default(), surface.as_com_ref(), RenderClientPermissions::default())?;
    let resources = domain.resources()?;
    let host = window.graphics_host()?;
    let broker = host.recording_broker()?;
    let source = Bytes::source(
        br#"(module
  (vertex vs ((i u32 (builtin vertex_index))) (returns vec4 (builtin position))
    (let p ((array vec2 3) (vec2 (- 0.8) (- 0.8)) (vec2 0.8 (- 0.8)) (vec2 0.0 0.8)))
    (return (vec4 (index p i) 0.5 1.0)))
  (fragment fs () (returns vec4 (location 0)) (return (vec4 0.2 0.7 1.0 1.0))))"#
            .to_vec(),
    );
    let vertex = resources.create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))?;
    let fragment = resources.create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))?;
    let description = PipelineDesc::default().source()?;
    let pipeline = resources.create_pipeline(description.as_com_ref(), vertex.as_com_ref(), fragment.as_com_ref())?;
    let mut rendered = 0;
    while rendered < frames {
        let size = window.size()?;
        if size.width < 0 || size.height < 0 {
            return Err(neocogi_com::NC_E_INVALID_ARG.into());
        }
        if size.width == 0 || size.height == 0 {
            // Suspension submits no native frame and receives no presentation pacing.
            std::thread::sleep(std::time::Duration::from_millis(16));
            continue;
        }
        let mut frame = broker.begin_frame()?;
        let bounds = Rect {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height,
        };
        let mut session = frame.begin_view(RenderViewRoot { client: bounds, target: bounds }, resources.as_com_ref())?;
        let mut view = session.view()?;
        view.bind_pipeline(pipeline.as_com_ref())?;
        view.draw(3, 1, 0)?;
        view.finish()?;
        let stream: ComPtr<IRenderStreamAbi> = Stream::from_value(Stream { block: session.take_block()? }).query()?;
        frame.finish()?;
        renderer.render(
            RenderInfo {
                size,
                clear: 0xff181818,
                invocation: u64::from(rendered) + 1,
                block_count: 1,
            },
            stream.as_com_ref(),
        )?;
        rendered += 1;
    }
    Ok(())
}
