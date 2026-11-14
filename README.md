# NeoCogi Platform

A Rust workspace for native/browser platform services, portable GPU resources,
scoped graphics commands, shader compilation and backend execution. Public APIs
use typed COM interfaces. PCM playback, microphone capture and capability queries
use native SDL or browser Web Audio. Applications can use graphics directly or combine it
with [NeoCogi UI](https://github.com/NeoCogi/neocogi-ui).

## Workspace

| Package | Responsibility |
| --- | --- |
| `crates/platform-contracts` (`neocogi-platform-contracts`) | Schemas, generated bindings, shared types, limits, validation and build support |
| `crates/platform` (`neocogi-platform`) | GPU recording/backends, renderer-owned compiler, builtin audio and window hosting |
| `crates/platform-dylib` (`neocogi-platform-dylib`) | Thin platform shared-library entrypoints including the renderer compiler |
| `examples/native`, `tests/*` | Unpublished application and acceptance consumers |
| `scripts` (`neocogi-platform-tools`) | Unpublished Rust development and qualification tools, run with `cargo tools` |

Platform and graphics implementations share native lifetime ownership. Native
windows, SDL/DOM objects and API handles remain private; callers receive typed
capabilities. The renderer accepts sealed generic commands and has no widget,
theme, camera, material or mesh model.

## Build and run

The checked-in toolchain selects the required nightly, standard-library sources
and WebAssembly target. Native builds require SDL2 development libraries; Vulkan
also needs the Vulkan loader. On Debian/Ubuntu these are `libsdl2-dev` and
`libvulkan-dev`. A native display or an appropriate software driver is needed
for execution.

Fetch dependencies first: some tests inspect every backend and the browser target
without network access, even when those dependencies are not used by the current build.

```sh
cargo fetch --locked
cargo check --locked --workspace --all-targets
cargo test --locked -p neocogi-platform-contracts --features build-support
cargo test --locked -p neocogi-platform --features test-support
cargo run --locked -p neocogi-platform-examples --bin triangle -- 3
```

For headless native GL qualification:

```sh
SDL_VIDEODRIVER=offscreen cargo run --locked -p neocogi-platform-examples --bin triangle -- 3
SDL_VIDEODRIVER=offscreen cargo test --locked -p neocogi-platform --features test-support --lib -- --include-ignored --test-threads=1
```

The SDK includes audio, window hosting and shader compilation, with **Glow as the
default renderer**. Select Vulkan or WGPU with `--no-default-features --features
vulkan` or `wgpu`; exactly one renderer is required. Browser hosting uses
`--no-default-features --features webgl` and `wasm32-unknown-unknown`.
Build scripts and contracts-only clients use `neocogi-platform-contracts`, which
has no SDK/native implementation dependency. Enable its `build-support` feature
for generation and resource staging.
The built-in COM compiler accepts S-expressions and, with the optional
`shader-python` feature, Python-like shader programs,
validates both through one typed IR, and directly emits backend shaders.
The platform root exposes `ShaderCompilerFactory` through QueryInterface.
Shader output is fixed by each renderer: WGPU uses WGSL, Vulkan uses SPIR-V and
GLES3/WebGL uses ES300. The compiler is built in; renderer features select the
available writers. `shader-python` is the only shader Cargo feature.

Native examples select `static,glow` by default. Use `--no-default-features
--features static,vulkan` or `static,wgpu` for alternatives. For dynamic assembly,
first run `cargo build -p neocogi-platform-dylib --no-default-features --features glow` (or vulkan/wgpu),
then select only `dynamic` in the application. It loads the deployed
`libnc_platform.so` (target-specific suffix/prefix on macOS/Windows). Backend
selection belongs to the library build; optional `-- --backend vulkan` verifies
that the deployed library uses Vulkan. Pass the same
`--profile` to both commands; min-size runs require preparing `--profile min-size`. Normal Rust
applications link the SDK; final shared-library shims expose the existing COM ABI.

The native glTF examples include a textured DamagedHelmet and animated GPU-skinned
BrainStem, both with arcball rotation, pan and zoom.
Both use Cannon Exterior HDR lighting, filtered reflections and a skybox.

| Damaged Helmet | BrainStem |
| :---: | :---: |
| <img src="crates/platform/docs/images/damaged-helmet.png" alt="Damaged Helmet viewed diagonally from slightly above, with textured metallic materials and the Cannon Exterior skybox" width="440"> | <img src="crates/platform/docs/images/brainstem.png" alt="BrainStem rendered with GPU skinning and the Cannon Exterior skybox" width="440"> |

Captured from the examples using the Glow backend. Damaged Helmet is by
theblueturtle_ and ctxwing ([license](examples/native/gltf/DamagedHelmet-LICENSE.md));
BrainStem is by Keith Hunter, © Smith Micro Software, Inc.
([license](examples/native/gltf/BrainStem-LICENSE.md)). The Cannon Exterior environment
is by Greg Zaal ([CC0](examples/native/gltf/Cannon_Exterior-LICENSE.md)).

```sh
cargo tools fetch-gltf-examples
cargo run --locked -p neocogi-platform-examples --bin gltf_damaged_helmet
cargo run --locked -p neocogi-platform-examples --bin gltf_brainstem
```

Load the platform shared object in the examples with:

~~~sh
cargo build --locked -p neocogi-platform-dylib --no-default-features --features glow
cargo run --locked -p neocogi-platform-examples --bin gltf_brainstem --no-default-features --features dynamic
~~~

The application build stages the prebuilt platform library, which includes its
renderer-owned compiler. Requests carry no shader target; results identify it for
backend validation. See [native assembly and relocation](examples/native/README.md).

Left-drag rotates, right/middle-drag pans, and the wheel zooms. Press R to reset,
Space to pause animation, B to toggle the skybox, or Escape to close. See the [example guide](examples/native/gltf/README.md)
for backend selection, bounded headless runs, supported glTF features and asset licenses.

The Platform workspace pins the published COM, host and IDL packages to exact
`0.2.0`. Rebuild generated consumers and providers together after this
toolchain upgrade. Shared dependencies participate in one application Cargo graph.
Independent builds can reuse compatible artifacts through `CARGO_TARGET_DIR`.

See [documentation](docs/README.md), [component ownership](docs/architecture/COMPONENTS.md),
[audio contracts](docs/contracts/AUDIO.md),
[GPU contracts](docs/contracts/GPU_RENDERING.md), [shader compilation](docs/contracts/SHADER_COMPILATION.md),
and [dependency compatibility and revision history](docs/project/HISTORY.md).

The current release target is `0.2.0`. See [release preparation](docs/project/RELEASING.md)
for the published package set, self-contained build inputs and verification.

Asset downloads, package qualification, browser tests and local debugging helpers
are Rust commands. See [repository tools](scripts/README.md); no Python interpreter
is required.

## Shader execution comparison: limited sample

This fixed snapshot covers only the handful of shaders used by this repository's
Triangle, Damaged Helmet and BrainStem examples. It is a small, non-exhaustive
sample; results apply to these workloads and this hardware, without establishing
general shader-compiler performance.

Measured on 2026-11-12: Intel Arc Meteor Lake, Mesa 26.0.8, Linux 7.0.0-42,
rustc 1.102.0-nightly (`3ef0c22cd`). Both revisions used `min-size`, rebuilt std
and immediate-abort panics. The Naga 30.0.1 implementation is revision `1a27094`;
our optimized S-expression compiler is revision `488291b`. These are historical
measurements of those revisions.

| Backend / output | Example | Naga FPS | Our compiler FPS | FPS change |
| --- | --- | ---: | ---: | ---: |
| Vulkan / SPIR-V | Triangle | 949.4 | 942.4 | -0.7% |
| Vulkan / SPIR-V | Damaged Helmet | 77.8 | 77.9 | ≈0.0% |
| Vulkan / SPIR-V | BrainStem | 82.4 | 83.0 | +0.7% |
| GLES3 / GLSL ES3 | Triangle | 2,830.3 | 2,817.2 | -0.5% |
| GLES3 / GLSL ES3 | Damaged Helmet | 76.1 | 77.7 | +2.1% |
| GLES3 / GLSL ES3 | BrainStem | 74.6 | 75.1 | +0.7% |
| WGPU / WGSL | Triangle | 789.8 | 750.8 | -4.9% |
| WGPU / WGSL | Damaged Helmet | 76.1 | 75.1 | -1.4% |
| WGPU / WGSL | BrainStem | 67.9 | 67.7 | -0.3% |

FPS is 300 divided by the median elapsed time of three alternating runs per
implementation, following 60-frame warmups. Changes are relative to Naga and use
unrounded values. Timing covers the entire application process, including
startup, presentation and teardown, rather than compilation
throughput or isolated GPU time. glTF animation time was fixed at 1, HDR data was
cached, and builds ran separately from measurements. Both implementations used
SDL offscreen rendering for GLES3 and X11 windows for Vulkan/WGPU; compare within
each backend. Short Triangle runs and presentation pacing limit sensitivity to
small differences. WGPU still uses Naga internally for native translation.

[Saved measurements, individual samples and artifact hashes](crates/platform/docs/benchmarks/shader-examples-2026-11-12.json).
See [shader qualification](docs/contracts/SHADER_COMPILATION.md#performance-qualification)
and [the profiling tool](scripts/README.md) for the current measurement workflow.
