# Fixed-material mesh rendering

`neocogi-render-mesh` supplies the renderer plugin's reusable mesh scene backend.
`neocogi-render-mesh` defines the `neocogi.render.mesh` ABI, including
`IMeshScenes`, `IMeshScene`, vertices, and scene transforms. Applications query
`IMeshScenes` from the platform window, create a scene, and retain it as an ordinary
`IRenderItem`. The full and smoke demos are consumers of this capability.

The implementation owns the existing simple directional/procedural material,
depth-tested mesh pipelines, immutable geometry, and backend-specific recording.
This is a fixed material capability, not an arbitrary shader or material system.
Glow/WebGL, Vulkan, and WGPU each preserve native resource ownership inside their
backend. Rust command values and GPU handles never cross the public COM boundary.

`MeshVertex` describes semantic application fields, with no native layout promise.
COM transport uses its checked wire format; GPU uploads use the private
[renderer-owned vertex format](src/vertex.rs). Each new upload converts named
position/normal/UV fields into initialized float arrays. All three backends derive
attribute offsets and stride from that format. The temporary encoding is dropped
after upload; the original shared mesh arrays still own cache identity. A COM
application-layout change therefore cannot change what the shader reads.

Glow/WebGL uploads index data as `ELEMENT_ARRAY_BUFFER` under the mesh renderer's
private vertex array. WebGL assigns the buffer's vertex/index role on its first
binding, so index uploads must never use `ARRAY_BUFFER`. The upload preserves
the UI renderer's independent vertex-array bindings.

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

## Visible client area

Mesh scenes fit their complete projection into the final visible client area.
`RenderItemArgs.view` drives both viewport and scissor through one backend
conversion. `content_area` remains the full allocation for layout and generic
render-item consumers; it does not determine mesh raster mapping. Moving a
client partly offscreen, or clipping it inside a parent, therefore refits the
whole scene into the remaining rectangle. An empty visible area emits no draw.

The application owns camera projection. It must use the visible width/height
for aspect and subtract the visible local origin when normalizing pointer input.
The full demo's mesh and 3D grid do this during input and again during paint,
after layout settles. Paint may edit scene transforms because those edits do
not mutate UI geometry. Captured pointers may remain outside the visible area.
Framebuffer scaling and backend Y conventions belong to the renderer.

This is the user-selected F10 policy. The original review's crop-of-full-scene
acceptance criterion was superseded; ordinary widget layout is unchanged.

## Independent depth per item

Each painter-ordered mesh item starts with depth 1.0, retaining previously
painted color. A farther scene painted later therefore appears above an earlier
independent scene, including after an intervening UI background. Triangles
inside one item still share depth and occlude one another. Inserting the same
scene twice also creates two independent item scopes; depth belongs to a draw
position, not to the scene object or the whole frame.

Glow/WebGL clears depth inside the item's scissor. Vulkan clears only the depth
attachment inside the same visible rectangle before its indexed draw. WGPU
starts a new pass before each additional application item, loads prior color,
and clears the existing depth target. Leading UI and the first item share the
initial pass; following UI stays with that item until the next boundary. Frames
with zero or one application item still use one pass. Depth is discarded at
pass completion because later items and frames never consume it. The repair
adds pass boundaries for multiple items and allocates no per-item depth images.

Application preparation remains once per frame, command indices span all
passes, and UI draws restore their complete pipeline state. WGPU recording
errors abandon the encoder before submission; Vulkan's existing acquired-frame
failure policy remains unchanged. Public offscreen UI surfaces stay depth-free
and reject application mesh commands.

The former demo-owned package names and contract identity are removed. Rebuild
applications and plugins together against the new mesh contract. No compatibility
aliases or parallel implementations are retained.

## Native pixel regressions

The ordinary unit suite checks exact position/normal/UV bytes across multiple
vertices and empty input, independently of the compiler's chosen application
layout. GPU tests execute the production upload and shader paths and read actual
framebuffers. They require native drivers and are explicitly enabled by
`scripts/test-demo.sh <glow|vulkan|wgpu>`, which is run by each native CI job.
The GL fixture also checks that repeated draws retain the original GPU buffer.

For a focused offscreen GL run:

```sh
SDL_VIDEODRIVER=offscreen cargo test -p neocogi-render-mesh \
  --no-default-features --features glow --lib -- --include-ignored --test-threads=1
```

Use `--features vulkan` or `--features wgpu` for the other native framebuffer
fixtures. A normal unit run leaves these driver-dependent tests ignored.
The [pixel fixtures](src/pixel_tests.rs) exercise the production upload and shader
layout; normal unit tests also check named-field GPU conversion and cache identity.
