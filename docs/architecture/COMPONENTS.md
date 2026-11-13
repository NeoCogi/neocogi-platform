# Platform and graphics ownership

The release contains three packages. `neocogi-platform-contracts` owns all authored
NCIDL declarations, generated renderer/audio/platform bindings, shared types, limits and
backend-neutral validation. Its `render::abi` contains compiler contracts;
`render::shader` owns typed snapshots and bounded readers. Upload producers,
shared values, typed buffer helpers and backend traits live beside these contracts.
The optional `build` module supplies schema generation and native staging without
renderer dependencies.

The schema folders `neocogi/renderer`, `neocogi/audio` and `neocogi/platform`
define sibling namespaces. Canonical audio bindings and helpers live at
`neocogi_platform_contracts::audio`; audio imports no graphics/window namespace.
Platform discovery imports audio types for its service methods. Compatibility
uses derived contract identities and exact signatures; rebuild consumers/providers
together after toolchain or namespace changes.

`neocogi-platform` is the implementation SDK. Its `render` module exposes the
contracts crate's exact types and adds the built-in S-expression COM compiler. `gpu` owns
resource versions, recording, validation, quotas and concrete backends; builtin
`platform` owns SDL/DOM hosting and native lifetimes. Builtin `audio` owns bounded PCM
playback/capture on SDL/Web Audio without opening a window or initializing graphics.
Platform hosting includes audio through the same discovery root. `neocogi-platform-dylib`
provides thin final platform discovery roots. Examples and browser/typed-buffer
fixtures remain unpublished workspace consumers.

Native windows, contexts and graphics resources remain together under concrete
ownership. No raw handle exchange or COM downcast is introduced. Reentrant COM
calls retain the existing short-borrow rules, preparation-before-acquisition
ordering and typed resource authentication. Surface and resource contracts are
unchanged; see [drawing targets](../contracts/RENDER_SURFACES.md).

The SDK always contains audio, window hosting and shader compilation. Glow is
the default renderer; exactly one of `glow`, `vulkan`, `wgpu` or `webgl` must be
selected. Alternate renderers disable default features first. Browser WebGL uses
`webgl` with `wasm32-unknown-unknown`. `audio` and `platform` are modules, not Cargo
features. Contracts-only clients use the independent contracts crate.

`shader-python` is the only shader feature. WGSL, SPIR-V and GLSL writers are
implied by `wgpu`, `vulkan` and `glow`/`webgl`. Each compiler instance has one
immutable target; requests contain no target. Reports identify their format,
which backends validate before copying code. `test-support` supplies an internal
virtual WGSL renderer and weakly enables WGPU counters only when WGPU is selected.
Dynamic applications retain no SDK implementation edge.

The contracts crate contains the sole authored `schemas/neocogi` declarations and
compiles generated bindings once. Its own build.rs uses its embedded source helper.
Downstream build scripts depend on `neocogi-platform-contracts` with `build-support`,
then call `build::{sources, build}` with explicit `Module` selections. Generated
external imports use `neocogi_platform_contracts::render::{abi, platform}` and
`neocogi_platform_contracts::audio`. Each
independent projection generates in its own OUT_DIR from embedded canonical bytes;
no sibling checkout or another package's build output is consulted. The SDK owns
no schemas, generator, staging implementation or generated binding copies.

The shim's normal `nc_platform` cdylib exports the selected platform descriptor
with its renderer-owned compiler. Cargo features select the one backend; the
filename remains `libnc_platform.so` (target-specific suffix/prefix on macOS/Windows).
It links ordinary SDK implementations and exports exactly one discovery symbol.
The SDK exports no discovery symbol. Only the platform is
registered or loaded; it has no external compiler dependency.
The platform root also exposes `ShaderCompilerFactory` through QueryInterface.
The compiler is built into that same platform library and registers no separate
plugin descriptor.
The executable owns the one COM host; providers receive its typed runtime.

`cargo build -p neocogi-platform-dylib --no-default-features --features glow`
builds the normal platform library. The application build script stages it from
the profile's output directory beside applications. Dynamic examples compile with
only `dynamic` and load that one deployed library. Its descriptor identifies the
backend; optional --backend verifies it. Application Cargo backend features select
static implementations only.
No build script invokes Cargo. The contracts crate's `build::layout` owns output
policy and resource snapshots; ordinary build scripts replace the former nightly
staging proc macros. See
[native staging](../contracts/NATIVE_LAYOUT.md).

NeoCogi UI remains a separate consumer owning retained trees, widgets, themes,
text and composition. It uses the contracts crate for shared types and the SDK
for implementations; no compatibility crates or reexports of former package
names are retained.
