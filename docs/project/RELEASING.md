# Preparing and publishing Platform packages

The release target is **0.2.0-alpha.2**, pending publication. The release set
contains three publishable packages:

1. `neocogi-platform-contracts`: schemas, generated bindings, shared types, limits, validation and build support.
2. `neocogi-platform`: renderer SDK with optional backends, owned compiler and hosting.
3. `neocogi-platform-dylib`: final platform libraries including renderer-owned compilation.

The five example/test/tool packages remain `publish = false`. This supersedes
the eleven-package release layout. Schema/build helpers and staging belong to the contracts archive; implementation
modules belong to the SDK archive. There are no private path
or Git dependencies hidden behind a facade. All internal registry requirements
pin the same exact pre-release version.

The `0.2.0-alpha.1` minor-version advance marks incompatible Rust API and schema
namespace changes since `0.1.0-alpha.2`; see the [release history](HISTORY.md).
Upgrade contracts, SDK and shims together and regenerate consumers. All NCIDL
declarations retain version 1 during alpha, so a matching interface version alone
does not establish compatibility with older providers.

## Package inputs and verification

The contracts archive owns all ten authored NCIDL files, its generation build.rs,
compiled Rust bindings, shared helpers and optional build-support APIs. The SDK
archive owns implementations and focused tests without a build script or copies
of generated bindings. Dynamic/build consumers use only the contracts crate; it has
no SDK, Naga, SDL or graphics dependency in any feature configuration. The shim
archive includes backend-named platform examples, with the selected feature and
`--examples` choosing the final shared library.

All packages include their README and combined license notices.
Their explicit manifest include lists keep repository documentation outside the
crate archives; each package's README is its packaged Markdown entry point.
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
cargo test --locked -p neocogi-platform --features platform,glow,test-support,shader-spirv
cargo test --locked -p neocogi-platform-tools
cargo tools check-packages --offline --stable-toolchain stable --allow-dirty
```

Omit `--allow-dirty` for release qualification. The checker constructs all three real
archives together, rejects unexpected release packages and duplicate schema
ownership, extracts them outside this checkout and uses only extracted contents
for internal registry patches. It builds all targets, runs focused SDK tests and
executes an independent generated COM consumer with retained shader exports and
real build-script resource staging, including restoration of deleted outputs and
propagation of changed authored bytes through Cargo rebuilds. The optional stable
run validates the same consumer without repository nightly configuration. It does not claim all native
adapters or browser execution are qualified on stable.

Native builds require SDL2 development libraries; Vulkan needs its loader.
The development workspace still pins nightly for build-std and its min-size
profile, independently of the package architecture. Build-only APIs use stable
Rust and have no proc-macro tracking or Cargo artifact-dependency requirement.

## Deployment and publication

Before dynamic acceptance, run `cargo build -p neocogi-platform-dylib --no-default-features --features glow --examples`
(or vulkan/wgpu), then build/run applications with the same target/profile and
dynamic assembly. Runtime backend selection uses --backend, independently of the
example executable's features. Qualify the same dynamic-only executable with
multiple separately built backend libraries and verify ambiguous/missing choices
reject before asset/HDR work. Copy the executable with its `lib` and optional assets
folders. Load only the selected platform; its renderer owns compilation and
has no external compiler plugin dependency. See [native staging](../contracts/NATIVE_LAYOUT.md).

Run relevant native/browser acceptance and review the actual archives before
publication. Publication remains a separate explicit action. Publish the contracts crate first, then the SDK, then the shim, waiting for each
dependency version to appear in the crates.io index before publishing its consumer.
COM/host/IDL dependencies remain the published exact `0.1.0` versions. A successful
`--list` or `--no-verify` alone is insufficient release evidence.

Shader qualification must include isolated `shader-wgsl`, `shader-spirv` and
`shader-glsl` compiler graphs. GPU features forward the selected target, and no
release compiler should enable all outputs implicitly. Stage the backend-suffixed
platform library including its compiler. Requests never select an output; results
identify it for backend validation. Rebuild generated consumers and platforms for
this alpha contract replacement; all NCIDL versions remain 1.

Minimal output ABI regressions run `cargo test -p neocogi-platform
--no-default-features --features shader-compiler,shader-wgsl --test shader_targets`
(and shader-spirv/shader-glsl), plus a shader-compiler-only build that rejects all
output configurations. They deliberately omit test-support so its WGSL fixture cannot mask
unavailable outputs. The single backend code getter must preserve its encoding
and ownership. Missing code propagates without fallback; wrong targets reject
before acquiring code, and invalid UTF-8 rejects for either text target.

Also run the matrix with all three output features together. Identical target-free
requests on different fixed-format compiler instances must keep their configured
outputs after validation and quota failures. Resource regressions reject an
otherwise consistent result for another backend without publishing a shader or
leaking reservations. The independent archive consumer generates the merged
renderer schema and exercises result-target validation through the SDK reader.
