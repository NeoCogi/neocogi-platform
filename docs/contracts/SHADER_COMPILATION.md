# Portable shader compilation, version 1

`neocogi-platform::render::compiler` implements the shader service built into the
existing platform COM plugin. Each backend is delivered in its one existing
platform library, `libnc_platform.so` on Linux. The platform crate owns the
parser, typed IR, validation, direct backend writers, and immutable report objects.
The existing platform-dylib shim exports only the platform descriptor. There is
no separate shader descriptor or registration, and
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
UTF-8 S-expressions or Python-like shader syntax, and the separate entry argument
selects an exact named entry. S-expressions are always available with the compiler.
Python-like input requires the optional `shader-python` Cargo feature, disabled
by default and never enabled by a backend or `test-support`. The shim forwards
that same feature to its compiler. It adds no registration, dependency or Python
runtime; both enabled frontends lower to one typed graphics IR.
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
entries are unsupported and reject. Identifiers start with a letter or underscore and contain
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

Statements are `(let NAME VALUE)`, `(let NAME TYPE VALUE)`, `(var NAME VALUE)`,
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

## Python-like frontend

Enable `shader-python` explicitly on `neocogi-platform` for linked applications or
CPU tools, or on `neocogi-platform-dylib` for a loaded platform. Without it the
parser module is excluded and Python-like input rejects with its original
line/column and an instruction to enable the feature. Backend selection still
enables the S-expression compiler automatically.

```sh
cargo build --profile min-size -p neocogi-platform-dylib --no-default-features \
  --features vulkan,shader-python
```

Frontend selection is deterministic: after ASCII whitespace and semicolon comments,
a leading `(` selects S-expressions; other input selects Python-like syntax. A
source contains one frontend. The Python-like parser produces the shared syntax
nodes directly, retaining byte positions into the original UTF-8 source. It does
not generate S-expression text or use Python, a VM, imports or a third-party parser.
Both parsers and shared semantic checks report the original one-based line/column,
excerpt and caret through the same Rust and COM result APIs.

```python
class Camera:
    transform: mat4

class Surface:
    clip: builtin[vec4, position]
    uv: location[vec2, 0]

camera: uniform[Camera, vertex, 0]
image: texture[fragment, 0]

@vertex
def vs(position: location[vec3, 0], uv: location[vec2, 1]) -> Surface:
    return Surface(camera.transform @ vec4(position, 1.0), uv)

@fragment
def fs(surface: Surface) -> location[vec4, 0]:
    return sample(image, surface.uv)
```

`class` declares a finite value record containing typed fields. `def` declares a
helper; `@vertex` and `@fragment` select entry stages. Parameters require types;
omitted return annotations or `None` mean void. `builtin[T, NAME]` and
`location[T, SLOT, MODE?, SAMPLING?]` apply the same IO rules as the S-expression
attributes. Uniforms use `NAME: uniform[TYPE, STAGE, SLOT]`, and paired textures
use `NAME: texture[STAGE, SLOT]`. Graphics annotations are absent from helper
signatures. Record layout, resource bounds and stage restrictions are unchanged.

Value types use the same portable names and `array[T, COUNT]`. Fixed array
constructors are `array[T, COUNT](...)`; `[a, b, c]` infers a fixed array from
identically typed elements. Arrays are values, rather than dynamic Python lists.
`True` and `False` are boolean literals. Decimal/exponent literals retain shader
f32/i32 semantics; `u32(1)` and `u32(4294967295)` provide unsigned values. Lowercase
`true`/`false`, structural Python keywords and `array` are reserved spellings.
Strings, imports, arbitrary Python objects, methods, comprehensions, tuples,
chained comparisons, keyword arguments, `for` and conditional expressions are
unsupported. Combine comparisons with `and`; use an `if` statement for selection
requiring conditional execution. `select(a, b, condition)` stays eager.

Expressions use calls, `value.member`, `value[index]`, ordinary infix arithmetic,
bit operations, shifts, comparisons and `and`/`or`/`not`. Precedence follows Python,
including right-associative `**` above unary signs. `**` uses the floating-point
`pow` intrinsic. `@` requires a matrix operand and performs the shared matrix
product; existing shader `*` overloads remain available. Intrinsics retain their
names, with `sample_level` and `inverse_sqrt` accepted as snake-case spellings.
Shader arithmetic does not acquire Python numeric promotion, truthiness or
negative-index wrapping: invalid indexed reads return zero and writes are skipped.

Statements include assignment, typed locals, augmented assignment,
`if`/`elif`/`else`, `while`, `break`, `continue`, `pass` and `return`. Suites require
newlines and consistent space indentation; tabs and semicolon statements reject.
Parentheses/brackets permit multiline expressions. `#` comments may contain
Unicode; CRLF and an omitted final newline are supported.

A first local assignment declares a value. A later assignment or member/index
write makes its declaration mutable; otherwise it retains immutable SSA/view
optimizations. `name: TYPE = value` provides an explicit type, and `name: TYPE`
starts at typed zero. Blocks have lexical scope: names first declared in a branch
or loop do not escape it. Explicit typed declarations may shadow outer locals.
Writing a parameter creates a private mutable value copy at function entry,
including when an aggregate member is modified; it cannot mutate caller storage.
Augmented assignment captures every target index and its previous value once,
before evaluating its right operand. Generated local names avoid every identifier
written in the module, including later declarations.

Both frontends enforce 96 syntax-tree levels and 100,000 syntax nodes, including
generated forms. The Python-like lexer additionally caps its token stream at
200,000 entries. Existing source, local, declaration and artifact limits apply.
Parsing and validation finish before backend selection or helper optimization.
Equivalent Triangle programs are tested for byte-identical WGSL, GLSL and SPIR-V
artifacts, as well as identical reflection. Native and WebGL tests render both
frontends' snapshot, indexing, parameter-copy and control-flow workloads.

On 2026-11-12, rustc 1.102.0-nightly (`3ef0c22cd`) produced a 459,424-byte
complete Vulkan library with both frontends, rebuilt std and immediate-abort
panics. Its only defined dynamic export was `nc_get_plugin_descriptor_v1`.
The loaded COM compiler accepted both source forms after root/factory release,
and the dynamic min-size Python-like Triangle rendered 60 frames. Exact-location
and retained-diagnostic tests, five output feature graphs, native GLSL/SPIR-V/WGSL
validation, all three native backend suites, seven Python-like pixel workloads,
Chromium WebGL/DOM/audio, Clippy and rustfmt passed. Extracted archives and their
independent nightly/stable COM consumers passed. Logs and the qualified library
are retained locally in `/tmp/neocogi-python-frontend/`, outside Cargo targets.

The subsequent optional-frontend qualification on the same nightly produced
418,104 bytes with `--features vulkan` and 459,424 bytes with
`--features vulkan,shader-python`. Both are complete normal platform libraries
below 524,288 bytes. The loaded COM probe checks Python rejection in the default
library and acceptance in the opt-in library, independently of the test host's
features. Set `NEOCOGI_TEST_SHADER_PYTHON=1` when qualifying an opt-in library;
the default expectation is disabled. Source-located disabled-feature errors,
ten output/frontend configurations, default and opt-in native examples, browser
execution, dependency graphs and package checks are covered. Evidence is kept
locally in `/tmp/neocogi-optional-python/`.

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

### Projection and helper lowering

All writers share the decomposition of local/uniform member and index paths.
An addressable read checks the complete path and loads its selected leaf; it does
not copy an array or structure merely to select one element. Proven integer
indices omit runtime bounds branches. Dynamic indices evaluate once, including
when an earlier projection is invalid. Constant invalid reads return typed zero
and invalid writes are skipped. No floating-point algebra is reassociated or
folded by this optimization.

Immutable uniform views retain their declaration-time index values. Immutable
locals can share stable storage, while mutable locals and snapshots of mutable
values retain separate owned storage. SPIR-V keeps ordinary immutable scalar and
vector values in SSA; real owned aggregates receive one function allocation at
their declaration. Only an unaddressable computed aggregate requiring dynamic
selection needs temporary storage. Conditional reads, short-circuit expressions
and guarded image loads join their results with SSA Phi instructions.

After whole-module validation, a shared IR pass expands small value-returning
helpers with array/matrix parameters or structures containing them. During an
expanded call, aggregate arguments may borrow caller storage: helpers cannot
write caller locals or resources, and the caller is suspended. Ordinary lets and
returned snapshots keep their value semantics. Scalar arguments still evaluate
eagerly, even when unused; short-circuit operands stay conditional. Expanded
returns terminate remaining callee work, and loop conditions reevaluate after
continue. Fresh typed locals preserve lexical scopes without name collisions.

Expansion stops after eight nested levels, for helper costs above 512, or after
65,536 units of additional IR work across the module. Remaining helpers use
ordinary calls with value semantics. These are code-growth limits, not promises
that every accepted algorithm has bounded GPU cost. Resource reflection and
stage/derivative admission retain the original validated module's requirements.
No output uses a runtime optimizer process or a shader-specific source rewrite.

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

## Performance qualification

`cargo tools profile-shaders` qualifies the exact production inventory exposed by
`examples/native/src/shaders.rs`: Triangle, the skinned PBR mesh and HDR skybox,
including shared lighting helpers. The applications consume these same sources;
qualification cannot silently substitute a simpler shader. The corpus also
contains twelve aggregate workloads covering nested fields, captured indices,
private snapshots, computed indexing and helpers with branches, nested loops,
early returns, short-circuiting and writable indices.

CI checks GLSL/SPIR-V validity, bounded private storage and palette-capacity
independence. Actual native Vulkan/GLES3/WGPU and Chromium WebGL2 executions check
the corpus pixels through the production COM compiler, pipelines and recording.
WGPU additionally validates generated WGSL for eager derivative arguments and
helper expansion falling back at its depth bound. Existing operator, diagnostic,
reflection, quota and ownership suites remain required.

For hardware qualification, fetch the assets and run, for each backend:

```sh
cargo tools profile-shaders --backend vulkan --output /tmp/shaders-current \
  --reference /tmp/shaders-previous/candidate --perf --intel-stats
```

Use `glow` and `wgpu` for the other backends. The command builds the normal single
min-size library, exports all three shader encodings, and runs Triangle, Damaged
Helmet and BrainStem. It alternates reference/candidate samples after warmup,
uses a fixed pose, requires all requested frames to render, and retains every raw
sample. Timings include process startup, presentation and teardown. Frame pacing
can limit throughput, so separate driver diagnostics check every reported SIMD
variant for zero spills/fills and at most 4096 instructions. A supplied reference
also enforces a configurable median elapsed-time ratio, default 1.2. See
[the tools guide](../../scripts/README.md) for requirements and report layout.

These workload gates do not establish a performance bound on arbitrary source
programs. Intel diagnostics require an appropriate Mesa driver; other devices
need equivalent native profiler evidence. Reports and copied deployments belong
outside Cargo targets so they survive the required clean builds.

On 2026-11-12, Intel Arc Meteor Lake with Mesa 26.0.8, Linux 7.0.0-42 and
rustc 1.102.0-nightly (`3ef0c22cd`) passed these gates against unoptimized source
`10eba17`. Each value is the median of three alternating 300-frame process runs,
after three-frame warmups, with cached HDR data and animation time 1. Builds and
uncached driver diagnostics ran separately from timing.

| Backend | Triangle FPS, before → after | Helmet FPS, before → after | BrainStem FPS, before → after | Complete library bytes |
| --- | --- | --- | --- | --- |
| Vulkan / SPIR-V | 869.0 → 921.2 | 63.4 → 76.9 | 14.8 → 83.3 | 415,432 |
| GLES3 / GLSL ES3 | 56.9 → 56.0 | 47.5 → 47.8 | 55.0 → 55.4 | 471,872 |
| WGPU / WGSL | 777.4 → 741.2 | 74.2 → 74.2 | 67.1 → 67.7 | 2,351,376 |

All three applications passed the 1.2 elapsed-time ratio gate on every backend.
Every reported native shader variant had zero spills/fills and fewer than 4096
instructions. The Vulkan mesh vertex shader fell from 20,215 instructions,
4181 spills and 298 fills to 319 instructions without spills. Its complete
library remains below 524,288 bytes, as does GLES3; WGPU includes its larger
native translation stack and has a separately reported size. These measurements
qualify this device/driver combination, rather than predicting other hardware.

Raw reports, all emitted corpus artifacts, deployments, Intel dumps and per-run
perf counters are retained locally under `/tmp/neocogi-shader-optimization/`
in `qualified-vulkan/`, `qualified-glow/` and `qualified-wgpu/`. The reusable
command regenerates this evidence; these temporary paths are not SDK inputs.
