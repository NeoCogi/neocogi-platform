# Platform and graphics ownership

`neocogi-platform-build` owns the authored renderer, platform and shader NCIDL
files and the shared generation helper. Consumers declare it as a build dependency
and generate into their own `OUT_DIR` from its embedded canonical source bytes.
Derived input files are reconstructed in that same consumer-owned output area;
a cached helper never relies on its previous source checkout remaining present.
They use
`neocogi_platform_build::sources` with explicit `Module` selections, then `build`.
Consumers adding their own namespaces extend the returned request with their own
source roots and module locations. Imported Rust helper types use the bindings of
their ordinary library dependency. There is one authored owner per namespace; no
consumer copies declarations or searches sibling checkout directories.

The runtime contract libraries expose generated bindings and Rust conveniences.
Their former `SCHEMA_ROOT` constants are removed: build-time consumers use the
build-only package instead, avoiding runtime dependency on the IDL compiler.
All authored declaration versions and COM identities remain unchanged.

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

`neocogi-platform-plugin` owns SDL/DOM windows, event normalization, text
input, clipboard, native devices, framebuffer views and independent image surfaces. Keeping native owners
together makes construction, destruction and recovery explicit without passing
raw window/context handles across COM. Generic commands contain application
geometry and uniforms; clients own mesh, material and camera meaning.

Each resource group selects a real surface and explicit operation permissions at
creation. There is one group constructor; a UI compositor uses ordinary groups
with depth operations disabled. Several independently owned groups can draw into
one surface. Framebuffer views join the window's ordered frame without acquiring
or presenting another frame; image surfaces expose their own renderer and publish
immutable color images. See [drawing targets](../contracts/RENDER_SURFACES.md).

Its reusable Rust library is `nc_platform`; `neocogi-platform-plugin-dylib`
produces the final native `nc_platform` shared library. Backend-specific catalog
names are `neocogi.platform.{glow,vulkan,wgpu,webgl}`. These supersede the former
renderer-package, artifact and catalog names without compatibility aliases.
Public authored platform/renderer contracts and their interface IDs remain in
`neocogi-render-core`. See the [consumer migration](../project/PLATFORM_PLUGIN_MIGRATION.md).

Applications own their COM runtime and choose dynamic, static or browser
assembly. Final `*-dylib` packages alone own loader exports; reusable libraries
have no discovery symbol. The final application selects target profiles and
its native artifact set. `neocogi-native-layout` stages Cargo-selected outputs
without starting a nested Cargo build. Its build-only companion,
neocogi-native-layout-core, owns output policy and exclusive resource snapshots
without a renderer/COM/IDL dependency. See [native staging](../contracts/NATIVE_LAYOUT.md).

NeoCogi UI owns retained trees, bounded fixed UI geometry, input routing,
logical windows/popups, widgets, text/icon/nine-patch composition, assets and
skins. Its frontend-dependent acceptance tests run in its own workspace against
these platform contracts. Platform tests cover graphics and OS behavior without
depending on the UI implementations.

The explicit renderer `test-support` feature provides typed factory-created
weak observations and native acceptance readback. It is absent from ordinary
provider builds and exposes no raw native API handle or COM downcast mechanism.

Native executable examples belong to neocogi-platform-examples rather than the
provider package. Its static mode registers linked shader/platform descriptors;
dynamic mode uses the artifact-only neocogi-example-plugins composition. Both
own one SDK host and acquire the same typed root. Upload producers come from
render-core, so dynamic executables have no normal GPU/provider implementation
dependency. Assets and libraries are executable-relative; scene semantics stay
in the application.

Shared acceptance fixtures are optional library APIs, not sibling source includes.
`neocogi-shader-core/test-support` owns portable-profile inputs used by compiler
and GPU tests. `neocogi-render-gpu::testing` owns the derivative and sampled-image
COM fixtures used by native and browser tests. Platform's `test-support` enables
the GPU fixtures explicitly. Ordinary builds do not compile these fixtures.
