# Generic graphics resource and recording contracts

The renderer ABI describes buffers, sampled textures, samplers, shaders,
graphics pipelines and bounded draw commands. Semantic meshes, cameras and
materials belong to clients. GPU payloads use explicit bytes, vertex formats and
uniform layouts; Rust or generated ncidl record layout never defines GPU layout.
Rust clients can publish initialized bytes with `source::Bytes` and typed pipeline
arrays with `source::PipelineDesc`; these helpers define no semantic geometry.
The [Suzanne widget](../../examples/full/src/mesh.rs) defines its own 32-byte
position/normal/UV layout, programmable lighting and two explicitly encoded
camera matrices. Its application retains the domain controller and grants the
widget only a resource factory. The [smoke widget](../../examples/smoke/src/gpu.rs)
uses a separate color-interpolation pipeline. Both consume the same scoped
binding, uniform and primitive-draw operations.

Each public GPU resource has one `ComBox` owner. Renderers observe its lifetime
through construction-proven `WeakLocalBox` and typed `WeakComBox` registrations.
Borrowed interface arguments validate identity, not resource liveness. Resource
capture takes the provider's local reader gate; mutation commits under its writer
gate. Neither guard spans a foreign call or owner destruction. Identity checks
use registered canonical allocation addresses, never a caller's owner number,
downcast or shared-owner promotion.

Bindings weakly observe public resources. Each draw checks current liveness and
captures exact immutable backing versions, complete bindings, copied uniforms
and bounded view/clip state. Owner expiry prevents later capture, while already
recorded work retains its backing through GPU completion. Pipelines retain shader
artifacts independently of public shader owners. Weak expiry is not a callback
or proof of GPU completion. Retirement releases only storage whose actual owners
and completion requirements are satisfied.

Creation and update publish complete CPU logical resources/versions. Native
allocation, driver compilation and upload remain fallible preparation work.
Failure there rejects the frame and leaves the logical version retryable; it
cannot silently roll version B back to A or rewrite previously captured A draws.

## Portable shader profile, version 1

WGSL is the canonical source. Shared validation/reflection emits GLSL ES 300 and
SPIR-V, while WGPU consumes the same WGSL. No public native binary bypasses the
profile. Vertex and fragment stages support read-only uniform blocks and paired
non-comparison sampled 2D float textures/samplers. Storage, atomics, compute,
workgroup state, binding arrays and additional color attachments are rejected,
including unused declarations. A fragment entry emits exactly one `vec4<f32>`
color at location zero. Portable location inputs use f32/u32 scalars or vectors.
Shader linkage compares locations, shapes, interpolation and sampling qualifiers.

Dynamic array and uniform-buffer accesses remain within valid storage. GL and
Vulkan explicitly return zero for out-of-bounds reads and skip out-of-bounds
private array writes. SPIR-V contains these guards. GLSL is emitted from the
guarded SPIR-V imported back into compiler IR, because the GLSL writer does not
itself lower array/buffer bounds policies. WGPU applies its checked translation
to canonical WGSL; that translation may clamp an invalid index instead. Clients
must not depend on a particular out-of-bounds result. Native robustness extensions
are not required by the GL/Vulkan guards. Fragment-coordinate adaptation recognizes
complete built-in identifier tokens and preserves ordinary identifiers containing
the same text.

WGSL source is limited to one MiB. Combined owned source, entry name, SPIR-V and
GLSL are limited to four MiB per shader and the domain's remaining live-byte
quota. Generated GLSL writes check that budget before growing their output
allocation. Exhausted output quota returns `NC_E_OUT_OF_RANGE`; failed bounded
output allocation returns `NC_E_OUT_OF_MEMORY`. These limits cover retained
artifacts and generated text, rather than all compiler scratch memory or runtime.

Min/max color and alpha blending compare unscaled source/destination values;
their factor fields have no effect. Native adapters normalize ignored factors
where their API requires a canonical value (WebGPU requires both to be One).

Binding groups are 0 for vertex sampled pairs, 1 for vertex uniforms, 2 for
fragment sampled pairs and 3 for fragment uniforms. There are four slots per
stage/class. A sampled slot uses texture binding `2*slot` and sampler binding
`2*slot+1`. Uniform binding equals slot. Reflection supplies generated GLSL block
and sampler names privately; clients never depend on native uniform names.

Uniform bytes use explicit column-major layouts whose member offsets, matrix
column strides and array strides agree with std140. Differing layouts reject.
Final native block padding may be added privately. Each uniform input must match
its reflected byte length. Vertex streams have explicit byte offsets, stride,
step mode and formats. Index and vertex buffer roles are distinct for WebGL.
The common profile limits strides to 252 bytes: WebGL 2 caps stride at 255,
while WebGPU requires four-byte stride alignment. Attribute offsets use the
minimum of four and the format size, including four-byte alignment for Unorm8x4.
See the [WebGL 2 attribute rules](https://registry.khronos.org/webgl/specs/latest/2.0/#5.3)
and [WebGPU vertex layout validation](https://gpuweb.github.io/gpuweb/#validating-gpuvertexbufferlayout).

## Scoped graphics authority

The host selects attachments and the final visible client root. Nested viewport
mapping and effective clipping are independent. Every client draw and regional
clear remains within the immutable physical root after checked DPI conversion.
A pipeline cannot bind another target, sample UI attachments, change submission
order or acquire native frame state. The graphics profile exposes no storage
writes or compute that could bypass attachment scissoring.

One trusted CPU traversal accumulates per-domain attempted-work charges across
widgets and sequential views. Cancel/reset never refund attempted work. Host
abort retires pending storage even without a GPU submission; successful finish
preserves transferred immutable blocks. Invocation-bound painter and view objects
cannot regain authority through a later widget's valid public token.

Hosts install the separate `IRenderHost::recording_broker()` result through
`UiRuntime::set_render_broker`. The runtime owns one recording frame around the
entire paint traversal. Widgets receive only `IPainter::begin_render_view` and
their restricted resource factory. Begin reserves the contribution's display-list
position immediately. Finish seals it; the runtime transfers the block at that
original position before another view begins or paint returns. Explicit cancel
leaves an empty position. Unfinished or rejected work fails the paint invocation;
retrying begin cannot erase that failure. Every return revokes escaped facades.

The view's `info().client` is the actual visible client rectangle in widget-local
coordinates. Its initial viewport is `(0,0,width,height)` relative to that root.
The runtime derives both from committed content geometry and clipping; caller
tokens or supplied painter rectangles cannot redefine them. Sealed blocks have
exclusive owners through synchronous GFX submission and weak stream observers.
The renderer authenticates them before native frame acquisition and rechecks all
domain gates after the final foreign stream/image callback.

Normalized graphics positions use positive Y upward and depth in `[0,1]`.
Sampled texture row zero and fragment-position Y use the top-left convention.
The GL executor privately normalizes direct presentation versus offscreen image
storage, including fragment coordinates and front-face winding. It restores
complete pipeline/binding state per draw. Its native caches retain device-free
immutable versions; recorded blocks and domain controllers retain native lifetime
separately, preventing an idle device-cache-domain ownership cycle.

WGPU prepares explicit layouts, pipelines and immutable bindings before acquiring
the target. Regional replacement uses private scissored clear pipelines, and
ordered views preserve shared depth through load/store passes. Submission retains
native allocations and device-free byte/capacity receipts until the queue's
completion callback is observed. That callback owns only an atomic completion
flag; UI-thread resource owners stay in the executor's completion collection.

Vulkan prepares explicit buffers, sampled images, descriptor sets and compatible
pipelines before target acquisition. Native versions retain a cache-free device
core and device-free accounting. Submitted blocks remain owned through their
graphics fence, including presentation failure. Memory and sampler counts are
checked device-wide, including host images and UI allocations, and are refunded
only after native destruction. Image format/usage, dimensions and attachment
limits are checked before native creation. Native preparation errors remain
fallible rather than invalid Vulkan calls.

Vulkan command buffers belong to waited frame-slot fences. Presentation wait
semaphores belong to swapchain images; reacquisition orders their reuse, because
a graphics fence alone does not prove presentation consumed its semaphore.
See the [Khronos synchronization guidance](https://docs.vulkan.org/guide/latest/swapchain_semaphore_reuse.html).
This backend retains the conventional device-idle swapchain teardown used by
unextended Vulkan; it does not enable presentation-completion fences from
`VK_EXT_swapchain_maintenance1`.

Preparation uploads have their own completion lifetime, including after native
preparation failure or abandoned target acquisition. WGPU flushes initialization
on the existing queue and retains typed buffer/image/uniform owners and command
receipts until upload completion is observed. Explicit scoped buffer creation and
aligned queue writes return native allocation errors without mapped-helper panics.
GL/WebGL likewise fences preparatory uploads even when no frame is acquired.
Frame abandonment never requires a later successful frame to retain that storage.

CPU admission/preparation rejection emits no native target commands. After native
execution starts, immediate APIs cannot promise rollback. Errors propagate;
native hosts suppress explicit failed-frame swaps. This in-process contract does
not sandbox native code, bound arbitrary shader execution time or isolate device
faults. Logical byte limits are not a complete driver VRAM/RSS guarantee.
