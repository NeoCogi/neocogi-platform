# Platform and graphics ownership

The contract packages expose renderer/platform and shader namespaces with their
authored NCIDL source trees. Consumers generate their own bindings using
`neocogi_render_core::SCHEMA_ROOT` and, when needed,
`neocogi_shader_core::SCHEMA_ROOT`. Imported Rust helper types use the bindings
of their ordinary dependency. There is one authored owner per namespace; no
consumer maintains a copy of another package's declarations.

`neocogi-render-gpu` owns logical resource versions, driver provenance, command
validation, quotas and scoped recording. Backends realize those immutable
versions and retain submitted native allocations until completion. The compiler
is a required typed dependency acquired before native device construction.

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
