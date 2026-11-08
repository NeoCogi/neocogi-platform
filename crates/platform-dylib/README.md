# NeoCogi Platform shared-library shims

Thin final COM exports over the reusable `neocogi-platform` Rust library.
The main `cdylib` target exports the selected platform descriptor. A second
`cdylib` library target, `--example nc_shader_compiler`, exports the independent
CPU compiler descriptor. Cargo represents the second link root as a library
example because a package supports only one main library target.

Build both with `cargo build -p neocogi-platform-dylib --lib --example
nc_shader_compiler --no-default-features --features glow` (or vulkan/wgpu).
The repository's `python3 scripts/build_plugins.py --backend glow` selects exact
Cargo-reported outputs and stages matching backend-suffixed compiler and platform
library into the profile's `lib` directory. Load the compiler before the platform.
All implementations are statically linked into these final shared libraries;
ordinary Rust applications link the SDK directly instead.

Each compiler artifact contains only the output support required by its selected
backend. Both deployed filenames include that backend, so staging another variant
does not replace a matching compiler already beside a different platform variant.
The GLSL build retains its internal guarded SPIR-V roundtrip without exporting it.
