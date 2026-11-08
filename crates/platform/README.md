# NeoCogi Platform

One Rust SDK for platform and graphics contracts, typed GPU resources, portable
shader compilation and optional SDL/browser hosting. Empty default features need
no SDL, graphics loader or Naga implementation.

Modules: `render` (contracts and sources), `gpu` (recording and backends), `render::shader`
(portable compiler snapshots and readers), optional `render::compiler` (`shader-compiler` feature plus selected outputs),
optional `platform` (hosting plus exactly one backend), and optional `build`
(`build-support`, for build scripts).

Native backend features `glow`, `vulkan` and `wgpu` are additive for GPU users.
The `platform` feature selects exactly one backend. Browser hosting uses
`platform,webgl`. `test-support` exposes acceptance fixtures and the compiler.
Build scripts enable `build-support` to generate consumer-owned IDL projections
or stage exclusively owned resource snapshots through `build::layout`.

Rust programs link this crate normally and register the platform descriptor.
Its renderer constructs and owns shader compilation directly. Dynamic applications depend on the same SDK with empty
features and load final artifacts from `neocogi-platform-dylib`.

See the repository documentation for contracts, ownership and deployment.

Compiler targets are fixed at renderer construction: `shader-wgsl` (WGPU),
`shader-spirv` (Vulkan), and `shader-glsl` (GLES3/WebGL2). Backends enable their
compiler and required output. CPU-only tools can select `shader-compiler` plus
output features and construct a fixed-format instance. Requests have no target.
A report identifies and exports exactly the compiler's configured format;
other format/native-name getters return `NC_E_NOINTERFACE`. Rust `ShaderCode`
owns one artifact, and quotas charge entry plus selected code bytes only.
