# Platform and graphics ownership

The release contains two packages: `neocogi-platform` (an ordinary Rust library)
and `neocogi-platform-dylib` (thin final COM shared-library roots). Examples and
browser/typed-buffer fixtures remain unpublished workspace consumers. Internal
module boundaries do not require separately published packages.

The SDK's `render` module owns renderer/window contracts, generated bindings,
shared values and upload producers. `render::abi` owns compiler contracts; `render::shader` owns reflection
and bounded artifact readers. `gpu` owns resource versions, driver provenance,
validation, quotas, recording and concrete backends. The optional `render::compiler`
module supplies renderer-owned Naga translation. The optional `platform` module owns SDL/DOM
windows, input, clipboard, native devices and drawing surfaces; its private
`prepared` and `wheel` modules replace the former support library.

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
providers dynamically use SDK defaults and retain no native implementation edge.

The SDK contains the sole authored `schemas/neocogi` declarations. Its own
build.rs uses the schema helper directly, without a self dependency. Downstream
build scripts depend on the SDK with `build-support`, then call
`neocogi_platform::build::{sources, build}` with explicit `Module` selections.
Imports use `neocogi_platform::render::{abi, platform}`.
Each consumer generates into its own OUT_DIR from embedded canonical source bytes;
no sibling checkout path or another package's generated output is consulted.
Generation and staging dependencies are build-only when used as build dependencies.

The shim's main library exports the platform descriptor. Its backend-named library
examples export the selected platform descriptor with its renderer-owned compiler.
These targets provide backend-specific filenames within Cargo's one-main-library
package model. Each links ordinary SDK implementations and exports exactly one
discovery symbol. The SDK exports no discovery symbol. Only the platform is
registered or loaded; it has no external compiler dependency.
The executable owns the one COM host; providers receive its typed runtime.

`cargo build -p neocogi-platform-dylib --no-default-features --features glow --examples`
builds the selected platform with a backend-specific filename. The application build
script stages that library from the profile's `examples/` directory beside applications.
No build script invokes Cargo. SDK `build::layout` owns output policy and resource snapshots; ordinary
build scripts replace the former nightly staging proc macros. See
[native staging](../contracts/NATIVE_LAYOUT.md).

NeoCogi UI remains a separate consumer owning retained trees, widgets, themes,
text and composition. It must migrate imports to the SDK module paths; no
compatibility crates or reexports of former package names are retained.
