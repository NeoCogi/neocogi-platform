# Portable shader compiler provider

CPU-only WGSL validation, guarded SPIR-V and GLSL emission through the NeoCogi shader COM interfaces. The Rust library name is `nc_shader_compiler`.

The application owns the COM runtime. Linked compositions register `plugin_descriptor()`; shared-library compositions use `neocogi-shader-compiler-dylib`. Native GPU execution belongs to the platform provider.

This is NeoCogi Platform `0.1.0-alpha.1`; authored interfaces remain version 1.
See the [repository documentation](https://github.com/NeoCogi/neocogi-platform/tree/master/docs)
for ownership, supported operations and deployment. The package LICENSE contains
the project license and bundled workspace license notices.
