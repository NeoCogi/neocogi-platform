# Migrating to contracts, renderer SDK and platform shims

The release now has `neocogi-platform-contracts` for authored schemas, generated
bindings, shared types, limits, validation and build helpers; `neocogi-platform`
for renderer/compiler/hosting implementations; and `neocogi-platform-dylib` for
final shared libraries. The contracts crate never depends on the SDK, Naga or
native graphics. All alpha NCIDL versions remain 1; rebuild generated clients and
providers together.

| Responsibility | Location |
| --- | --- |
| Renderer contracts and upload producers | `neocogi_platform_contracts::render` |
| GPU resources, recording and backends | `neocogi_platform::gpu` |
| Shader contracts and reflection | `neocogi_platform_contracts::render::shader` |
| Naga compiler provider | `neocogi_platform::render::compiler` (`shader-compiler`) |
| SDL/DOM provider | `neocogi_platform::platform` (`platform` plus backend) |
| Frame preparation and wheel conversion | private platform modules |
| Schema generation helper | `neocogi_platform_contracts::build` (`build-support`) |
| Output policy and resource staging | `neocogi_platform_contracts::build::layout` |

SDK defaults are empty. Static Rust applications explicitly enable their backend,
and `platform`, initialize one COM host, and register only
`platform::plugin_descriptor()`. The selected backend enables and owns compilation.
Dynamic applications depend on the contracts crate without the SDK and
load one platform shared library containing the renderer and its compiler.

Build scripts declare `neocogi-platform-contracts` with `build-support` and use
`build::{Module, sources, build}`. The single authored schema tree is packaged
under `crates/platform-contracts/schemas`; embedded source bytes are reconstructed in each
consumer's own OUT_DIR. Replace staging macro invocations with ordinary
`build::layout` calls and Cargo rerun directives for returned inputs/outputs.

Build final shared libraries with `cargo build -p neocogi-platform-dylib
--no-default-features --features glow --examples` (or vulkan/wgpu) before dynamic
applications. Cargo builds the selected backend's explicitly named library
example. The application's Rust build script stages their exact filenames from
the same profile's `examples/` directory. This replaces
artifact dependencies and nightly compile-time staging macros. See
[native deployment](../contracts/NATIVE_LAYOUT.md).

## Selected shader artifact migration

Remove `ShaderCompileRequest.target`. Compilation output is fixed at renderer
construction. A successful report still identifies `Wgsl`, `SpirV` or `GlslEs300`
through `target()` and `ShaderInfo.target`, so the backend can validate it before
import. Replace the three format-specific code getters with `code()`, returning
`INcBytes` encoded as UTF-8 or little-endian SPIR-V according to the result tag. Summary code size
is `ShaderInfo.code_byte_count`, with `target` replacing the three old format
lengths. `PreparedShader::new` takes the expected target before entry bytes.
Rust snapshots replace `wgsl/glsl/spirv` fields with one `ShaderCode` enum.
Native GL names are optional in plain reflection and unavailable through COM for
non-GL targets. Device families implement their immutable `shader_target` choice.

Standalone CPU consumers enable `shader-compiler` plus output features; backend
features enable their own compiler and output. Replace `GpuDevice::new(core, compiler)`
with fallible `GpuDevice::new(core)`. The compiler implementation lives under
`render::compiler`; no compiler descriptor is registered and no compiler shared
library is deployed. All NCIDL versions remain 1 during alpha. See [shader contracts](../contracts/SHADER_COMPILATION.md).
