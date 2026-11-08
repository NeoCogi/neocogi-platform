# NeoCogi Platform contracts

`neocogi-platform-contracts` owns the canonical NCIDL sources and generated Rust
bindings in `render::abi` and `render::platform`, together with shared types,
limits and backend-neutral validation. `render::shader` contains typed shader
snapshots and bounded readers; the remaining render modules provide frame and
image constraints, traits, upload producers and typed buffer conveniences. It
contains no renderer implementation, Naga, SDL or native graphics loader.

Dynamic applications depend on these contracts and load a platform library.
Static applications use `neocogi-platform`, which exposes the same contract types
and supplies concrete renderers and their owned shader compilers.

Build scripts enable `build-support` and use `build::{Module, sources, build}`
for consumer-owned projections or `build::layout` for native resource staging.
All nine authored schemas live in this archive, are embedded by the helper and
are reconstructed in each consumer's own OUT_DIR. Alpha schema versions remain 1.
