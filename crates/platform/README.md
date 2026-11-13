# NeoCogi Platform renderer SDK

Typed GPU recording/resources, optional concrete backends, a built-in dual-frontend COM
compiler, PCM playback/capture and SDL/browser hosting. Canonical schemas and generated contracts
live in `neocogi-platform-contracts`; `render` reexports those exact contract types
and adds the optional `render::compiler` implementation. Build scripts depend on
the contracts crate with `build-support`, not this implementation crate.

SDK defaults are empty. The `audio` feature enables SDL/Web Audio without a
renderer, window or shader compiler. `platform` includes audio; its root exposes
typed capability queries and asynchronous playback/capture opening. PCM types
carry their width/scale as Rust enum payloads. Backend features `glow`, `vulkan`, `wgpu` and `webgl`
enable their required compiler output. `platform` requires one selected hosting
backend; GPU-only Rust consumers may combine native backends. Static applications
link this SDK and register the platform descriptor. Dynamic clients use the
contracts crate and load the matching platform library from `neocogi-platform-dylib`.
Its built-in `ShaderCompilerFactory` is queried from that same COM root.

Compiler output is fixed at device construction. Requests contain stage and output
budget only; results identify their target and export one `code()` byte buffer:
UTF-8 for WGSL/GLSL or little-endian words for SPIR-V. Import validates target,
encoding and bounds before retaining one typed `ShaderCode`. Alpha schema versions
remain 1. CPU-only tools may enable `shader-compiler` and output features directly.

The compiler accepts S-expressions and a Python-like shader syntax, validates the
entire module through one typed IR, then directly
emits WGSL, GLSL ES 3.00, or SPIR-V with shared typed reflection and explicit bounds
checks. Shared lowering reads aggregate leaves directly and expands small aggregate
helpers within a fixed code-growth budget. Mutable snapshots keep value semantics.
Parse/type/profile failures carry line/column diagnostics. The COM
service is `ShaderCompilerFactory`; it creates immutable target-specific compiler
instances. See [the shader contract](../../docs/contracts/SHADER_COMPILATION.md)
for grammar, limits, error/lifetime semantics, validation, and the <512 KiB
min-size Vulkan platform bundle qualification.

The Python-like frontend uses typed `def` functions, `@vertex`/`@fragment`, `class`
records, indentation, ordinary expressions and fixed array literals. Both parsers
retain original line/column diagnostics. No Python runtime or extra parser
dependency is needed. See [triangle.pysh](examples/triangle.pysh) for a complete
program; existing S-expression input remains supported by the same COM service.
