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
are reconstructed in each consumer's own `OUT_DIR`. The helper maintains only
the reserved `neocogi-shared-sources` directory. An alias for the outer
`OUT_DIR` is supported. An existing file, link or junction at the reserved
entry fails generation before any pruning. The pinned IDL compiler is
`0.4.0-alpha.2` and generated Rust refers to the `neocogi_com_sdk` crate.
An independent generator consumer declares `neocogi-com-sdk` directly at the
same version. Rebuild providers and generated consumers together when changing
the toolchain. The schema determines declaration identities and exact signatures.
The sibling `neocogi.renderer`, `neocogi.audio` and `neocogi.platform` namespaces
live in matching schema folders. For example, pass
`sources(&[(Module::Audio, "app_projection::audio")])`, declare
`extern crate self as app_projection`, and include `IDL_NEOCOGI_AUDIO` in `audio`.
With all three namespaces selected, generated cross-namespace references use those
local paths and Contracts is only a build dependency. Reachable namespaces left
out of the selection use their canonical Rust definitions and require Contracts
as a regular dependency. Platform imports its audio service types from the audio
namespace in either case.

Generated data values implement `ComSerialize` and `ComDeserialize` when their
fields support persistence. Live COM capabilities cannot be reconstructed
automatically. Callers select a format through `neocogi_com_sdk::serialization::Format`.
The application supplies its chosen codec from `neocogi-com-serialization`.
Tests exercise NCB and JSON, including malformed
documents and owned source/field diagnostics. Deserialization checks representation;
callers still apply the subsystem's semantic validation before admitting resources.
