# Shader compiler shared library

Final native loader entrypoint for the CPU shader compiler. Produces the `nc_shader_compiler` shared library.

Build this package to deploy a shared shader provider. Applications loading it own their COM host and must stage this library beside the selected platform library.

This is NeoCogi Platform `0.1.0-alpha.1`; authored interfaces remain version 1.
See the [repository documentation](https://github.com/NeoCogi/neocogi-platform/tree/master/docs)
for ownership, supported operations and deployment. The package LICENSE contains
the project license and bundled workspace license notices.
