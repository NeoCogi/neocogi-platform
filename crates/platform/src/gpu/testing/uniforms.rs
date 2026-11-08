//! Exact logical uniform spans shared by native GL and strict browser WebGL acceptance.
use crate::gpu::{
    GpuDevice,
    abi::*,
    native::NativeResources,
    source::{Bytes, PipelineDesc},
};
use neocogi_com::{ComBox, ComPtr, NcCopyFromBufferOffset, NcOutputBuffer};

/// One recorded public-COM draw and its still-owned logical uniform buffer.
pub struct Fixture {
    /// Sealed commands retain the exact selected logical bytes.
    pub block: ComBox<IRenderViewBlockAbi>,
    /// Public storage remains exact even when its native backing has private padding.
    pub buffer: ComBox<IGpuBufferAbi>,
}

/// Compile and record a scalar-member block ending exactly at public capacity.
/// The final member supplies red=1; a prefix before an offset binding contains sentinel bytes.
pub fn fixture<B: NativeResources>(gpu: &GpuDevice<B>, resources: &ComPtr<IRenderResourcesAbi>, members: u32, offset: u64, usage: GpuBufferUsage) -> Fixture {
    assert!((1..=8).contains(&members));
    let fields = (0..members).map(|index| format!("v{index}:f32,")).collect::<String>();
    let wgsl = format!(
        r#"
        struct Values {{ {fields} }}
        @group(3) @binding(0) var<uniform> values:Values;
        @vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4<f32> {{
            let p=array<vec2<f32>,3>(vec2(-1.,-1.),vec2(3.,-1.),vec2(-1.,3.));
            return vec4(p[i],0.5,1.);
        }}
        @fragment fn fs()->@location(0) vec4<f32> {{ return vec4(values.v{},0.,0.,1.); }}
    "#,
        members - 1
    );
    let source = Bytes::source(wgsl.into_bytes());
    let vertex = resources
        .create_shader(GpuShaderStage::Vertex, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"vs"))
        .unwrap();
    let fragment = resources
        .create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"))
        .unwrap();
    let span = members * 4;
    let descriptor = PipelineDesc {
        bindings: vec![GpuShaderBinding {
            stage: GpuShaderStage::Fragment,
            slot: 0,
            kind: GpuBindingKind::Uniform(span),
        }],
        ..Default::default()
    }
    .source()
    .unwrap();
    let pipeline = resources
        .create_pipeline(descriptor.as_com_ref(), vertex.as_com_ref(), fragment.as_com_ref())
        .unwrap();
    let capacity = offset + u64::from(span);
    let desc = GpuBufferDesc::Uniform { byte_len: capacity };
    let mut bytes = vec![0xa5; capacity as usize];
    bytes[offset as usize..].fill(0);
    bytes[capacity as usize - 4..].copy_from_slice(&1f32.to_le_bytes());
    let buffer = match usage {
        GpuBufferUsage::Static => resources.create_static_buffer(desc, Bytes::source(bytes.clone()).as_com_ref()).unwrap(),
        GpuBufferUsage::Dynamic => {
            let mut buffer = resources.create_dynamic_buffer(desc).unwrap();
            buffer.upload(NcCopyFromBufferOffset::new(0, &bytes)).unwrap();
            buffer
        }
        GpuBufferUsage::Streamed => {
            let mut buffer = resources.create_streamed_buffer(desc).unwrap();
            buffer.upload(NcCopyFromBufferOffset::new(0, &bytes)).unwrap();
            buffer
        }
        _ => panic!("fixture usage must be known"),
    };
    let mut read = NcOutputBuffer::new();
    buffer.read(read.output(0, capacity as usize)).unwrap();
    assert_eq!(read.as_slice(), bytes, "native padding must not enter public contents");
    let root = Rect { x: 0, y: 0, width: 8, height: 8 };
    let mut frame = gpu.broker().begin_frame().unwrap();
    let mut session = frame.begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref()).unwrap();
    let mut view = session.view().unwrap();
    view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    // Enlarging the public range to native padding remains invalid.
    if !span.is_multiple_of(16) {
        assert!(
            view.bind_uniform_buffer(GpuShaderStage::Fragment, 0, buffer.as_com_ref(), offset, u64::from(span.next_multiple_of(16)))
                .is_err()
        );
        // A rejected view is terminal, so use a fresh session for the accepted exact range.
        drop(view);
        session.expire().unwrap();
        session = frame.begin_view(RenderViewRoot { client: root, target: root }, resources.as_com_ref()).unwrap();
        view = session.view().unwrap();
        view.bind_pipeline(pipeline.as_com_ref()).unwrap();
    }
    view.bind_uniform_buffer(GpuShaderStage::Fragment, 0, buffer.as_com_ref(), offset, u64::from(span))
        .unwrap();
    view.draw(3, 1, 0).unwrap();
    view.finish().unwrap();
    let block = session.take_block().unwrap();
    frame.finish().unwrap();
    Fixture { block, buffer }
}
