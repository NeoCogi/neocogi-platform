# Migrating to the two-package SDK

The former eleven library/tooling packages are replaced by `neocogi-platform`
and the thin `neocogi-platform-dylib` package. This pre-release migration retains
no compatibility packages. The package consolidation preserves plugin names and native ownership. The
subsequent selected-target shader contract changes request/report ABI signatures;
rebuild all compiler and renderer consumers together.

| Former responsibility | SDK location |
| --- | --- |
| Renderer contracts and upload producers | `neocogi_platform::render` |
| GPU resources, recording and backends | `neocogi_platform::gpu` |
| Shader contracts and reflection | `neocogi_platform::shader` |
| Naga compiler provider | `neocogi_platform::compiler` (`shader-compiler`) |
| SDL/DOM provider | `neocogi_platform::platform` (`platform` plus backend) |
| Frame preparation and wheel conversion | private platform modules |
| Schema generation helper | `neocogi_platform::build` (`build-support`) |
| Output policy and resource staging | `neocogi_platform::build::layout` |

SDK defaults are empty. Static Rust applications explicitly enable their backend,
`platform` and `shader-compiler`, initialize one COM host, and register
`compiler::plugin_descriptor()` before `platform::plugin_descriptor()`. Dynamic
applications depend on SDK contracts without implementation features and load the
same compiler/platform identities from the final shims.

Build scripts declare the SDK with `build-support` and use
`build::{Module, sources, build}`. The single authored schema tree is packaged
under `crates/platform/schemas`; embedded source bytes are reconstructed in each
consumer's own OUT_DIR. Replace staging macro invocations with ordinary
`build::layout` calls and Cargo rerun directives for returned inputs/outputs.

Build final shared libraries with `python3 scripts/build_plugins.py --backend glow`
(or vulkan/wgpu) before dynamic applications. The script explicitly builds the
shim's main library and `nc_shader_compiler` library-example target, reads Cargo
JSON artifact paths and stages backend-suffixed platform filenames. This replaces
artifact dependencies and nightly compile-time staging macros. See
[native deployment](../contracts/NATIVE_LAYOUT.md).

## Selected shader artifact migration

Set `ShaderCompileRequest.target` to `Wgsl`, `SpirV` or `GlslEs300`. Success exports
only that code getter; other formats return `NC_E_NOINTERFACE`. Summary code size
is `ShaderInfo.code_byte_count`, with `target` replacing the three old format
lengths. `PreparedShader::new` takes the expected target before entry bytes.
Rust snapshots replace `wgsl/glsl/spirv` fields with one `ShaderCode` enum.
Native GL names are optional in plain reflection and unavailable through COM for
non-GL targets. Device families implement their immutable `shader_target` choice.

Standalone CPU consumers enable `shader-compiler` plus output features; backend
features forward their own output without linking a compiler fallback. Compiler
shared-library filenames now include the backend suffix as well as platform
filenames. See [shader contracts](../contracts/SHADER_COMPILATION.md).
