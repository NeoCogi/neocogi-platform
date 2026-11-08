# NeoCogi Platform

One Rust SDK for platform and graphics contracts, typed GPU resources, portable
shader compilation and optional SDL/browser hosting. Empty default features need
no SDL, graphics loader or Naga implementation.

Modules: `render` (contracts and sources), `gpu` (recording and backends), `shader`
(portable compiler contracts), optional `compiler` (`shader-compiler` feature),
optional `platform` (hosting plus exactly one backend), and optional `build`
(`build-support`, for build scripts).

Native backend features `glow`, `vulkan` and `wgpu` are additive for GPU users.
The `platform` feature selects exactly one backend. Browser hosting uses
`platform,webgl`. `test-support` exposes acceptance fixtures and the compiler.
Build scripts enable `build-support` to generate consumer-owned IDL projections
or stage exclusively owned resource snapshots through `build::layout`.

Rust programs link this crate normally and explicitly register compiler and
platform descriptors. Dynamic applications depend on the same SDK with empty
features and load final artifacts from `neocogi-platform-dylib`.

See the repository documentation for contracts, ownership and deployment.
