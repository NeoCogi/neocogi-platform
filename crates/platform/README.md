# NeoCogi Platform renderer SDK

Typed GPU recording/resources, optional concrete backends, renderer-owned shader
compilation, PCM playback/capture and SDL/browser hosting. Canonical schemas and generated contracts
live in `neocogi-platform-contracts`; `render` reexports those exact contract types
and adds the optional `render::compiler` implementation. Build scripts depend on
the contracts crate with `build-support`, not this implementation crate.

SDK defaults are empty. The `audio` feature enables SDL/Web Audio without a
renderer, window or shader compiler. `platform` includes audio; its root exposes
typed capability queries and asynchronous playback/capture opening. PCM types
carry their width/scale as Rust enum payloads. Backend features `glow`, `vulkan`, `wgpu` and `webgl`
enable their required compiler output. `platform` requires one selected hosting
backend; GPU-only Rust consumers may combine native backends. Static applications
link this SDK and register only the platform descriptor. Dynamic clients use the
contracts crate and load the matching platform library from `neocogi-platform-dylib`.

Compiler output is fixed at device construction. Requests contain stage and output
budget only; results identify their target and export one `code()` byte buffer:
UTF-8 for WGSL/GLSL or little-endian words for SPIR-V. Import validates target,
encoding and bounds before retaining one typed `ShaderCode`. Alpha schema versions
remain 1. CPU-only tools may enable `shader-compiler` and output features directly.
