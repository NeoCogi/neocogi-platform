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

Choose the same backend feature as the renderer plugin. All three backends
cache immutable vertex and index buffers by the shared allocation identity of
`MeshBuffers`, so repeated draws reuse geometry uploads while transforms remain
per draw. Cache entries retire when no scene or frame command retains their
source arrays. Vulkan also retains submitted frame references until the owning
frame-slot fence completes; WGPU command buffers retain their referenced GPU
handles after preparation advances. The platform validates finite geometry,
triangle indices, and transforms before publishing scene changes.

Glow/WebGL rectangle conversion computes origin-plus-extent in `i64`, clips those
mathematical edges to the logical frame, then scales outward with exact integer
division. Positive content rectangles may have far edges beyond `i32::MAX`;
a contained visible clip remains admissible. Only bounded pixel edges narrow.
A framebuffer extent that cannot fit GL's signed arguments reports a backend
error before viewport/scissor calls. Empty intersections produce no draw.

The current mesh backends still use a bounded content viewport. Preserving the
unclipped scene mapping independently of the scissor is the separate open F10
issue; the wide-arithmetic repair does not claim to resolve that projection rule.

The former demo-owned package names and contract identity are removed. Rebuild
applications and plugins together against the new mesh contract. No compatibility
aliases or parallel implementations are retained.
