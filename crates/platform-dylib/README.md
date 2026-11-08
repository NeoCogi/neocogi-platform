# NeoCogi Platform shared-library shims

Thin final COM exports over the reusable `neocogi-platform` Rust library.
Each backend has two explicitly named Cargo library examples: `nc_platform_{backend}`
and `nc_shader_compiler_{backend}`. Each exports one independent descriptor.
Cargo represents these final link roots as library examples because a package
supports only one main library target. The main `nc_platform` library remains a
platform export for consumers that build the library directly.

Build the selected pair with ordinary Cargo:

```sh
cargo build -p neocogi-platform-dylib --no-default-features --features glow --examples
```

Use vulkan or wgpu instead of glow for other backends. `required-features` selects
only the matching pair; the filenames already contain the backend suffix. The
native application's Rust build script stages these exact named artifacts from
`target/<profile>/examples/` into `target/<profile>/lib/`. Build and run with the
same profile/target. No custom command or external builder is needed. Load the
compiler before the platform. All implementations are statically linked into
these final shared libraries; ordinary Rust applications link the SDK directly.

Each compiler artifact contains only the output support required by its selected
backend. Both filenames include that backend, so building another variant does
not replace a matching compiler beside a different platform variant. The GLSL
build retains its internal guarded SPIR-V roundtrip without exporting it.
