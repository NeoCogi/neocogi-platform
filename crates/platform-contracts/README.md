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
are reconstructed in each consumer's own OUT_DIR. The pinned COM/IDL toolchain is
`0.2.0-alpha.4`; declaration identities and exact signatures are derived from the
contract without manual `#[version(...)]` attributes. Rebuild providers and
generated consumers together after upgrading this toolchain.
The sibling `neocogi.renderer`, `neocogi.audio` and `neocogi.platform` namespaces
live in matching schema folders. `Module::Audio` selects audio-only generation;
platform imports audio service types instead of owning them.

Generated data values implement `ComSerialize` and `ComDeserialize` when their
fields support persistence. Live COM capabilities cannot be reconstructed
automatically. Callers select a format through `neocogi_com::serialization::Format`;
codec implementations live in `neocogi-com-serialization` and are not a runtime
dependency of these contracts. Tests exercise NCB and JSON, including malformed
documents and owned source/field diagnostics. Deserialization checks representation;
callers still apply the subsystem's semantic validation before admitting resources.
