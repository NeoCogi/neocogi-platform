//! Observe real compiler allocations across the generated COM resource factory.
use neocogi_com::{NcCopyFromBufferOffset, NC_E_OUT_OF_RANGE};
use neocogi_render_gpu::{abi::*, shader::Shader, source::Bytes, GpuDevice};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

thread_local! {
    /// Only this regression thread participates; other harness allocations are irrelevant.
    static OBSERVING: Cell<bool> = const { Cell::new(false) };
    /// Largest allocation or reallocation requested during compilation.
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

/// Allocation observer delegates all storage and failure behavior to the system allocator.
struct Allocator;
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

/// Never allocates or panics, including during thread-local teardown.
fn observe(size: usize) {
    if OBSERVING.try_with(Cell::get).unwrap_or(false) {
        let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(size)));
    }
}

// SAFETY: every request and deallocation is forwarded unchanged; observation owns no memory.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        observe(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        observe(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        observe(size);
        unsafe { System.realloc(pointer, layout, size) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }
}

/// A tiny valid input expands to over one MiB when zero-initialized arrays are emitted in GLSL.
const EXPANDING: &str = r#"
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    var values: array<vec4<f32>, 100000>;
    values[u32(p.x)] = vec4<f32>(0.25);
    return values[0];
}
"#;

/// The factory must stop output expansion before allocating the artifact it cannot admit.
#[test]
fn generated_output_obeys_remaining_domain_quota_before_large_allocation() {
    let device = GpuDevice::new();
    let domain = device.create_client(RenderClientLimits { live_bytes: 65536, ..Default::default() }).unwrap();
    let factory = domain.resources().unwrap();
    let source = Bytes::source(EXPANDING.as_bytes().to_vec());
    LARGEST.with(|largest| largest.set(0));
    OBSERVING.with(|observing| observing.set(true));
    let result = factory.create_shader(GpuShaderStage::Fragment, source.as_com_ref(), NcCopyFromBufferOffset::new(0, b"fs"));
    OBSERVING.with(|observing| observing.set(false));
    let largest = LARGEST.with(Cell::get);
    assert!(matches!(result, Err(NC_E_OUT_OF_RANGE)));
    // Leave room for compiler scratch allocations: this measures actual early rejection,
    // rather than merely observing the status returned after an unbounded String grows.
    assert!(largest <= 256 * 1024, "compiler requested {largest} bytes before quota rejection");
    assert_eq!(device.live_bytes(factory.as_com_ref()).unwrap(), 0);
    let artifact = Shader::compile(GpuShaderStage::Fragment, EXPANDING, "fs").unwrap();
    assert!(artifact.glsl.len() > 1024 * 1024, "the fixture must exercise real output amplification");
}
