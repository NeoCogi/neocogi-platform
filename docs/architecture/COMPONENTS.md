# Platform and graphics ownership

The contract packages expose renderer/platform and shader namespaces with their
authored NCIDL source trees. Consumers generate their own bindings using
`neocogi_render_core::SCHEMA_ROOT` and, when needed,
`neocogi_shader_core::SCHEMA_ROOT`. Imported Rust helper types use the bindings
of their ordinary dependency. There is one authored owner per namespace; no
consumer maintains a copy of another package's declarations.

`neocogi-render-gpu` owns logical resource versions, driver provenance, command
validation, quotas, scoped recording and concrete implementations under
`backends::{glow,vulkan,wgpu,webgl}`. Backends realize those immutable
versions and retain submitted native allocations until completion. The compiler
is a required typed dependency acquired before native device construction.

The GPU crate enables no backend by default. Optional native backend features
are additive and retain distinct concrete resource families and driver identities.
WebGL shares the GL executor; target-specific dependencies keep SDL out of browser
builds. CPU-only consumers and virtual fixtures require no graphics library.
`test-support` weakly enables WGPU allocation counters only if WGPU is already
selected. The native counter regression therefore requires `wgpu,test-support`.

Transactional native construction cleanup belongs to the GPU crate. It is private
to that crate, avoiding a dependency back to `render-support`, whose authenticated
frame preparation already depends on GPU records. The former four backend crates
and support-layer cleanup path are superseded, with no compatibility reexports.

The selected renderer plugin owns SDL/DOM windows, event normalization, text
input, clipboard, native devices and offscreen surfaces. Keeping native owners
together makes construction, destruction and recovery explicit without passing
raw window/context handles across COM. Generic commands contain application
geometry and uniforms; clients own mesh, material and camera meaning.

Applications own their COM runtime and choose dynamic, static or browser
assembly. Final `*-dylib` packages alone own loader exports; reusable libraries
have no discovery symbol. The final application selects target profiles and
its native artifact set. `neocogi-native-layout` stages Cargo-selected outputs
without starting a nested Cargo build.

NeoCogi UI owns retained trees, bounded fixed UI geometry, input routing,
logical windows/popups, widgets, text/icon/nine-patch composition, assets and
skins. Its frontend-dependent acceptance tests run in its own workspace against
these platform contracts. Platform tests cover graphics and OS behavior without
depending on the UI implementations.

The explicit renderer `test-support` feature provides typed factory-created
weak observations and native acceptance readback. It is absent from ordinary
provider builds and exposes no raw native API handle or COM downcast mechanism.
