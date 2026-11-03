//! A complete platform/graphics application using only public COM contracts.
#[cfg(not(target_arch = "wasm32"))]
use neocogi_com::{ComBox, ComPtr, NcCopyFromBufferOffset, NcStatus, com_object};
#[cfg(not(target_arch = "wasm32"))]
use neocogi_render_core::abi::*;

#[cfg(not(target_arch = "wasm32"))]
#[com_object(interfaces(IRenderStream))]
/// Retains one sealed contribution until the synchronous renderer call finishes.
struct Stream {
    /// The application exclusively owns the immutable command block.
    block: ComBox<IRenderViewBlockAbi>,
}
#[cfg(not(target_arch = "wasm32"))]
impl IRenderStream for Stream {
    fn view(&self, _: u64, index: u32) -> Result<neocogi_com::WeakComBox<IRenderViewBlockAbi>, NcStatus> {
        if index == 0 {
            Ok(self.block.downgrade())
        } else {
            Err(neocogi_com::NC_E_INVALID_ARG)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Owns one example session and records a procedural triangle in a framebuffer-bound resource group.
/// The window renderer owns the whole-frame clear and presentation; group recording only supplies commands.
fn main() -> Result<(), NcStatus> {
    use neocogi_render_core::source::{Bytes, PipelineDesc};
    let platform = neocogi_platform_examples::acquire_platform()?;
    let size = Size { width: 320, height: 240 };
    let window = platform.create_window(NcCopyFromBufferOffset::new(0, b"NeoCogi Platform triangle"), size)?;
    let renderer: ComPtr<IRendererAbi> = window.query()?;
    let clients: ComPtr<IRenderClientFactoryAbi> = window.query()?;
    // Select the existing window framebuffer. This creates no second frame or attachment; the
    // window renderer below still owns acquisition, the whole-frame clear and presentation.
    let surface = window.query::<IRenderSurfaceFactoryAbi>()?.create_surface(RenderSurfaceDesc {
        kind: RenderSurfaceKind::Framebuffer,
        size: Size::default(),
    })?;
    let domain = clients.create_client(RenderClientLimits::default(), surface.as_com_ref(), RenderClientPermissions::default())?;
    let resources = domain.resources()?;
    let host = window.graphics_host()?;
    let broker = host.recording_broker()?;
    let source = Bytes::source(
        br#"
        @vertex fn vs(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
            let p = array<vec2<f32>,3>(vec2(-0.8,-0.8), vec2(0.8,-0.8), vec2(0.0,0.8));
            return vec4(p[i], 0.5, 1.0);
        }
        @fragment fn fs() -> @location(0) vec4<f32> { return vec4(0.2, 0.7, 1.0, 1.0); }
    "#
        .to_vec(),
    );
    let vertex = resources.create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))?;
    let fragment = resources.create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))?;
    let description = PipelineDesc::default().source()?;
    let pipeline = resources.create_pipeline(description.as_com_ref(), vertex.as_com_ref(), fragment.as_com_ref())?;
    let frames = std::env::args()
        .nth(1)
        .map(|value| value.parse::<u32>())
        .transpose()
        .map_err(|_| neocogi_com::NC_E_INVALID_ARG)?
        .unwrap_or(3);
    for _ in 0..frames {
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
                invocation: 1,
                block_count: 1,
            },
            stream.as_com_ref(),
        )?;
    }
    println!("Rendered {frames} platform frames.");
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn main() {}
