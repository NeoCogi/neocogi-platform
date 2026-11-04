# GPU recording and optional graphics backends

Persistent GPU buffers, quotas, immutable draw captures, scoped recording and concrete backend execution. The default feature set is CPU-only.

Features `glow`, `vulkan`, `wgpu` and `webgl` expose backend implementations. Native backend features are additive here. Native execution needs SDL2 development libraries; Vulkan also requires its loader. Browser consumers select `webgl` for `wasm32-unknown-unknown`.

This is NeoCogi Platform `0.1.0-alpha.2`; authored interfaces remain version 1.
See the [repository documentation](https://github.com/NeoCogi/neocogi-platform/tree/master/docs)
for ownership, supported operations and deployment. The package LICENSE contains
the project license and bundled workspace license notices.

Buffer creation selects static initial data, preserving dynamic writes or bounded streamed writes. Vertex, Index, Uniform and Texture share `IGpuBuffer`; application typing lives in `neocogi_render_core::buffer`. See [the coordinated alpha migration](https://github.com/NeoCogi/neocogi-platform/blob/master/docs/GPU_BUFFER_MIGRATION.md).
