# Migrating to the two-package SDK

The former eleven library/tooling packages are replaced by `neocogi-platform`
and the thin `neocogi-platform-dylib` package. This pre-release migration retains
no compatibility packages. Authored declarations, interface identities, plugin
names and runtime ownership remain unchanged.

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
