# Portable shader compilation, version 1

`neocogi-platform::render::compiler` implements the shader service built into the
existing platform COM plugin. Each backend is delivered in its one existing
platform library, `libnc_platform.so` on Linux. The platform crate owns the
parser, typed IR, validation, direct backend writers, and immutable report objects.
The existing platform-dylib shim exports only the platform descriptor. There is
no shader feature to select, no separate shader descriptor or registration, and
no additional shared library to deploy. WGPU retains its internal translation dependency.

The contracts crate owns `neocogi.renderer` compiler declarations in
`schemas/neocogi/renderer/compiler.ncidl`. Static and loaded clients use exactly
these generated interfaces and `render::shader` snapshots/readers.

## COM acquisition and outcomes

Register or load only the selected platform descriptor. Its root implements both
`Platform` and `ShaderCompilerFactory`; QueryInterface obtains the typed shader
factory without initializing SDL, a window, or a graphics device. `create(target)`
returns an independent `ShaderCompiler` fixed to an enabled output format.

Each renderer device constructs and retains its built-in compiler with its native
output format. Shader creation and drawing perform no plugin lookup. Linked
CPU-only tools may instantiate the same `render::compiler::Factory` COM object,
or use the pure Rust `Compiler` facade. There is no global runtime lookup, erased
payload, downcast registry, or mutable last-error slot.

`ShaderCompileRequest` carries a graphics stage and output quota. The source is
UTF-8 S-expressions, and the separate entry argument selects an exact named entry.
WGSL source input has been removed. Requests cannot change a compiler's target.
`compile_result` returns either complete success or a rejection report containing
its original status and diagnostic. The Rust `compile` facade reads that same
report and returns an owned `RenderError` on rejection.

Each successful report owns one `INcBytes` code export, its entry name, typed
input/output/uniform/texture collections, and an empty diagnostic. Code, collection
items, and GL native names independently outlive the report and factory. An item
retains no unrelated code. `info()` summarizes the same exports and is optional;
`target()` independently authenticates the actual encoding. Rejection exposes no
partial artifacts. Invalid source/configuration, unavailable targets, exhausted
quotas, and fallible output reservations retain distinct statuses. Outer COM
transport or report-allocation errors remain outer failures.

Syntax and validation diagnostics include one-based line/column, a bounded source
excerpt, and a caret. Raw COM diagnostics are capped at 4096 UTF-8 bytes without
splitting a character. Allocation failures in infallible parser/IR/SDK allocations
may abort; the quota is an exported-artifact bound, not a total scratch-memory or
shader-execution-time guarantee.

## S-expression frontend

The parser accepts exactly one `(module ...)`, ASCII atoms, parentheses, whitespace,
and semicolon comments. Unicode is allowed in comments. Strings, quoting, macros,
implicit native-source escape hatches, storage/workgroup resources, and compute
entries are unsupported and reject. Identifiers start with a letter and contain
at most 128 ASCII letters, digits, or underscores. Source identifiers are resolved
before emission and backend symbols are generated independently.

```lisp
(module
  (struct Camera (transform mat4))
  (struct Surface
    (clip vec4 (builtin position))
    (uv vec2 (location 0)))
  (uniform camera Camera vertex 0)
  (texture image fragment 0)

  (vertex vs ((position vec3 (location 0)) (uv vec2 (location 1)))
    (returns Surface)
    (return (Surface (* (. camera transform) (vec4 position 1.0)) uv)))

  (fragment fs ((surface Surface)) (returns vec4 (location 0))
    (return (sample image (. surface uv)))))
```

| Form | Meaning |
| --- | --- |
| `(struct NAME (FIELD TYPE [ATTRS...])...)` | Ordered value structure; referenced types are declared first. |
| `(uniform NAME TYPE STAGE SLOT)` | Named structure with portable WGSL/std140 layout, at most 16 KiB. |
| `(texture NAME STAGE SLOT)` | Read-only float 2D texture and its implicit ordinary paired sampler. |
| `(fn NAME (PARAMS...) (returns TYPE) BODY...)` | Value-returning helper, with forward function calls and no recursion. |
| `(vertex NAME (PARAMS...) (returns TYPE [ATTRS...]) BODY...)` | Vertex entry requiring exactly one position builtin. |
| `(fragment NAME (PARAMS...) (returns TYPE [ATTRS...]) BODY...)` | Fragment entry requiring exactly one vec4 color at location zero. |

STAGE is `vertex` or `fragment`. Parameters use `(NAME TYPE [ATTRS...])`. Interface attributes are `(location N)`,
`(builtin position|vertex_index|instance_index|front_facing)`, and
`(interpolate perspective|flat [center|centroid])`. Builtin stage/direction/types
are checked, including inactive entries. Integer varyings are flat. Linear
interpolation and sample-frequency qualifiers reject because ES 3.00 cannot
represent the complete requested portable contract. User locations admit f32/u32
scalars or vectors only, with no duplicates and locations below sixteen.
Entry structure members have direct builtin/location bindings; nested IO
structures and IO attributes on helper signatures reject.

Types are `f32`, `i32`, `u32`, `bool`, `vec2`..`vec4`, `ivec2`..`ivec4`,
`uvec2`..`uvec4`, `bvec2`..`bvec4`, `mat2`..`mat4`, rectangular `matCxR`,
`(array TYPE COUNT)`, and named structures. Decimal/exponent literals are finite
f32 values; integral literals are i32 unless suffixed `u`. Explicit constructors
perform numeric conversions. Booleans do not implicitly convert to numbers.
Vector constructors pack components or splat one scalar; matrix constructors
supply columns; arrays/structures supply every element/member. No value exceeds
64 KiB. Arrays contain 1..4096 elements; direct arrays of arrays reject for ES300
portability (structure elements may contain arrays). Uniform matrix rows and array/member
strides must agree with std140; incompatible layouts reject before emission.
Uniform reflection retains exact WGSL bytes, with native trailing padding private
to the backend.

Statements are `(let NAME VALUE)`, `(var NAME VALUE)`,
`(var NAME TYPE [VALUE])`, `(set LVALUE VALUE)`,
`(if CONDITION (then BODY...) [(else BODY...)])`, `(while CONDITION BODY...)`,
`(break)`, `(continue)`, and `(return [VALUE])`. Typed variables without an
initializer start at zero. Assignment requires a mutable local or its member,
single component, or indexed element. Resource writes and mutable globals are
absent. Conditions are bool; every path of a value-returning function must return.

Expressions include `(. VALUE MEMBER_OR_SWIZZLE)`, `(index VALUE INDEX)`, typed
constructors, helper calls, unary `- ! ~`, arithmetic `+ - * / %`, comparisons,
integer bit operations/shifts, and scalar `&& ||`. Numeric scalar/vector arithmetic
broadcasts scalars explicitly during lowering. Matrix multiplication uses column
semantics. Intrinsics include min/max/clamp, rounding, trigonometry, exponential
functions, vector products, normalize/reflect, mix/select, all/any, and graphics
derivatives. Intrinsic overloads and arities are validated independently of output.
Selection is eager in every writer. Derivatives and implicit `sample` operations
are restricted to unconditional fragment control flow, including transitive
helpers and paths following a possibly divergent return. Use explicit mip-zero
sampling in conditional helpers. This conservative rule is deliberate, not a claim
of a general uniformity analysis.

`(sample IMAGE UV)` and `(sample-level IMAGE UV 0.0)` use the declaration's paired
sampler. `(load IMAGE INTEGER_VEC2)` and `(dimensions IMAGE)` operate at mip zero;
other levels and count queries reject. Every used image must also be sampled by
that entry's reachable functions. Binding groups follow the renderer contract:
vertex texture/sampler pairs use group 0, vertex uniforms 1, fragment pairs 2,
fragment uniforms 3. Pair bindings are `2*slot` and `2*slot+1`; slots are below four.
Stage-crossing resource use rejects.

Admission checks the complete module, including unused structures/resources,
helpers, and other entries, before output selection. Parsing is bounded to 96
levels and 100,000 nodes; a module has at most 256 declarations, a structure 64
fields, a function sixteen parameters and 1024 locals. These limits prevent
unbounded syntax expansion. Only the requested entry and reachable helpers and
resources are emitted or reflected.

## Backend artifacts and build features

| Renderer | Fixed output | Features |
| --- | --- | --- |
| WGPU | Generated UTF-8 WGSL | `shader-compiler,shader-wgsl` |
| GLES3/WebGL2 | Generated UTF-8 GLSL ES 3.00 | `shader-compiler,shader-glsl` |
| Vulkan | SPIR-V 1.0 words, little-endian byte export | `shader-compiler,shader-spirv` |

Backends enable their output automatically. CPU-only tools select these features
without SDL, GL, Vulkan, or WGPU. Outputs are additive, but each compiler instance
returns only its configured target. The SPIR-V writer uses the small `spirv`
opcode/enumeration crate. No Naga frontend, Naga writer, external compiler process,
or temporary cross-language round trip is used by the plugin. WGPU's independent
native translation may still depend on Naga.

All writers consume the same validated typed IR. Generated shaders return
zero for out-of-range array/vector/matrix/image reads; local writes outside the
index range are skipped. Guards execute before an indexed pointer or texel fetch
is formed. WGSL uses the same explicit checks, in addition to native validation.
GL exports reflected block and combined-sampler names; other outputs return
`NC_E_NOINTERFACE` for these getters.

GLSL has standard `main`. Canonical vertex position `(x,y,z,w)` becomes
`(x,-y,2*z-w,w)` once. Vulkan negates clip Y once. The renderer continues to own
presentation restoration, fragment-height normalization, winding, viewport/scissor,
and target policy. Derivatives retain the existing canonical signed axis.

The complete Vulkan platform bundle exports exactly `nc_get_plugin_descriptor_v1`
and does not embed the COM host. The executable supplies its one process-lived
host. Build and qualify this single library with the repository's
`channel = "nightly"`, rebuilt std, `min-size`, and immediate-abort panics:

```sh
cargo build --locked -p neocogi-platform-dylib --no-default-features \
  --features vulkan --profile min-size
NEOCOGI_PLATFORM_DYLIB="$PWD/target/min-size/libnc_platform.so" \
NEOCOGI_TEST_BACKEND=vulkan NEOCOGI_PLATFORM_SIZE_LIMIT=524288 \
  cargo test --locked -p neocogi-platform --features test-support,shader-glsl,shader-spirv \
  --test compiler_plugin -- --ignored --exact dynamic_platform_plugin_exposes_its_builtin_compiler
NEOCOGI_SHADER_VALIDATORS=1 cargo test --locked -p neocogi-platform \
  --features test-support,shader-glsl,shader-spirv --test sexp_compiler
```

The dynamic probe loads the actual platform library, checks its requested size
limit, queries the built-in COM factory, compiles its native output, and checks
code/diagnostic ownership after root release. Use the target's `.dll`/`.dylib`
filename on Windows/macOS. Numeric emission uses Display/explicit bits rather
than Debug so min-size formatting cannot erase literals or symbol identity.

The CPU `shader_compile` example accepts
`wgsl|glsl|spirv vertex|fragment ENTRY INPUT OUTPUT`. Native validation tests run
`glslangValidator` and `spirv-val` when available; setting
`NEOCOGI_SHADER_VALIDATORS=1` makes missing validators fail qualification.

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

Vulkan consumes aligned guarded SPIR-V and the selected entry; WGPU consumes
emitted WGSL and the entry through its checked native module/pipeline path;
Glow/WebGL consume ES300 and reflected native names. Native linking and resource
allocation remain fallible. An export failure never falls back to the input.

Shader creation returns the exclusive shader or an owned `RenderError`. The
renderer reads the compilation report before publishing its creation result.
Failure text remains valid after later calls and compiler/resource-group release.
No shader operation reads, clears, or replaces a device-wide diagnostic.

Alpha NCIDL versions remain 1. Rebuild generated consumers and providers together.
The factory interface and source-language replacement intentionally supersede the
previous WGSL/Naga contract without a compatibility parser.

## Qualification baseline

On 2026-11-12, Linux x86_64 with rustc 1.102.0-nightly
(`3ef0c22cd`, 2026-11-11) produced a **399,720-byte complete Vulkan platform
library**, below the 524,288-byte limit, using the min-size command above with
rebuilt std and immediate-abort panics. Its sole defined dynamic export was
`nc_get_plugin_descriptor_v1`. The dynamic probe compiled through the queried
factory after releasing both the platform root and factory, and retained code
and diagnostic exports after compiler/report release. The dynamic min-size
BrainStem application rendered three Vulkan frames with this single library.

Fresh qualification also passed the CPU/COM suites, all five compiler-output
feature configurations, GLSL/SPIR-V validators, native WGPU validation of emitted
WGSL, and native tests and Clippy for Glow, Vulkan, and WGPU. Triangle, Damaged
Helmet, and BrainStem ran with each backend through static builds and relocated
dynamic bundles. Real Chromium DOM/WebGL/audio regressions and independent COM
consumers of extracted package archives passed, including nightly and stable
package checks. CI enforces the direct validators, native WGSL validation, and
the complete Vulkan library's COM lifetime and byte limit; other backend and
target sizes require their own qualification.

The same day's normal-library packaging qualification retained the 399,720-byte
Vulkan result as `libnc_platform.so`, with the same sole COM discovery export.
All three min-size applications identified Vulkan automatically and rendered
three frames each. Relocated dynamic examples and descriptor/missing-library
regressions passed on Glow, Vulkan, and WGPU. Independently generated COM callers
built and loaded the shim as a nonmember registry-style dependency from extracted
archives on both nightly and stable, retaining code and diagnostics after provider
release. These checks use one shared Cargo target directory; library-example
targets are no longer part of packaging or qualification.
