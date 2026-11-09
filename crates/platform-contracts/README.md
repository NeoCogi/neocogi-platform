# NeoCogi Platform contracts

`neocogi-platform-contracts` owns the canonical NCIDL sources and generated Rust
bindings in `render::abi`, `audio` and `render::platform`, together with shared types,
limits and backend-neutral validation. `render::shader` contains typed shader
snapshots and bounded readers; the remaining render modules provide frame and
image constraints, traits, upload producers and typed buffer conveniences. It
contains no renderer implementation, Naga, SDL or native graphics loader.

Dynamic applications depend on these contracts and load a platform library.
Static applications use `neocogi-platform`, which exposes the same contract types
and supplies concrete renderers and their owned shader compilers.

Build scripts enable `build-support` and use `build::{Module, sources, build}`
for consumer-owned projections or `build::layout` for native resource staging.
All ten authored schemas live in this archive, are embedded by the helper and
are reconstructed in each consumer's own OUT_DIR. Alpha schema versions remain 1.
The sibling `neocogi.renderer`, `neocogi.audio` and `neocogi.platform` namespaces
live in matching schema folders. `Module::Audio` selects audio-only generation;
platform imports audio service types instead of owning them. Rebuild providers
and generated consumers together after this alpha namespace change.
