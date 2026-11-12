# NeoCogi Platform shared-library shim

One normal Cargo `cdylib` target named `nc_platform`. Select exactly one backend
with `glow` (the default), `vulkan`, or `wgpu`. Each build includes its renderer,
built-in S-expression COM shader compiler, and audio services, and exports only
`nc_get_plugin_descriptor_v1`. The application supplies the process-lived COM host.

```sh
cargo build --profile min-size -p neocogi-platform-dylib \
  --no-default-features --features vulkan
```

This produces `target/min-size/libnc_platform.so` on Linux,
`libnc_platform.dylib` on macOS, or `nc_platform.dll` on Windows. Backend selection
changes the library's implementation and descriptor, never its filename. No
example target, separate shader feature, or compiler library is needed.

## Using the published crate

For static Rust integration, depend on `neocogi-platform`. Dynamic applications
depend on `neocogi-platform-contracts` and load this final library through COM.
The shim is a deployment artifact, not a Rust API to import or link into the host.

Declare the shim in a consumer's `Cargo.toml` so Cargo can resolve and build it
from crates.io. This can be a separate deployment-build package when the
application's build graph must contain only the contracts:

```toml
[dependencies]
neocogi-platform-dylib = { version = "=0.2.0", default-features = false, features = ["vulkan"] }
```

Then build the selected package's normal library and deploy the result:

```sh
cargo build --release -p neocogi-platform-dylib --lib
# Copy target/release/libnc_platform.so beside your application's other deployed libraries.
```

The consumer chooses its own release profile and toolchain. Native builds require
SDL2 development libraries and the selected backend's loader/driver; Vulkan also
needs its loader. No checkout-relative paths, nightly artifact dependencies, or
Cargo invocation from a build script are required. A cdylib-only package provides
no Rust-linkable target; Cargo may warn about that dependency. The
host uses the resulting library through the COM loader rather than a Rust import.

## Repository examples and size qualification

Build the shim first, then the dynamic application using the same profile and
target. Its build script stages the normal library from `target/<profile>/` into
`target/<profile>/lib/`. Runtime loading uses that one file and validates its
platform descriptor. Optional `--backend NAME` verifies the compiled backend;
changing it requires rebuilding the library.

The complete Vulkan platform bundle is qualified below 524,288 bytes using the
repository's rolling nightly, rebuilt std, `min-size`, and immediate-abort panics.
See [shader qualification](../../docs/contracts/SHADER_COMPILATION.md).
