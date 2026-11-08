# Portable shader compilation, version 1

`neocogi.shader.compiler` is a synchronous CPU-only COM provider. A renderer
requires and acquires `ShaderCompiler` during activation, before SDL, DOM or GPU
construction. Native dynamic packaging stages `nc_shader_compiler` before the
renderer; monolith and WebGL assemblies register the same provider statically.
Missing dependencies or incompatible roots fail activation. The renderer has no
linked compiler fallback, widget-facing compiler getter or per-draw lookup.

The namespace lives in `crates/platform/schemas/neocogi/shader`. The SDK shader module supplies
the generated ABI, plain reflection and bounded readers without Naga. The
provider's private implementation owns Naga. It uses the SDK render module's existing
`GpuShaderStage`, preserving the one-way contract dependency.

## Outcomes and owned exports

The Rust `compile(request, source, entry)` facade accepts canonical UTF-8 WGSL, a selected entry,
one explicit `ShaderTarget` and a selected-output byte cap. There is one supported read-only graphics
contract, documented in [GPU rendering](GPU_RENDERING.md). The compiler has no
viewport, surface, Window/Texture, target height or arbitrary profile selector.

Rust callers receive `Result<ComPtr<IShaderCompilationAbi>, RenderError>`: an error
owns its status and message before the call returns. Raw COM clients use
`compile_result` to obtain the report for that same operation.

A report is either complete success or rejection with an original failing status
and diagnostic. Success requires parsing, whole-module admission/validation,
selected-stage/entry agreement and emission of the one requested artifact within
quota. Forbidden unused declarations and other entries cannot bypass admission.
WGPU's native checked translation and native linking/allocation stay fallible.

Portable admission uses the complete Naga-validated module: every entry's IO,
stage-local resource use and image/sampler combinations must satisfy the profile.
Every declared uniform has a named structure root with portable layout and at most
16 KiB, including unused blocks. A used image must participate in its entry's valid
sampled pair; load/query-only use does not invent that pair. Paired textureLoad
reads at constant mip level 0 and textureDimensions with omitted/constant level 0
produce complete native artifacts. Signed and unsigned integer coordinates are
accepted. Other levels, dynamic level selection, mip-count, layer-count and
sample-count queries reject, including operations in unused functions/entries.
This is the common single-mip 2D image contract, not admission-only support.
Only the requested entry is emitted/reflected; inactive entries cannot alter its
selected collections.

## Selected target and build features

| Device | Requested target | Compiler output features | Naga features |
| --- | --- | --- | --- |
| Vulkan | `SpirV` | `shader-spirv` | `wgsl-in,spv-out` |
| GLES3 / WebGL2 | `GlslEs300` | `shader-glsl` | `wgsl-in,spv-out,spv-in,glsl-out` |
| WGPU | `Wgsl` | `shader-wgsl` | `wgsl-in` |

The backend forwards the corresponding output feature without enabling the optional
compiler. A standalone CPU provider selects `shader-compiler` and output features
without SDL, a Vulkan loader or WGPU. Output features are additive, so an explicitly
multi-target compiler supports their union, but each request returns one format.
`test-support` includes WGSL for CPU fixtures and does not enable either native writer.
Naga defaults are disabled; no WGSL emitter is required. WGPU's own native drivers
can independently enable Naga writers through Cargo feature unification.

The device's immutable native resource family chooses its target once before the
foreign compiler call. Public resource clients cannot substitute another target.
A known format absent from the compiler build produces a rejected report with
`SHADER_E_TARGET_UNAVAILABLE`; unknown enum tags reject at the generated ABI boundary.
There is no output substitution or fallback to the request source.

A successful report has an independent `target()` getter and exactly one code
export: `wgsl()`, `spirv()` or `glsl()`. Other code getters return `NC_E_NOINTERFACE`.
`ShaderInfo.target` and `code_byte_count` summarize those same objects, replacing the
former three mandatory format lengths. An importer authenticates the requested
target against both getters and the actual selected export. It does not acquire,
copy or retain unselected formats. Rust snapshots use `ShaderCode::{Wgsl,SpirV,GlslEs300}`.

WGSL output is the exact original validated source. SPIR-V output is guarded Vulkan
code. GL still emits and reimports guarded SPIR-V as temporary compiler scratch to
lower array/buffer checks before emitting ES300; those words are never exported or
retained by a GL shader. Only GL items have generated block/combined-sampler names;
the corresponding getters return `NC_E_NOINTERFACE` for WGSL/SPIR-V, while portable
slot/layout/IO metadata remains available for every target.

Failure of the selected emitter rejects the whole result. Unselected emitters are
not invoked and cannot veto success. Outer COM transport/result-construction
failures remain outer errors. Invalid source, output bounds and fallible reservation
retain distinct statuses. Diagnostics are per-report, capped at 4096 UTF-8 bytes;
infallible SDK/Naga allocation aborts are not promised to become diagnostics.

This is a breaking pre-release shader request/report ABI change. Rebuild SDK
consumers and both providers together; plugin qualified names and registration
order remain unchanged. No compatibility path retains the three-output contract.

Code getters return immutable `INcString`/`INcBytes` objects. Inputs, outputs,
uniforms and sampled pairs return explicitly named typed collections, each with
`len` and `get`. Out-of-range `get` returns `NC_E_OUT_OF_RANGE`. Items expose plain
metadata and, for GL only, independently owned native names. Every export survives report/root
release; an item survives collection release and retains no unrelated code.
`info()` is an optional summary of those same objects, never a prerequisite:

```rust,ignore
// request.target selects exactly one format; this example selects WGSL.
let report = compiler.compile(request, NcCopyFromBufferOffset::new(0, source), NcCopyFromBufferOffset::new(0, entry))?;
let uniforms = report.uniforms()?;
for index in 0..uniforms.len()? {
    let uniform = uniforms.get(index)?;
    let layout = uniform.info()?;
    // Native GL names are queried only for a GlslEs300 request.
    // Both layout and name are usable without consulting report.info().
}
let validated_wgsl = report.wgsl()?;
drop(report); // validated_wgsl remains owned and readable.
```

SPIR-V exports encode little-endian u32 words. Readers check word divisibility,
minimum five-word header and magic, allocate aligned `Vec<u32>` fallibly, and
copy/decode bounded stack chunks. They do not allocate an intermediate full byte
vector or reinterpret unaligned byte storage. SDK string/byte `copy_to` promises
complete initialization on successful status; it returns no written count.
This is distinct from counted `IGpuDataSource` reads. Readers do not promise to
detect ABI-invalid success after a partial write or every semantic truncation.
The selected provider remains responsible for validated artifact semantics.

## Admission and transfer lifetime

Source is at most one MiB; entry is at most 128 UTF-8 bytes. Retained code payload
is `entry bytes + selected code bytes`, at most four MiB and the caller cap.
SPIR-V code bytes are exactly four times its word count. Inputs and temporary
intermediate representations are not retained code and do not inflate this charge. The bounded GLSL writer checks growth before allocating. This is
not a total compiler scratch/RSS, shader-time or native-device fault guarantee.

Reflection is excluded from that charge but bounded: at most 16 inputs, 16 outputs,
four uniforms and four sampled pairs per stage. GL-only native names are bounded by the
admitted GLSL length, with checked aggregate length bounded by eight such names.
No new small name limit excludes existing long generated identifiers.

The device operation gate encloses the complete transaction. Input bytes are
charged before counted-source callbacks. A provider reservation C is admitted
from actual remaining bytes before compile and passed as the compiler cap. Input
allocations are released before their receipt after compile returns. The importer
checks exported lengths/stage/entry, typed qualifiers, unique locations/slots,
collection bounds and summary agreement, then shrinks C to actual code payload P.

Receiving P is charged before code copying. If that admission fails, all foreign
owners release before provider P is refunded; no resource is published. On success,
the importer copies plain immutable data and rechecks lengths/metadata. It drops
all report/export owners outside registry borrows while provider P and the busy
gate remain live, refunds provider P, then checks domain openness after the last
foreign Release. Only then does it publish one complete shader retaining receiving
P. Pipeline versions retain that immutable shader snapshot; native executables and
completion retain opaque shared definition/shader charges,
with no foreign compiler calls during drawing or destruction.

## Renderer handoff and diagnostics

Vulkan consumes returned aligned guarded SPIR-V and entry. WGPU consumes returned
validated WGSL and entry through `ShaderSource::Wgsl`, preserving checked native
translation; it never falls back to request source on an export failure. Glow and
WebGL consume returned ES300 and reflected native names.

Exact texel reads retain the guarded SPIR-V coordinate/level branches in native
GLSL. The GL IR specializes compiler-generated mip-count checks to one, matching
every admitted texture allocation. Its writer does not add a redundant image
guard requiring unavailable ES300 texture-level queries. Explicit source count
queries are rejected before this specialization; Vulkan retains complete guarded
SPIR-V and WGPU uses its checked WGSL translation. Out-of-bounds results remain
backend-dependent as documented in the GPU contract; clients must not rely on
a particular value.

ES300 has standard `main`. For canonical vertex output `(x,y,z,w)` its completion
produces `(x,-y,2*z-w,w)`. Coordinate writer/importer options are explicit, applying
that conversion once. Fragment position uses raw GL `gl_FragCoord`. A shared small
renderer token adapter renames standard vertex main and restores Y after it for
window presentation; offscreen retains compiled Y. The renderer owns fragment
height normalization, winding, viewport/scissor and target policy. Comments,
identifier substrings and whitespace do not depend on Naga's epilogue spelling.

Shader creation returns either the exclusive shader or `RenderError { status,
message }`. The renderer reads its compiler report before publishing its own
one-shot creation result. Text export failures preserve the original compilation
status and supply a local explanation. Each error remains valid across later
successes, later failures, and destruction of the compiler or resource group.
No shader operation reads, clears or replaces a device-wide diagnostic.

The min-size profile preserves Naga/naga-types formatting needed for executable
float literals and distinct symbols while stripping debug symbols. WGPU may
legitimately retain its own internal Naga dependency. No measured binary-size,
compile-time or frame-time improvement is implied by this separation.
