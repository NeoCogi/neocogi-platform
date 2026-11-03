# NeoCogi Platform

A Rust workspace for native/browser platform services, portable GPU resources,
scoped graphics commands, shader compilation and backend execution. Public APIs
use typed COM interfaces. Applications can use graphics directly or combine it
with [NeoCogi UI](https://github.com/NeoCogi/neocogi-ui).

## Workspace

| Package family | Responsibility |
| --- | --- |
| `crates/render/core` | Renderer/platform contracts, upload producers and packaged authored NCIDL sources |
| `crates/render/gpu` | Typed resources, provenance, bounded command recording and lifetime accounting |
| `crates/render/support` | Shared frame preparation and platform algorithms |
| `crates/render/glow`, `webgl`, `vulkan`, `wgpu` | Native resource realization and graphics execution |
| `crates/render/plugin` | Selected platform/backend assembly; windows, input, clipboard and surfaces |
| `crates/shader/core`, `plugin` | Portable compiler contracts, reflection and Naga-backed compiler provider |
| `tools/native-layout` | Compile-time staging of Cargo artifacts and application resources |

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

```sh
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
cargo run --locked -p neocogi-render-plugin --example triangle -- 3
```

For headless native GL qualification:

```sh
SDL_VIDEODRIVER=offscreen cargo run --locked -p neocogi-render-plugin --example triangle -- 3
SDL_VIDEODRIVER=offscreen cargo test --locked -p neocogi-render-plugin --lib -- --include-ignored --test-threads=1
```

Select exactly one backend. Cargo adds feature selections to defaults, so use
`--no-default-features --features vulkan` or `wgpu` for another native backend.
Browser consumers select `webgl` and `wasm32-unknown-unknown`. Dynamic deployments
use the final renderer and compiler `*-dylib` packages; static applications use
their ordinary Rust implementations. The application owns the COM host and
compiler/profile selection for the complete build graph.

The native glTF examples include a textured DamagedHelmet and animated GPU-skinned
BrainStem, both with arcball rotation, pan and zoom:

```sh
python3 scripts/fetch_gltf_examples.py
cargo run --locked -p neocogi-render-plugin --example gltf_damaged_helmet
cargo run --locked -p neocogi-render-plugin --example gltf_brainstem
```

Left-drag rotates, right/middle-drag pans, and the wheel zooms. Press R to reset,
Space to pause animation, or Escape to close. See the [example guide](crates/render/plugin/examples/gltf/README.md)
for backend selection, bounded headless runs, supported glTF features and asset licenses.

Both companion workspaces pin the published COM, host and IDL packages to exact
`0.1.0-alpha.1`. Shared dependencies participate in one application Cargo graph.
Independent builds can reuse compatible artifacts through `CARGO_TARGET_DIR`.

See [documentation](docs/README.md), [component ownership](docs/architecture/COMPONENTS.md),
[GPU contracts](docs/contracts/GPU_RENDERING.md), [shader compilation](docs/contracts/SHADER_COMPILATION.md),
and [companion revisions](docs/project/HISTORY.md).
