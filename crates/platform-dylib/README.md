# NeoCogi Platform shared-library shims

Thin final COM exports over the reusable `neocogi-platform` Rust library.
Each backend has one explicitly named Cargo library example:
`nc_platform_{backend}`. It includes its renderer and shader compiler and exports
one platform descriptor. No compiler plugin or separate compiler library is needed.
The main `nc_platform` library also exposes the selected platform for consumers
that build the library directly; named targets provide backend-specific filenames.

```sh
cargo build -p neocogi-platform-dylib --no-default-features --features vulkan --examples
```

Use glow or wgpu instead of vulkan for other backends. Required features select
only the matching platform target. The native application's Rust build script
stages that exact named output from `target/<profile>/examples/` into
`target/<profile>/lib/`. Build and run with the same profile/target. Ordinary Rust
applications link the SDK directly and register only the platform descriptor.

Each renderer constructs a compiler fixed to its required shader format. Requests
contain no target; successful results identify their format for backend validation.
The built-in compiler parses and validates S-expressions and directly emits the
selected WGSL, GLSL ES300, or SPIR-V artifact. The platform root exposes the
`ShaderCompilerFactory` COM interface through QueryInterface. The shader service
has no separate feature, descriptor, registration, or shared library.


The complete Vulkan platform bundle is qualified against the 524,288-byte goal
with nightly, rebuilt std, and immediate-abort panics:

```sh
cargo build --profile min-size -p neocogi-platform-dylib --no-default-features --features vulkan --examples
```

Deploy only `libnc_platform_vulkan.so` on Linux (the corresponding platform
filename on macOS/Windows). See [shader qualification](../../docs/contracts/SHADER_COMPILATION.md).
