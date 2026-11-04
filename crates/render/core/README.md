# Renderer and platform contracts

Backend-neutral generated COM bindings, window/input contracts, image validation and owned upload producers. This package has no plugin entrypoint.

CPU-only use needs no native graphics libraries. Build-time binding generation uses `neocogi-platform-build` as a declared build dependency.

This is NeoCogi Platform `0.1.0-alpha.2`; authored interfaces remain version 1.
See the [repository documentation](https://github.com/NeoCogi/neocogi-platform/tree/master/docs)
for ownership, supported operations and deployment. The package LICENSE contains
the project license and bundled workspace license notices.

The `buffer` module provides `TypedGpuBufferFactory` over `ComPtr<IRenderResourcesAbi>` and `TypedGpuBuffer<T>` over the original driver-created `IGpuBuffer`. Application vertex types and factories stay above that untyped contract.
