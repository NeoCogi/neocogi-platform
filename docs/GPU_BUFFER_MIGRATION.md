# Persistent GPU buffers during alpha

All NCIDL declarations remain version 1. This change uses the published,
unchanged `neocogi-com = 0.1.0` SDK and its existing input/output slices.
Platform builds normally, with no SDK source override:

```sh
cargo test -p neocogi-platform --lib
```

For the UI branch, include Platform's checkout when generating the overlay and
run Cargo in that UI checkout:

```sh
python3 /path/to/neocogi-platform/tools/local-sources.py \
  --platform /path/to/neocogi-platform > /tmp/neocogi-ui-local.toml
cargo check --offline --config /tmp/neocogi-ui-local.toml --workspace --all-targets
```

Cargo may refresh the local lockfile while applying patches; those development
resolutions should not replace published-source locks. Without `--offline`, Cargo
can obtain any missing ordinary dependencies.

Tests that launch their own Cargo fixture need the overlay in inherited Cargo
configuration; a top-level `--config` argument is not forwarded to child processes.
An isolated Cargo home can share the ordinary dependency cache while retaining
these temporary patches:

```sh
neocogi_cargo_dir=$(mktemp -d)
cp /tmp/neocogi-ui-local.toml "$neocogi_cargo_dir/config.toml"
ln -s "${CARGO_HOME:-$HOME/.cargo}/registry" "$neocogi_cargo_dir/registry"
CARGO_HOME="$neocogi_cargo_dir" CARGO_NET_OFFLINE=true cargo test -p neocogi-ui-runtime --lib
```

## Creation and writes

`IRenderResources` is the untyped driver resource capability. Its three creation
methods return the same exclusive `IGpuBuffer` interface:

| Method | Input | Later writes |
| --- | --- | --- |
| `create_static_buffer` | Shape and exact initial data source | Forbidden |
| `create_dynamic_buffer` | Shape/capacity only | Upload; preserves unselected contents |
| `create_streamed_buffer` | Shape/capacity only | Upload; replaces contents using three fixed slots |

The tagged shape selects Vertex, Index, Uniform or Texture. Linear variants carry
`byte_len`; Texture carries dimensions, format and mip count. Mutable buffers
start initialized to zero. A public object is created once and reused; larger
capacity requires an explicit new object. Native backing is created lazily once
per slot. Each slot is charged at creation; no rectangle mapping scratch is
required. There is no 64-allocation/4-MiB Vulkan buffer pool.

The NCIDL transfer methods are:

```rust
fn upload(&mut self, data: &[u8]) -> Result<()>;
fn read(&self, output: &mut [u8]) -> Result<()>;
```

Existing COM slice arguments already carry a logical offset. Rust calls use
`buffer.upload(NcCopyFromBufferOffset::new(byte_offset, &data))` and
`buffer.read(output.output(byte_offset, byte_count))` with a reusable
`NcOutputBuffer<u8>`. Offset and capacity refer to bytes of the resource, never
indices within the supplied slice. Both transfers finish during the call.
Textures expose tightly packed row-major RGBA bytes; row pitch is width times four.

Dynamic uploads preserve unselected bytes. Streamed uploads zero everything
outside the supplied range, including shorter tails. If all eligible slots have
recorded or native owners, upload returns Busy without changing published data or
allocating more storage. Maintain completed native work and retry. Empty uploads
are checked no-ops. Static and GPU-only imported storage reject uploads; native-only
imports also reject reads. Reads copy CPU-authored contents, not GPU readback.

GL/WebGL use SubData/TexSubImage; WGPU uses queue writes into the same native
destinations. Vulkan refreshes persistent mapped buffer storage or copies reusable
staging into the same image. The public object and bounded destination storage
remain persistent; drivers may still perform internal staging work.

Uniforms now use Uniform-role buffers and `bind_uniform_buffer`. Ranges have
256-byte-aligned offsets, exact reflected block lengths and a 16-KiB size limit.
The copied `push_uniforms`, factory update methods, optional initial buffer data
and separate `IGpuTexture` interface have been removed. Ordinary immutable
`ITexture` images still exist and import as static Texture-role buffers.

## Typed application code

`TypedGpuBufferFactory::new(resources)` takes `ComPtr<IRenderResourcesAbi>`.
It sits above the untyped driver and wraps the original returned objects. The
application defines its vertex types and their exact shader packing:

```rust
use neocogi_platform::render::buffer::{GpuElement, TypedGpuBufferDesc, TypedGpuBufferFactory};

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex { position: [f32; 3], color: [u8; 4] }
// SAFETY: This 16-byte layout has no padding, ownership or restricted bit patterns
// and matches this application's explicit vertex shader/pipeline layout.
unsafe impl GpuElement for Vertex {}

// resources: ComPtr<IRenderResourcesAbi>
let factory = TypedGpuBufferFactory::new(resources);
let mut vertices = factory.create_streamed::<Vertex>(
    TypedGpuBufferDesc::Vertex { element_count: 1024 }
)?;
vertices.upload(0, &frame_vertices)?;
view.bind_vertex_buffer(0, vertices.as_gpu_buffer(), 0)?;
```

`vertices.upload(first_element, &values)` and `vertices.read(first_element,
&mut output)` use element offsets, including for four-byte texture texels. Bounds
and multiplication are checked before calling the byte-oriented driver.

NCIDL also declares `ITypedGpuBuffer<T>: IGpuBuffer` for application COM
specializations. Its `upload_elements(data: &[T])` and
`read_elements(output: &mut [T])` use element offsets and the normal COM codecs. Drivers
never instantiate application vertex types. Such a wrapper returns its original
registered resource through `underlying_buffer()` because COM inheritance does
not substitute the wrapper's object address for the driver's authenticated one.
The Rust wrapper's `as_gpu_buffer()` serves that same purpose without creating
another COM identity.

The `neocogi-typed-buffer-tests` package compiles an application-owned
`ITypedGpuBuffer<Point>` specialization, exercises its inherited ABI and checks
element/byte offset conversion and that copied results survive wrapper destruction.

The complete state, accounting and backend lifetime contract is in
[GPU_RENDERING.md](contracts/GPU_RENDERING.md); the authoritative interface is
[gpu.ncidl](../crates/platform/schemas/neocogi/renderer/gpu.ncidl).
