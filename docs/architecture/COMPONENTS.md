# Platform and graphics ownership

The release contains three packages. `neocogi-platform-contracts` owns all authored
NCIDL declarations, generated renderer/platform bindings, shared types, limits and
backend-neutral validation. Its `render::abi` contains compiler contracts;
`render::shader` owns typed snapshots and bounded readers. Upload producers,
shared values, typed buffer helpers and backend traits live beside these contracts.
The optional `build` module supplies schema generation and native staging without
renderer dependencies.

`neocogi-platform` is the implementation SDK. Its `render` module exposes the
contracts crate's exact types and adds the renderer-owned Naga compiler. `gpu` owns
resource versions, recording, validation, quotas and concrete backends; optional
`platform` owns SDL/DOM hosting and native lifetimes. `neocogi-platform-dylib`
provides thin final platform discovery roots. Examples and browser/typed-buffer
fixtures remain unpublished workspace consumers.

Native windows, contexts and graphics resources remain together under concrete
ownership. No raw handle exchange or COM downcast is introduced. Reentrant COM
calls retain the existing short-borrow rules, preparation-before-acquisition
ordering and typed resource authentication. Surface and resource contracts are
unchanged; see [drawing targets](../contracts/RENDER_SURFACES.md).

Default SDK features are empty. GPU backends `glow`, `vulkan` and `wgpu` are
additive; `webgl` reuses GL execution on Wasm. `platform` enables hosting and
requires exactly one legal target/backend selection. `shader-compiler` enables
Naga WGSL parsing; output features `shader-wgsl`, `shader-spirv` and `shader-glsl`
select emitters independently of graphics dependencies. Backends enable their
compiler and selected output. Each compiler instance has one immutable target;
requests contain no target. Reports identify their format, which backends validate
before copying code; Rust `ShaderCode` snapshots retain only that format. `test-support` includes WGSL fixtures and the compiler, weakly
enabling WGPU counters only when WGPU is selected. Applications that load
providers dynamically use schema contracts and retain no SDK implementation edge.

The contracts crate contains the sole authored `schemas/neocogi` declarations and
compiles generated bindings once. Its own build.rs uses its embedded source helper.
Downstream build scripts depend on `neocogi-platform-contracts` with `build-support`,
then call `build::{sources, build}` with explicit `Module` selections. Generated
external imports use `neocogi_platform_contracts::render::{abi, platform}`. Each
independent projection generates in its own OUT_DIR from embedded canonical bytes;
no sibling checkout or another package's build output is consulted. The SDK owns
no schemas, generator, staging implementation or generated binding copies.

The shim's main library exports the platform descriptor. Its backend-named library
examples export the selected platform descriptor with its renderer-owned compiler.
These targets provide backend-specific filenames within Cargo's one-main-library
package model. Each links ordinary SDK implementations and exports exactly one
discovery symbol. The SDK exports no discovery symbol. Only the platform is
registered or loaded; it has no external compiler dependency.
The executable owns the one COM host; providers receive its typed runtime.

`cargo build -p neocogi-platform-dylib --no-default-features --features glow --examples`
builds the selected platform with a backend-specific filename. The application build
script stages all prebuilt backend libraries from the profile's `examples/`
directory beside applications. Dynamic examples compile with only `dynamic` and
choose a library at startup: the sole deployed backend, or an explicit --backend
when several exist. Cargo backend features select static implementations only.
No build script invokes Cargo. The contracts crate's `build::layout` owns output
policy and resource snapshots; ordinary build scripts replace the former nightly
staging proc macros. See
[native staging](../contracts/NATIVE_LAYOUT.md).

NeoCogi UI remains a separate consumer owning retained trees, widgets, themes,
text and composition. It uses the contracts crate for shared types and the SDK
for implementations; no compatibility crates or reexports of former package
names are retained.
