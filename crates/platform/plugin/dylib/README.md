# Platform shared library

Final native loader entrypoint for the selected platform/backend provider. Produces the `nc_platform` shared library.

Default backend is `glow`. Disable defaults when choosing `vulkan` or `wgpu`; exactly one backend is allowed. The loading application owns its COM host and loads the compatible shader compiler first. Native builds require SDL2 development libraries and, for Vulkan, its loader.

This is NeoCogi Platform `0.1.0-alpha.1`; authored interfaces remain version 1.
See the [repository documentation](https://github.com/NeoCogi/neocogi-platform/tree/master/docs)
for ownership, supported operations and deployment. The package LICENSE contains
the project license and bundled workspace license notices.
