# NeoCogi Platform

A Rust workspace for native/browser platform services, portable GPU resources,
scoped graphics commands, shader compilation and backend execution. Public APIs
use typed COM interfaces. Applications can use graphics directly or combine it
with [NeoCogi UI](https://github.com/NeoCogi/neocogi-ui).

## Workspace

| Package family | Responsibility |
| --- | --- |
| `crates/render/core` | Renderer/platform bindings and upload producers |
| `crates/render/gpu` | Typed resources, command recording, lifetime accounting and optional graphics backends |
| `crates/render/support` | Shared frame preparation and platform algorithms |
| `crates/platform/plugin` | Selected platform/backend assembly; windows, input, clipboard and surfaces |
| `crates/shader/core`, `plugin` | Portable compiler contracts, reflection and Naga-backed compiler provider |
| `examples/native` | Triangle and glTF applications with static/dynamic plugin assembly |
| `tools/platform-build` | Packaged authoritative interfaces and build-only generation helper |
| `tools/native-layout` | Build-only output policy and compile-time staging of Cargo artifacts and exact resource snapshots |

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
cargo test --locked --workspace
cargo run --locked -p neocogi-platform-examples --bin triangle -- 3
```

For headless native GL qualification:

```sh
SDL_VIDEODRIVER=offscreen cargo run --locked -p neocogi-platform-examples --bin triangle -- 3
SDL_VIDEODRIVER=offscreen cargo test --locked -p neocogi-platform-plugin --lib -- --include-ignored --test-threads=1
```

Select exactly one backend. Cargo adds feature selections to defaults, so use
`--no-default-features --features vulkan` or `wgpu` for another native provider.
Native examples select their assembly too: `static,vulkan` or `dynamic,vulkan`.
Browser consumers select `webgl` and `wasm32-unknown-unknown`. Dynamic deployments
use the final renderer and compiler `*-dylib` packages; static applications use
their ordinary Rust implementations. The application owns the COM host and
compiler/profile selection for the complete build graph.

The GPU package has empty default features. Its `glow`, `vulkan`, `wgpu` and
`webgl` features expose implementations under `neocogi_render_gpu::backends`;
native features are additive there. The platform assembly selects exactly one
backend. CPU-only recording and validation need no SDL or graphics dependencies.

The native glTF examples include a textured DamagedHelmet and animated GPU-skinned
BrainStem, both with arcball rotation, pan and zoom.
Both use Cannon Exterior HDR lighting, filtered reflections and a skybox.

| Damaged Helmet | BrainStem |
| :---: | :---: |
| <img src="docs/images/damaged-helmet.png" alt="Damaged Helmet rendered with textured metallic materials and the Cannon Exterior skybox" width="440"> | <img src="docs/images/brainstem.png" alt="BrainStem rendered with GPU skinning and the Cannon Exterior skybox" width="440"> |

Captured from the examples using the Glow backend. Damaged Helmet is by
theblueturtle_ and ctxwing ([license](examples/native/gltf/DamagedHelmet-LICENSE.md));
BrainStem is by Keith Hunter, © Smith Micro Software, Inc.
([license](examples/native/gltf/BrainStem-LICENSE.md)). The Cannon Exterior environment
is by Greg Zaal ([CC0](examples/native/gltf/Cannon_Exterior-LICENSE.md)).

```sh
python3 scripts/fetch_gltf_examples.py
cargo run --locked -p neocogi-platform-examples --bin gltf_damaged_helmet
cargo run --locked -p neocogi-platform-examples --bin gltf_brainstem
```

Load shared shader/platform objects in the examples with:

~~~sh
cargo run --locked -p neocogi-platform-examples --bin gltf_brainstem --no-default-features --features dynamic,glow
~~~

Cargo builds and stages the selected libraries; startup loads the compiler before
the platform. See [native assembly and relocation](examples/native/README.md).

Left-drag rotates, right/middle-drag pans, and the wheel zooms. Press R to reset,
Space to pause animation, B to toggle the skybox, or Escape to close. See the [example guide](examples/native/gltf/README.md)
for backend selection, bounded headless runs, supported glTF features and asset licenses.

The Platform and UI workspaces currently pin the published COM, host and IDL
packages to exact `0.1.0`. Shared dependencies participate in one application Cargo graph.
Independent builds can reuse compatible artifacts through `CARGO_TARGET_DIR`.

See [documentation](docs/README.md), [component ownership](docs/architecture/COMPONENTS.md),
[GPU contracts](docs/contracts/GPU_RENDERING.md), [shader compilation](docs/contracts/SHADER_COMPILATION.md),
and [dependency compatibility and revision history](docs/project/HISTORY.md).

The current release target is `0.1.0-alpha.2`. See [release preparation](docs/project/RELEASING.md)
for the published package set, self-contained build inputs and verification.
