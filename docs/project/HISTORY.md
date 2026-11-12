# Unreleased

## Portable shader projection performance

WGSL, GLSL ES3 and SPIR-V now share bounded aggregate-helper expansion and direct
member/index path lowering. Uniform selection reads a leaf without copying the
palette; immutable SPIR-V values use SSA, while real mutable snapshots retain
their own storage. Bounds, captured indices, eager arguments and conditional
evaluation retain their semantics. Structural budgets and real native/WebGL
pixels exercise the same aggregate corpus, and the applications share one exact
production shader inventory with qualification tests.

`cargo tools profile-shaders` validates that inventory, builds normal min-size
libraries, compares all three applications with a preserved reference and retains
raw samples, perf counters and optional Intel instruction/spill diagnostics.
On the qualified Intel Arc/Mesa system, Vulkan BrainStem improves from 14.8 to
83.3 FPS with zero shader spills; all backend comparisons pass. The complete
Vulkan library is 415,432 bytes. See the shader contract for conditions and all
backend measurements.

## Normal platform library packaging

The shim now exposes only its normal `nc_platform` cdylib. Backend features select
the implementation inside `libnc_platform.so` (target-specific suffix/prefix on
macOS/Windows). The backend-named library-example targets are removed; ordinary
`cargo build -p neocogi-platform-dylib` builds the deployable library. Dynamic
examples stage that one output and identify its backend through the descriptor.
Optional `--backend` verifies the compiled backend rather than choosing another
file. Package qualification builds and loads the shim as a consumer's registry-style
dependency outside the repository workspace on nightly and stable.

## S-expression shader COM plugin

The renderer's Naga frontend/writers are replaced by the platform crate's bounded
S-expression parser, typed IR, whole-module validation, and direct WGSL, ES300,
and SPIR-V writers. Shader compilation is built into each platform backend's
existing COM plugin and library. `ShaderCompilerFactory` is queried from its
platform root; no separate feature, plugin registration, or library is introduced.
The complete min-size Vulkan platform bundle targets less than 512 KiB with
rebuilt std and immediate-abort panics. All shader fixtures and examples use the
new frontend. The workspace follows `channel = "nightly"`.

# 0.2.0

This release promotes `0.2.0-alpha.2` to `0.2.0` with coordinated versions for
`neocogi-platform-contracts`, `neocogi-platform` and `neocogi-platform-dylib`.
There are no runtime, API or schema changes from alpha.2. Upgrade the package
set together; internal registry dependencies remain pinned to the exact release.
All NCIDL declarations remain version 1.

All three crates are published as `0.2.0` after all eight
[CI jobs](https://github.com/NeoCogi/neocogi-platform/actions/runs/42822747992)
passed for the tagged source. Tag `v0.2.0` identifies the published source
commit `55fbd04`. See the [release guide](RELEASING.md) for qualification
and publication order.

# 0.2.0-alpha.2

This tag advances the three coordinated package versions and includes the
post-alpha.1 CI repairs: the workflow's YAML process filter, macOS cleanup of
zombie-only process groups, GL completion ownership assertions before teardown
polling, and bounded observation of asynchronous HTTP listener shutdown.
All three crates are published as `0.2.0-alpha.2`. NCIDL declarations remain version 1;
the published renderer and audio runtime contracts are unchanged.
Tag `v0.2.0-alpha.2` identifies the published source commit `13f8fbd`.

# 0.2.0-alpha.1

This alpha replaces the former package set with three coordinated releases:
`neocogi-platform-contracts`, `neocogi-platform` and `neocogi-platform-dylib`.
The minor-version advance marks incompatible Rust API, generated contract and
namespace changes since `0.1.0-alpha.2`. Upgrade the package set together and
regenerate clients; NCIDL declarations remain version 1 during alpha.

- Canonical schemas, generated bindings, shared values, limits and build support
  have one contracts owner. The implementation SDK and final library shims use
  those exact types. Dynamic clients no longer compile renderer/Naga code.
- Shader compilation belongs to the renderer and returns only its backend's
  selected shader artifact. Dynamic examples select prebuilt platform libraries
  at runtime; native builds use ordinary Cargo commands.
- PCM playback, microphone capture and capability queries support native SDL
  and browser Web Audio. Audio owns the sibling `neocogi.audio` namespace.
  Audio, input and rendering contracts use Rust enum payloads for typed values.
- GL uniform backing extents, mutable texture unpack state, final drawable-size
  admission, owned diagnostics, keypad shortcut aliases and browser process
  deadlines include focused regressions.
- Rust repository tools replace Python scripts. Numerical kernels use
  `rs-math3d` 0.21.0. Current contract docs own worked examples; upgrade steps
  and earlier designs are recorded below.

The [release guide](RELEASING.md) defines package inputs and qualification;
the [documentation index](../README.md) points to current contracts and usage.

# Alpha contract upgrade reference

The current release set is `neocogi-platform-contracts`, `neocogi-platform` and
`neocogi-platform-dylib`. The entries below record earlier layouts and APIs;
current behavior belongs to the linked contracts. All NCIDL declarations retain
version 1 during alpha, so rebuild generated clients and providers together.

- Replace former package names and aliases with the contracts crate for schemas,
  generated bindings, shared types, limits and validation; the SDK for renderer,
  compiler, audio and hosting implementations; and the shim for final platform
  libraries. Dynamic applications use contracts without an SDK dependency.
  Static applications enable their SDK backend and hosting features, initialize
  one COM host and register `platform::plugin_descriptor()`. See
  [component ownership](../architecture/COMPONENTS.md).
- Build scripts use the contracts crate's `build-support` feature and
  `build::{Module, sources, build}`. Select `Module::Renderer` for shader and GPU
  declarations; `Module::Shader` and the separate shader namespace are removed.
  Replace staging macros and artifact dependencies with `build::layout` calls
  and rerun directives for returned inputs and outputs. Canonical schema bytes
  are reconstructed in each consumer's OUT_DIR. See
  [component ownership](../architecture/COMPONENTS.md) and
  [native staging](../contracts/NATIVE_LAYOUT.md).
- Build the shim's normal library before running dynamic applications, using
  the same profile and target. Dynamic examples need only `dynamic`; the library's
  feature selects the backend and optional `--backend` verifies its descriptor. The renderer owns compilation inside that platform library; remove
  external compiler registration and deployment. Current two-command invocations
  are in [native staging](../contracts/NATIVE_LAYOUT.md) and the
  [native examples](../../examples/native/README.md).
- Remove `ShaderCompileRequest.target`: output is fixed at compiler construction.
  Replace the three code getters with `code() -> INcBytes`, and the three format
  lengths with `ShaderInfo.target` and `code_byte_count`. Results retain `target()`
  for backend validation. Rust snapshots use one `ShaderCode` enum;
  `PreparedShader::new` takes the expected target before entry bytes. Replace
  `GpuDevice::new(core, compiler)` with fallible `GpuDevice::new(core)`. Standalone
  CPU compilation uses `shader-compiler` plus selected output features; backends
  enable their own output. See [shader compilation](../contracts/SHADER_COMPILATION.md).
- Replace optional initial buffer data, update methods, separate `IGpuTexture`
  and copied per-draw uniforms with persistent `IGpuBuffer` resources. Choose
  static storage with exact initial bytes, or zero-initialized dynamic/streamed
  storage with `upload`/`read`; bind uniform ranges through
  `bind_uniform_buffer`. The [GPU contract](../contracts/GPU_RENDERING.md)
  contains the typed vertex example, transfer units and storage lifetimes.
- Audio moved from `neocogi.platform` to sibling `neocogi.audio`, authored under
  `schemas/neocogi/audio`. Use `neocogi_platform_contracts::audio` and
  `Module::Audio` for independent projections. Platform discovery imports those
  service types. See [PCM audio](../contracts/AUDIO.md).

For consumers already targeting this package set, the
[repository tools guide](../../scripts/README.md) describes temporary local-source
overlays and configuration inherited by child Cargo processes.

# Bounded owned diagnostic storage

Audio/render diagnostic normalization now bounds retained String capacity as
well as visible UTF-8 length. A shared private scalar-safe prefix helper releases
oversized input storage, including spare capacity behind short/empty messages.
Distinct status policies and per-operation ownership remain unchanged. Contract
and real COM request/report regressions cover storage, UTF-8 and retained exports.
The bound applies after normalization; input conversion may allocate first.

# Final drawable extent admission

Window submission now checks actual physical backing dimensions again after
foreign stream callbacks, alongside final group/target admission. Reentrant
browser CSS/DPR synchronization or native resizing cannot make an old-size frame
clear/draw a differently sized target. Failure is owned InvalidArg; unchanged
queries and the next correctly sized frame remain usable. Native/browser COM
acceptance covers growth/shrink, sentinel pixels, final-callback ordering and recovery.

# Owned browser process deadlines

The Rust browser runner now owns a Unix process group or Windows job, including
descendants that outlive their direct parent. Both output streams spool to owned
temporary files; one deadline covers execution and bounded output collection
without pipe-reader joins. Normal parent exit also triggers descendant cleanup
while preserving the parent's status. Real Rust subprocess regressions cover
timeout, early parent exit, live descendant cleanup and large diagnostics.

# Persistent texture upload state

Dynamic and streamed GL/WebGL texture refreshes now use the same complete unpack
initializer as first realization. Valid application PBO, stride, skip and browser
conversion state cannot reinterpret tightly packed CPU RGBA bytes. Native and
browser acceptance exercise repeated completed uploads, exact pixels and reuse
of the existing one/three native slot names under contaminated application state.

# Private native uniform extents

GL/WebGL now preserve the distinction between exact logical WGSL uniform bytes
and std140's private final block rounding. Persistent uniform buffers initialize
their bounded native tail once, refresh logical bytes in place and bind the
required native extent. Public reflection, descriptors, range checks, CPU reads
and quota semantics remain exact. Shared native/browser acceptance exercises
small scalar blocks, all creation policies and offset-at-capacity ranges through
the actual compiler and generated COM recording path.

# Rust enum payloads in platform and renderer contracts

Keyboard keys carry their character/function payload directly. Presses own their
repeat flag; releases have none. Surface descriptors select a framebuffer view
without a size or an image with its own dimensions. SDL, DOM input, native
examples and surface acceptance use these Rust enums. Invalid payload values
still reject before delivery/publication, while unknown raw tags reject at the
generated boundary. Shader bindings carry byte lengths only for uniform blocks.
Blend state carries equations only when enabled, and min/max equations carry no
scaling factors. All three native adapters and WebGL consume the same typed
descriptors. Alpha declarations retain version 1; clients and providers must
rebuild together.

# Typed platform audio

The platform now exposes PCM playback, microphone capture and capability queries
on SDL and browser Web Audio. Audio-only Rust consumers enable `audio` without
graphics/Naga. Concrete `PcmType` variants carry widths and fixed scales; support,
enumeration and directional queue snapshots use typed enums. The ten canonical
schemas remain in the contracts crate and alpha versions remain 1. Browser
completion is explicit; cancellation/final release stops late microphone tracks.
See the audio contract for ownership, bounds and qualification.

# Rust repository tools

The unpublished `neocogi-platform-tools` member replaces the Python scripts with
`cargo tools` commands for sample downloads, release archive qualification,
browser execution, LLDB source-map generation and local-source overlays. Their
regressions now run under `cargo test -p neocogi-platform-tools`. CI, application
diagnostics and documentation use these Rust commands. HTTPS, gzip/tar, hashing,
manifest/HTML parsing and loopback serving run in Rust; the browser workflow keeps
its explicit Chrome/Chromium and matching wasm-bindgen prerequisites. Native
build/run commands and the three-crate release set remain unchanged. Debuggers
source generated native LLDB commands instead of importing a Python hook.

# Contracts package naming

`neocogi-platform-contracts` replaces `neocogi-platform-schema` in
`crates/platform-contracts`. The name covers its authored schemas, generated Rust
bindings, shared types, limits and validation as well as build support. Rust
imports and generated external paths use `neocogi_platform_contracts`; the SDK
continues to expose the same shared contract types. This is a package rename:
NCIDL namespaces, interface identities and alpha version 1 declarations remain
unchanged. Older entries retain their historical package names.

# Runtime backend selection for dynamic examples

Native dynamic examples now compile with only --features dynamic. The application
loads the sole deployed platform library or accepts --backend glow/vulkan/wgpu
when several are present. Build scripts stage all known prebuilt platform variants;
changing a runtime choice does not rebuild the example. Static assemblies retain
one backend Cargo feature. Schema APIs, version 1 contracts and renderer-owned
shader formats are unchanged. This supersedes dynamic backend features below.

# Dedicated schema and contract package

`neocogi-platform-schema` now owns the nine authored schemas, generated Rust
contracts and backend-neutral Rust/build helpers. The implementation SDK depends
on this crate and exposes its exact render types. Dynamic applications and build
scripts depend only on the schema crate; the private source-include workaround
is removed. The release set is three packages: schema/contracts, SDK, and final
platform shims. This supersedes the two-package layout described below. Alpha
schema versions stay 1.

# One backend shader export

Compiler results now expose only `code() -> INcBytes`, tagged by `ShaderTarget`.
Text targets use UTF-8 and SPIR-V uses little-endian words. The former three code
getters are removed; bounded import still authenticates the backend format and
owns one decoded Rust `ShaderCode`. Versions remain 1 during alpha. Dynamic
example build graphs also exclude host-side renderer/compiler features through
the dedicated schema crate, without a provider build edge.

# Renderer-owned shader compilation

Shader declarations now belong to `neocogi.renderer` and `render::abi`, with Rust
snapshots and readers in `render::shader`. The renderer owns its compiler as
`render::compiler`; there is no
external compiler plugin or shared library. Each device binds compilation to its
backend format once. Requests contain stage and output limit only; results retain
the target so backends reject incompatible artifacts. Native deployments ship one
backend-specific platform library. All NCIDL versions remain 1 during alpha;
consumers and providers rebuild together when contracts change. This supersedes
the external compiler and request-target arrangements described in older entries.

# Native builds with ordinary Cargo

The library-example packaging described here is superseded by the normal
`nc_platform` library target in Unreleased above.

Dynamic examples used two standard Cargo commands: build the shim package's
backend-named library examples, then build/run the application with the same
backend/profile/target. The application's Rust build script stages the exact
prebuilt pair and tracks inputs and deployed outputs for rebuilds. The Python
plugin builder is removed. Missing libraries return a profile-specific Cargo
preparation command before model import or HDR processing. See NATIVE_LAYOUT.md
and the native example README for the current invocation.

# Backend-specific shader artifacts

The shader request now selects WGSL, SPIR-V or GLSL ES300, and success owns only
that representation. Device families choose their immutable format; importers
check target agreement and charge entry plus selected code bytes. The prior
three-output shader request/report ABI is superseded without compatibility
aliases. Rebuild SDK clients and both providers together. Optional compiler output
features enable only required Naga writers; GL keeps its guarded SPIR-V roundtrip
as temporary scratch. Compiler and platform deployment filenames both identify
the selected backend. Current contracts are in SHADER_COMPILATION.md and
GPU_RENDERING.md; older entries below describe earlier behavior.

# Two-package SDK consolidation

The current pre-release layout supersedes the eleven-package release set with
`neocogi-platform` and `neocogi-platform-dylib`. Implementations, shared contracts,
schema generation and stable build-script staging are SDK modules. The final
shim package supplies platform and independent compiler discovery roots, with
backend-suffixed deployed platform filenames. Examples and fixtures remain
unpublished. Older entries below describe the historical architecture and retain
their former names for context; current contracts are in COMPONENTS.md,
NATIVE_LAYOUT.md and RELEASING.md.

# Dependency compatibility and revision history

NeoCogi UI depends on compatible NeoCogi Platform packages. The repositories can
evolve independently; a Platform revision does not require a corresponding UI
commit. Manifests and lockfiles select reproducible dependencies, and integration
tests qualify supported combinations.

Consumer manifests and lockfiles select their Platform dependencies independently
of repository commit pairing. Upgrade related Platform packages together and
resolve shared Rust contract types from one Cargo source. See
[component ownership](../architecture/COMPONENTS.md) for current package roles.
The Platform manifests pin the published COM, host and IDL packages to `0.1.0`;
these dependency choices do not require synchronizing repository commits.

Static consumers must resolve compatible shared Rust types. Dynamically loaded
plugins must satisfy the COM SDK's interface, ABI and threading checks as well as
the behavior required by their consumers. Matching package names, version labels
or interface IDs alone do not establish compatibility. Qualify upgrades through
the affected static, shared-library and browser integration paths.

## Historical lookup

Historical commit trailers record `NeoCogi-Revision-ID` and the companion
repository. UI trailers also identify the Platform commit used at the time.
Where available, reciprocal links in `refs/notes/neocogi-companions` identify
historical combinations. These notes are optional lookup metadata, not Cargo
build inputs or a requirement for new commits.

```sh
git fetch origin refs/notes/neocogi-companions:refs/notes/neocogi-companions
git log --show-notes=neocogi-companions
```

To reproduce an old build, use that revision's manifests, lockfile and toolchain;
the historical links can help locate the repositories used. Current source
manifests and lockfiles remain authoritative for normal Cargo builds.
The [COM SDK](https://github.com/NeoCogi/neocogi-com) owns COM/IDL documentation.
