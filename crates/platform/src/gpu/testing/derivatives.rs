//! Shared canonical derivative pixels, exercised through real compiler/resource COM calls.
use neocogi_com::{ComBox, ComPtr, NcCopyFromBufferOffset};
use neocogi_platform::gpu::{
    GpuDevice,
    abi::*,
    source::{Bytes, PipelineDesc},
};

/// One uniform red result independent of which physical row is read.
pub struct Case {
    /// Names the operation and overload for diagnostic output.
    pub name: &'static str,
    /// S-expression module containing both legal graphics entries.
    pub source: String,
    /// Canonical red channel, rounded to RGBA8.
    pub red: u8,
}

/// Positive/negative gradients, every float/vector overload, fine/coarse and unchanged controls.
pub fn cases() -> Vec<Case> {
    let vertex = "(vertex vs ((i u32 (builtin vertex_index))) (returns vec4 (builtin position)) (let p ((array vec2 3) (vec2 -1.0 -1.0) (vec2 3.0 -1.0) (vec2 -1.0 3.0))) (return (vec4 (index p i) 0.5 1.0)))";
    let mut cases: Vec<_> = [
        ("positive scalar", "(dpdy (. p y))", 191),
        ("negative scalar", "(dpdy (- (. p y)))", 64),
        ("vec2", "(. (dpdy (. p xy)) y)", 191),
        ("vec3", "(. (dpdy (. p xyz)) y)", 191),
        ("vec4", "(. (dpdy p) y)", 191),
        ("fine", "(dpdyFine (. p y))", 191),
        ("coarse", "(dpdyCoarse (. p y))", 191),
        ("x control", "(dpdx (. p x))", 191),
        ("width control", "(fwidth (. p y))", 191),
        ("constant control", "2.0", 255),
    ]
    .into_iter()
    .map(|(name, expression, red)| Case {
        name,
        red,
        source: format!(
            r#"(module
  {vertex}
  (fragment fs ((p vec4 (builtin position))) (returns vec4 (location 0))
    (return (vec4 (clamp (+ 0.5 (* 0.25 {expression})) 0.0 1.0) 0.0 0.0 1.0))))"#
        ),
    })
    .collect();
    for (name, expression, red) in [("varying only", "(dpdy y)", 191), ("negative varying", "(dpdy (- y))", 64)] {
        cases.push(Case {
            name,
            red,
            source: format!(
                r#"(module
  (struct V (p vec4 (builtin position)) (y f32 (location 0)))
  (vertex vs ((i u32 (builtin vertex_index))) (returns V)
    (let p ((array vec2 3) (vec2 -1.0 -1.0) (vec2 3.0 -1.0) (vec2 -1.0 3.0)))
    (return (V (vec4 (index p i) 0.5 1.0) (* (- 1.0 (. (index p i) y)) 4.0))))
  (fragment fs ((y f32 (location 0))) (returns vec4 (location 0))
    (return (vec4 (clamp (+ 0.5 (* 0.25 {expression})) 0.0 1.0) 0.0 0.0 1.0))))"#
            ),
        });
    }
    cases
}

/// Records a real sealed block without a mesh, test renderer or native callback.
pub fn block<B: neocogi_platform::gpu::native::NativeResources>(
    gpu: &GpuDevice<B>,
    resources: &ComPtr<IRenderResourcesAbi>,
    source: &str,
) -> ComBox<IRenderViewBlockAbi> {
    let source = Bytes::source(source.as_bytes().to_vec());
    let vs = resources
        .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fs = resources
        .create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let descriptor = PipelineDesc::default().source().unwrap();
    let pipeline = resources.create_pipeline(descriptor.as_com_ref(), vs.as_com_ref(), fs.as_com_ref()).unwrap();
    let mut recording = gpu.broker().begin_frame().unwrap();
    let root = Rect { x: 0, y: 0, width: 8, height: 8 };
    let mut session = recording
        .begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref())
        .unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    let block = session.take_block().unwrap();
    recording.finish().unwrap();
    block
}
