# Generic graphics resource and recording contracts

The renderer ABI describes buffers, sampled textures, samplers, shaders,
graphics pipelines and bounded draw commands. Semantic meshes, cameras and
materials belong to clients. GPU payloads use explicit bytes, vertex formats and
uniform layouts; Rust or generated ncidl record layout never defines GPU layout.
Rust clients can publish initialized bytes with `source::Bytes` and typed pipeline
arrays with `source::PipelineDesc`; these helpers define no semantic geometry.
The [Suzanne widget](https://github.com/NeoCogi/neocogi-ui/blob/master/examples/full/src/mesh.rs) defines its own 32-byte
position/normal/UV layout, programmable lighting and two explicitly encoded
camera matrices. Its application retains the domain controller and grants the
widget only a resource factory. The [smoke widget](https://github.com/NeoCogi/neocogi-ui/blob/master/examples/smoke/src/gpu.rs)
uses a separate color-interpolation pipeline. Both consume the same scoped
binding, uniform and primitive-draw operations.

The standalone [glTF examples](../../crates/platform/plugin/examples/gltf/README.md)
similarly own their scene import, arcball camera, materials and animation. Their
64-byte vertex streams and 64-matrix uniform palettes exercise indexed, textured,
depth-tested drawing and GPU skinning through these same generic contracts.
Scene/node/skin semantics remain outside the renderer.
Their image-based lighting similarly packs RGBE radiance, filtered environment
levels and BRDF data into ordinary 2D images; the example owns HDR decoding,
material atlases and skybox semantics. This does not extend the portable texture
formats, mip count or sampled-slot limits.

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

Initial buffer and sampled-image contents use the checked borrowed optional value
`&Option<ComPtr<IGpuDataSource>>` in NCIDL. `None` means descriptor-sized zero
contents, not undefined memory. It reserves the full extent without a CPU zero
vector. `Some` supplies exactly the required bytes; empty/short/long sources reject.
Updates and shader source remain required inputs. Use `source::initial` or
`source::initial_bytes` and borrow the generated `InitialGpuData` alias for Rust
calls. That alias derives from the actual generated method rather than a copied
specialization UUID. Receiver-owned source references and their foreign Release
calls end before final admission and publication.

Driver-bound resources, factories and renderer/surface capabilities expose the
same opaque `IRenderDriverIdentity` for one compatible native driver instance.
Independent devices/contexts return distinct retained tokens. Tokens own no native
core or renderer authority and are process-local metadata, not persistent IDs.
Matching public metadata never substitutes for construction-proven registration
and same-device/domain admission. Portable byte sources and compiler artifacts
remain independent of a driver.

The trusted host calls `IRenderHost::maintain` before GFX allocates frame geometry.
It observes completion and reclaims safe storage without acquiring, clearing or
presenting a target. This entry remains reachable when quota pressure would reject
lowering before the normal renderer call. Actual in-flight owners remain charged;
public release does not promise immediate refunds. Native transaction exits also
maintain completed work on success and failure, including preparation without a
target. This changes neither CPU-frame abort nor actual GPU-completion rules.

Creation and update publish complete CPU logical resources/versions. Native
allocation, driver compilation and upload remain fallible preparation work.
Failure there rejects the frame and leaves the logical version retryable; it
cannot silently roll version B back to A or rewrite previously captured A draws.

Ordinary immutable `ITexture` images remain separate from exclusive `IGpuTexture`
resources. The trusted graphics host may import an authenticated ordinary image
only into its private compositor domain. Client factories reject this bridge,
and the imported resource exposes no conversion back to `ITexture`, native key,
or renderer capability. Imported images cannot be updated through the factory.

At the Rust backend boundary, native images are opaque owned values. Every safe
`RendererBackend::create_texture(size, pixels)` validates the actual RGBA slice
before native work; absent data is initialized to zero. The checked extent is
width × height × 4 bytes within the shared image budget. No caller-generated image
key or explicit destruction call exists.

`create_render_texture(size)` returns an unpublished single-use output. The caller
moves it into `texture_frame`; successful `finish` returns `Some(image)` for that
output and `None` for a window. Failure or abandonment publishes no image. An
immutable image cannot be passed back as a draw target. The exact image and
framebuffer follow every actual native use, including a zero-block host clear
and uncertain submission. A retained completed image never changes on later
frames, resize, or surface release.

Imports capture the authenticated native image directly after receiver-owned
foreign QI/AddRef/Release calls end. They retain no public-image COM lease. An
ordinary image owns native pixels and physical device/context lifetime without
retaining the driver facade, event pump, UI runtime, or provenance registry. Each
import's logical reservation follows that import and its prepared/in-flight use;
an unrelated long-lived alias does not retain that import's charge.
`IRenderHost::prepare_ui_texture` completes fallible native preparation before a
compositor publishes a candidate asset revision.

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
of a healthy recording leaves an empty position. Unfinished or rejected work fails
the paint invocation; retrying begin cannot erase that failure. Every return
revokes escaped facades.

The host session owns pending CPU commands; the exclusive client view holds only
a weak link and its bounded shell charge. Dropping that view releases the shell
without changing the session's outcome. A dropped unfinished view therefore stays
`Recording`, with its payload charged, until the host harvests or expires it.
UI harvesting runs after `IWidget::paint` returns and before the paint service
returns its combined result, or before a later begin in the same invocation. It
rejects unfinished work and expires the session, releasing its payload before
returning the failure. A trusted host retaining a session outside UI traversal
must expire/drop it, abort its recording frame or close its domain to release
unfinished payload. The existing pending-byte limits remain in force throughout.

Cancellation changes only `Recording` to `Cancelled` and immediately releases its
payload. It is idempotent; a `Rejected` session retains its first command error,
and a `Committed` session retains its sealed contribution. Rejection releases
payload immediately. Neither cancel nor client Drop can turn rejection into
success. Host expiry is terminal, and a committed block transfers exactly once.
Frame finish rejects live unfinished/rejected sessions. Attempted-work charges
are never refunded by cancellation, destruction or failed recording. These CPU
rules are separate from native frame maintenance and GPU completion below.

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
complete pipeline/binding state per draw. Every native allocation retains the
actual physical context, including SDL ownership or the browser context. A GL
handle's destruction first selects its own context. Selection failure or observed
WebGL loss tears down that physical owner terminally; restoration requires a new
core and identity. Supported operations reselect their context after late cleanup
on another context. A bare GL dispatch table is not a lifetime guard.

WGPU prepares explicit layouts, pipelines and immutable bindings before acquiring
the target. Regional replacement uses private scissored clear pipelines, and
ordered views preserve shared depth through load/store passes. Submission retains
native allocations and device-free byte/capacity receipts until the queue's
completion callback is observed. That callback owns only an atomic completion
flag; UI-thread resource owners stay in the executor's completion collection.

Vulkan prepares explicit buffers, sampled images, descriptor sets and compatible
pipelines before target acquisition. Native allocations retain only the physical
device core and opaque shared accounting. Submitted blocks remain owned through their
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
ordinary-image initialization. Each candidate's scopes finish before its native
slot becomes Ready. A failed candidate leaves its own slot retryable and preserves
every successful sibling target variant. No global cache rollback exists. A scoped
validation error alone does not indicate device loss. Initialization and frame
submission share one scoped queue boundary that installs exact owners before
submit. A submit error may have consumed pending writes: it marks the compatible
core unavailable and retains uncertain owners until final idle/loss teardown.
Another facade cannot reuse that core's Ready slots to bypass failed initialization.
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


## Immutable native preparation data and identity

Captured versions, admitted block roots/commands and shared uniform storage expose
only read access. Dropping public COM ownership or obtaining a unique Rc never
allows descriptors, shader artifacts, byte storage or command authority to change.
Factory updates publish new versions; earlier captures retain their exact data and
charges. Editable input descriptors and owned compiler-import DTOs remain ordinary
authoring data before admission. Native pixel uploads also check the actual slice
against its extent before allocating or passing pointers to GL.

The common resource, command and recording types are parameterized by one concrete
`NativeResources` family. That Rust type selects native storage and a physical core;
COM interfaces retain their existing v1 roles and contain no native handles. Each
immutable version owns its native slot, and each pipeline version owns its target
variants. Prepared objects retain native allocations and opaque accounting receipts,
never the parent version or a client domain. Uniform payloads use the same mechanism.
There are no renderer-global resource maps, numeric version IDs, source-count scans,
or passive weak realization receipts. Small host-only regional-clear variants are
driver state, with no application resource identities.

Every source, imported image, prepared block and consumed output checks its actual
retained core against the selected driver. Two logical producers may share one
explicit compatible core without sharing resource/domain provenance. A matching
backend type or metadata token alone does not establish compatibility. Terminal
native loss invalidates previously Ready slots as well as future creation.

A last native backing owner transfers a sender-free owned packet through mpsc to
the existing driver. Packets contain native handles, narrow physical lifetime and
opaque reservations; they contain no driver, domain, source version or sender.
Core owns neither endpoint nor submitted work. Completion retains the actual
backing before native use, so final-owner notification is not proof of completion.
Maintenance observes completion, releases its owners and drains retirement packets.
It runs before CPU admission, on frame exits, failures, abandonment and shutdown.
Idle releases wait for maintenance; a disconnected send returns the same owned
packet for safe late destruction. Native storage is destroyed before its final
reservation refunds. Metadata alone cannot retain the physical core.

WGPU presentation construction, resize and lost/outdated recovery use one checked
fallible target transaction. Extents are validated against the actual device before
surface configure/depth allocation. The candidate configuration and depth owner
are installed only after native scopes succeed. Before a destructive configuration
attempt the presentation state becomes NeedsConfiguration. Failure retains a retry
template, and even a frame requesting the previous size must reconfigure before
acquisition. Acquisition itself also scopes unexpected native errors. Presentation
has a separate scoped phase after successful recording/submission validation;
its native errors return through finish and require reconfiguration. Oversized
precheck failures preserve a complete existing assembly and return errors.

## Vulkan presentation failure boundary

Presentation readiness covers the complete swapchain generation as well as acquired
image/semaphore ownership. Destructive reconstruction marks presentation unavailable
before waiting or replacing parts. It restores its previous Ready or Acquired phase
only after complete success; any reconstruction error is terminal for later native
preparation, upload, output creation and acquisition. An outer acquired finalizer
preserves a prelatched failure and its original error. No extent equality can make
an incomplete generation reusable. Initial zero dimensions reject before bootstrap;
runtime minimization of a complete context remains a no-op.

Acquisition out-of-date recovery owns no frame token. A successful rebuild returns
the existing transient frame error while leaving Ready for retry; a generic frame
error is not itself proof of terminal presentation failure. Device loss, generic
upload completion and offscreen submission keep their separate owners/states.
Renderer usability derives from them instead of caching another disabled flag.
Retirement and teardown still observe native completion before releasing resources.
