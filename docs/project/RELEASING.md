# Preparing and publishing Platform packages

The current published release is **0.2.0**. The release set
contains three publishable packages:

1. `neocogi-platform-contracts`: schemas, generated bindings, shared types, limits, validation and build support.
2. `neocogi-platform`: renderer SDK with optional backends, owned compiler and hosting.
3. `neocogi-platform-dylib`: final platform libraries including renderer-owned compilation.

The five example/test/tool packages remain `publish = false`. This supersedes
the eleven-package release layout. Schema/build helpers and staging belong to the contracts archive; implementation
modules belong to the SDK archive. There are no private path
or Git dependencies hidden behind a facade. All internal registry requirements
pin the same exact release version.

The `0.2.0-alpha.1` minor-version advance marks incompatible Rust API and schema
namespace changes since `0.1.0-alpha.2`; see the [release history](HISTORY.md).
Upgrade contracts, SDK and shims together and regenerate consumers. COM/IDL
`0.2.0-alpha.4` derives nominal identities and exact compatibility signatures from
the contract; deprecated manual declaration versions are removed. Rebuild all
providers and consumers together for this toolchain migration. Old generated
identities and signatures are incompatible even where field layouts are unchanged.
The platform shim exports `nc_get_plugin_descriptor_v2`, matching this SDK loader.

## Package inputs and verification

The contracts archive owns all ten authored NCIDL files, its generation build.rs,
compiled Rust bindings, shared helpers and optional build-support APIs. The SDK
archive owns implementations and focused tests without a build script or copies
of generated bindings. Dynamic/build consumers use only the contracts crate; it has
no SDK, Naga, SDL or graphics dependency in any feature configuration. The shim
archive exposes one normal `nc_platform` cdylib target. Its selected backend
feature controls the implementation while its filename remains backend-independent.

All packages include their README and combined license notices. The SDK and shim
also include `docs/images`, screenshot credits/license notices and the saved
limited shader benchmark in `docs/benchmarks`. Their READMEs contain standalone
static/dynamic setup, feature selection and repository example commands. Release
inspection rejects either archive if any required documentation asset is missing.
The SDK's copies are canonical; the shim ships byte-identical documentation assets.
The root README links directly to the SDK's images and measurement report.

Other repository guides remain outside crate archives; each package's README is
its packaged Markdown entry point. Keep links to those guides absolute in crate
READMEs and use package-local paths for included assets.
The [documentation index](../README.md) points to current contracts and
architecture, examples, tools and maintainer release/history records in the
repository. Git source archives include these tracked documents. Keep worked
examples with their current contract and record replaced APIs in
[history](HISTORY.md), retiring separate migration guides after preserving their
useful content.

Run:

```sh
cargo fetch --locked
cargo check --locked --workspace --all-targets
cargo test --locked -p neocogi-platform-contracts --features build-support,test-support
cargo test --locked -p neocogi-platform --features test-support
cargo test --locked -p neocogi-platform-tools
cargo tools check-packages --offline --stable-toolchain stable --allow-dirty
```

Omit `--allow-dirty` for release qualification. The checker constructs all three real
archives together, rejects unexpected release packages and duplicate schema
ownership, extracts them outside this checkout and uses only extracted contents
for internal registry patches. It builds all targets, runs focused SDK tests and
executes an independent generated COM consumer with retained shader exports and
real build-script resource staging, including restoration of deleted outputs and
propagation of changed authored bytes through Cargo rebuilds. It also builds the
shim as a nonmember registry-style dependency with ordinary `cargo build --lib`,
reads Cargo's artifact receipt, loads the actual library, and calls its built-in
compiler through independently generated COM bindings. The optional stable run
builds both the shim and consumer without repository nightly configuration. It does not claim all native
adapters or browser execution are qualified on stable.

Native builds require SDL2 development libraries; Vulkan needs its loader.
The development workspace uses the rolling nightly toolchain for build-std and its min-size
profile, independently of the package architecture. Build-only APIs use stable
Rust and have no proc-macro tracking or Cargo artifact-dependency requirement.

## Deployment and publication

Before dynamic acceptance, run `cargo build -p neocogi-platform-dylib --no-default-features --features glow`
(or vulkan/wgpu), then build/run applications with the same target/profile and
dynamic assembly. The shim's Cargo features choose its backend; optional runtime
--backend verifies that choice. Qualify automatic descriptor identification and
mismatched requests before asset/HDR work. Copy the executable with its `lib` and optional assets
folders. Load only the selected platform; its renderer owns compilation and
has no external compiler plugin dependency. See [native staging](../contracts/NATIVE_LAYOUT.md).

Run relevant native/browser acceptance and review the actual archives before
publication. Publication remains a separate explicit action. Publish the contracts crate first, then the SDK, then the shim, waiting for each
dependency version to appear in the crates.io index before publishing its consumer.
COM/host/IDL dependencies use the published exact `0.2.0-alpha.4` versions. A successful
`--list` or `--no-verify` alone is insufficient release evidence.

Shader qualification includes isolated `wgpu`, `vulkan` and `glow` renderer
builds. The compiler is builtin; renderer features select their output writers.
`shader-python` is the only shader feature. Stage the normal platform library
including its compiler. Requests never select an output; results identify it for
backend validation. Rebuild generated consumers and platforms after contract or
COM/IDL toolchain changes.

Run `cargo test -p neocogi-platform --no-default-features --features wgpu
--test shader_targets` and repeat with `vulkan` and `glow`. Also run with ordinary defaults to verify Glow selection. These runs
omit `test-support` so the virtual WGSL renderer cannot mask disabled outputs.
The single backend code getter preserves encoding and ownership. Missing code
propagates without fallback, wrong targets reject before code acquisition, and
invalid UTF-8 rejects for either text target.

Invalid zero/multiple renderer selections are compile-time errors. Identical target-free
requests on different fixed-format compiler instances must keep their configured
outputs after validation and quota failures. Resource regressions reject an
otherwise consistent result for another backend without publishing a shader or
leaking reservations. The independent archive consumer generates the merged
renderer schema and exercises result-target validation through the SDK reader.
