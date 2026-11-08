# Preparing and publishing Platform packages

The release target is **0.1.0-alpha.2**. Exactly two packages are published:

1. `neocogi-platform`: reusable Rust SDK, contracts, optional providers and build support.
2. `neocogi-platform-dylib`: thin final platform/compiler shared-library roots.

The four existing example/test packages remain `publish = false`. This supersedes
the eleven-package release layout. Internal implementation modules, schema helpers
and native staging code are part of the SDK archive. There are no private path
or Git dependencies hidden behind a facade. All internal registry requirements
pin the same exact pre-release version.

## Package inputs and verification

The SDK archive owns the nine authored NCIDL files, the schema helper, build.rs,
implementation modules and focused tests. Its own build.rs uses the helper's
source directly; downstream generation uses an SDK build dependency with
`build-support`. Runtime defaults enable neither generation/staging dependencies
nor SDL, Naga or a graphics backend. The shim archive includes both final link
roots as backend-named library examples, selected together by `--examples` and
the matching backend feature.
All packages include their README and combined license notices.

Run:

```sh
cargo fetch --locked
cargo check --locked --workspace --all-targets
cargo test --locked -p neocogi-platform --features platform,glow,test-support,shader-spirv,build-support
python3 -B -m unittest discover -s scripts -p 'test_*.py'
python3 -B scripts/check_packages.py --offline --stable-toolchain stable --allow-dirty
```

Omit `--allow-dirty` for release qualification. The checker constructs both real
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
selected dynamic backend. Copy the executable with its `lib` and optional assets
folders. Load the compiler before the platform; COM identities and dependencies
are unchanged. See [native staging](../contracts/NATIVE_LAYOUT.md).

Run relevant native/browser acceptance and review the actual archives before
publication. Publication remains a separate explicit action. Publish the SDK
first and wait for it to appear in the crates.io index, then publish the shim.
COM/host/IDL dependencies remain the published exact `0.1.0` versions. A successful
`--list` or `--no-verify` alone is insufficient release evidence.

Shader qualification must include isolated `shader-wgsl`, `shader-spirv` and
`shader-glsl` compiler graphs. GPU features forward the selected target, and no
release compiler should enable all outputs implicitly. Stage the backend-suffixed
compiler and platform pair together. Shader request/report ABI signatures changed;
rebuild generated consumers and both providers for this release.

Minimal output ABI regressions run `cargo test -p neocogi-platform
--no-default-features --features shader-compiler,shader-wgsl --test shader_targets`
(and shader-spirv/shader-glsl), plus a shader-compiler-only build that rejects all
formats. They deliberately omit test-support so its WGSL fixture cannot mask
unavailable outputs. Poisoned unselected getters prove import copies only its
selected representation; a missing selected getter propagates without fallback.
