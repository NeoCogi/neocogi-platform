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

Ordinary immutable `ITexture` images remain separate from exclusive `IGpuTexture`
resources. The trusted graphics host may import an authenticated ordinary image
only into its private compositor domain. Client factories reject this bridge,
and the imported resource exposes no conversion back to `ITexture`, native key,
or renderer capability. Imported images cannot be updated through the factory.

At the Rust backend boundary, `TextureId` is a process-unique map key with checked
immutable dimensions. It is not an authenticated COM resource or a certificate for
later byte slices. Every safe `RendererBackend::create_texture` validates its actual
RGBA input before ownership reservation or native work. Uploaded storage must have
exactly width × height × 4 bytes within the shared image budget; render-target
allocation checks the same extent without pretending to upload an empty slice.

Private CPU versions retain an imported image's COM lease until native capture.
Native caches and submitted draws retain the shared native image allocation,
passive weak version metadata and a device-free byte receipt. They never retain
that COM lease or the device facade it owns. Thus public image release can retire
its registry/map ownership while prepared/in-flight draws keep the pixels alive,
without a device → cache → imported image → device cycle. Texture receipts retain
logical charges through the final native owner even after private CPU bytes expire.
`IRenderHost::prepare_ui_texture` completes the fallible native preparation boundary
before a compositor publishes a candidate asset revision.

## Portable shader profile, version 1

Canonical fragment/texture coordinates have their origin at the top left; signed
Y derivatives increase downwards on both window and offscreen targets. GL/WebGL
window adaptation normalizes `dFdy` independently of fragment-position use, including
varying-only shaders. X derivatives and unsigned `fwidth` retain their native
values. The CPU compiler has no target-selection option; the existing native
program variant owns coordinate, winding, depth and differential-axis adaptation.

WGSL is the canonical source. The required `neocogi.shader.compiler` COM provider
validates and returns owned WGSL, GLSL ES 300, guarded SPIR-V and typed reflection.
The renderer imports those results; WGPU consumes returned validated WGSL. See
[shader compilation](SHADER_COMPILATION.md) for the complete boundary. No public native binary bypasses the
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
quota. During COM import the provider and receiving copy each retain a charge
for their actual live code payload; receiving admission can therefore reject a
compilation that succeeded before copying. Reflection is excluded from the byte
charge but independently bounded to 16 input/output locations and four uniform/
sampled slots per stage. Generated GLSL writes check that budget before growing their output
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

Trusted host assembly may create a private compositor domain. Its factory stays
inside GFX and shares the ordinary graphics vocabulary. Construction establishes
UI provenance; public client creation cannot select or report it. UI blocks do
not initialize or clear depth, and their pipelines cannot test or write depth.
This permits UI-only surfaces to authenticate compositor work while preserving
depth between UI overlays and neighboring client draws.

Hosts obtain a separate `IRenderHost` allocation through
`IPlatformWindow::graphics_host()`. Retaining it keeps the graphics device alive
without retaining the window COM object or its input registrations. QueryInterface
cannot recover the window, renderer or factories from it. Hosts install it through
`UiRuntime::set_render_host`. GFX prepares its private UI domain, pipeline and atlas
before the runtime publishes the matching client recording broker. A failed binding
preserves the previous complete assembly. The runtime owns one recording frame around the
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

GFX owns the UI shader, packed vertex layout and geometry expansion. It records
UI triangles and ordinary immutable images through the same generic command
vocabulary as clients. Shared triangle-aligned vertex buffers span UI operations;
client contributions seal the preceding UI block and retain their original painter
positions. The private UI domain permits neither depth writes nor depth clears, so
the following UI overlay cannot disturb a client's depth state. Offscreen surfaces
accept only blocks authenticated as private UI work, and recheck domain admission
after every foreign preparation call before allocating or clearing an output.
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
Every WGPU error scope is popped, including failures after capturing imported
ordinary-image initialization. Invalid native caches are cleared and a later valid
preparation may retry; a scoped validation error alone does not indicate device loss.
Vulkan installs its transfer owner before submission. An uncertain submit or wait
result makes that executor terminal and retains commands, staging, destination and
accounting until proven idle or device loss at teardown. A failure status alone
never permits releasing potentially queued storage or reusing that executor.
GL/WebGL likewise fences preparatory uploads even when no frame is acquired.
Frame abandonment never requires a later successful frame to retain that storage.

CPU admission/preparation rejection emits no native target commands. After native
execution starts, immediate APIs cannot promise rollback. Errors propagate;
native hosts suppress explicit failed-frame swaps. This in-process contract does
not sandbox native code, bound arbitrary shader execution time or isolate device
faults. Logical byte limits are not a complete driver VRAM/RSS guarantee.
