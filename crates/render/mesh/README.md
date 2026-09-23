# Fixed-material mesh rendering

`neocogi-render-mesh` supplies the renderer plugin's reusable mesh scene backend.
`neocogi-render-mesh-contracts` defines the `neocogi.render.mesh` ABI, including
`IMeshScenes`, `IMeshScene`, vertices, and scene transforms. Applications query
`IMeshScenes` from the platform window, create a scene, and retain it as an ordinary
`IRenderItem`. The full and smoke demos are consumers of this capability.

The implementation owns the existing simple directional/procedural material,
depth-tested mesh pipelines, immutable geometry, and backend-specific recording.
This is a fixed material capability, not an arbitrary shader or material system.
Glow/WebGL, Vulkan, and WGPU each preserve native resource ownership inside their
backend. Rust command values and GPU handles never cross the public COM boundary.

Choose the same backend feature as the renderer plugin. Vulkan retains immutable
mesh uploads across draws and frame slots, retiring geometry only after its owning
frames complete. The platform validates finite geometry, triangle indices, and
transforms before publishing scene changes.

The former demo-owned package names and contract identity are removed. Rebuild
applications and plugins together against the new mesh contract. No compatibility
aliases or parallel implementations are retained.
