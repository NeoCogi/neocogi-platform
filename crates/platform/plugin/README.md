# Native and browser platform provider

Windows, normalized input, clipboard, devices, drawing surfaces and selected GPU execution. The Rust library name is `nc_platform`.

Select exactly one backend. Default is `glow`; use `default-features = false` with `vulkan` or `wgpu` for alternatives. Browser builds use `webgl` on `wasm32-unknown-unknown`. Native builds need SDL2 development libraries, and Vulkan needs its loader. Acquire one Platform root and retain it for your application’s windows. The application owns its COM runtime and registers/acquires the shader compiler before this provider.

This is NeoCogi Platform `0.1.0-alpha.2`; authored interfaces remain version 1.
See the [repository documentation](https://github.com/NeoCogi/neocogi-platform/tree/master/docs)
for ownership, supported operations and deployment. The package LICENSE contains
the project license and bundled workspace license notices.
