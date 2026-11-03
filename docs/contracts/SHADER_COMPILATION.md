# Portable shader compilation, version 1

`neocogi.shader.compiler` is a synchronous CPU-only COM provider. A renderer
requires and acquires `ShaderCompiler` during activation, before SDL, DOM or GPU
construction. Native dynamic packaging stages `nc_shader_compiler` before the
renderer; monolith and WebGL assemblies register the same provider statically.
Missing dependencies or incompatible roots fail activation. The renderer has no
linked compiler fallback, widget-facing compiler getter or per-draw lookup.

The namespace lives in `tools/platform-build/schemas/neocogi/shader`. Shader-core supplies
the generated ABI, plain reflection and bounded readers without Naga. The
provider's private implementation owns Naga. It uses render-core's existing
`GpuShaderStage`, preserving the one-way contract dependency.

## Outcomes and owned exports

`compile(request, source, entry)` accepts canonical UTF-8 WGSL, a selected entry
and a combined output-byte cap. There is one supported read-only graphics
contract, documented in [GPU rendering](GPU_RENDERING.md). The compiler has no
viewport, surface, Window/Texture, target height or arbitrary profile selector.

A report is either complete success or rejection with an original failing status
and diagnostic. Success requires parsing, whole-module admission/validation,
selected-stage/entry agreement, guarded SPIR-V emission and ES300 emission within
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

The returned WGSL is the exact source validated by Naga. A native emission failure
rejects the whole result; it cannot return WGSL-only success. Outer COM transport
or result-construction errors remain outer failures. Invalid source, output
bounds and fallible allocation preserve distinct statuses. Diagnostic text is
per-report, capped at 4096 UTF-8 bytes at a character boundary; allocator aborts
from infallible SDK construction cannot be converted into a diagnostic.

Code getters return immutable `INcString`/`INcBytes` objects. Inputs, outputs,
uniforms and sampled pairs return explicitly named typed collections, each with
`len` and `get`. Out-of-range `get` returns `NC_E_OUT_OF_RANGE`. Items expose plain
metadata and independently owned native names. Every export survives report/root
release; an item survives collection release and retains no unrelated code.
`info()` is an optional summary of those same objects, never a prerequisite:

```rust,ignore
let report = compiler.compile(request, NcCopyFromBufferOffset::new(0, source), NcCopyFromBufferOffset::new(0, entry))?;
let status = NcStatus::from_code(report.status()?);
if status.is_err() {
    return Err(status);
}
let uniforms = report.uniforms()?;
for index in 0..uniforms.len()? {
    let uniform = uniforms.get(index)?;
    let layout = uniform.info()?;
    let name = uniform.glsl_block_name()?;
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

Source is at most one MiB; entry is at most 128 UTF-8 bytes. Combined code payload
is `WGSL bytes + entry bytes + 4*SPIR-V words + GLSL bytes`, at most four MiB and
the caller cap. The bounded GLSL writer checks growth before allocating. This is
not a total compiler scratch/RSS, shader-time or native-device fault guarantee.

Reflection is excluded from that charge but bounded: at most 16 inputs, 16 outputs,
four uniforms and four sampled pairs per stage. Native names are bounded by the
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

Logical and native renderer halves share one bounded device diagnostic. An
admitted shader operation clears the old text and records its current failure.
The demo host captures first UI-bind failure detail while the fresh device/window
is alive, preserving setup phase, original status and independently owned text.
Later frame handling retains its status-specific guard against stale messages.

The min-size profile preserves Naga/naga-types formatting needed for executable
float literals and distinct symbols while stripping debug symbols. WGPU may
legitimately retain its own internal Naga dependency. No measured binary-size,
compile-time or frame-time improvement is implied by this separation.
