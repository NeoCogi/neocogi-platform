# Native glTF examples

These examples run through the public platform, shader compiler and graphics COM
contracts. `gltf_damaged_helmet` displays a textured rigid mesh;
`gltf_brainstem` loops the model's first animation and skins its vertices on the GPU.
Both use the reference viewer's **Cannon Exterior HDR environment** for diffuse
lighting, roughness-dependent metal reflections and a blurred skybox. Exposure and
environment intensity default to one, with the reference +Z orientation and
Khronos PBR Neutral tone mapping.

```sh
cargo tools fetch-gltf-examples
cargo run --locked -p neocogi-platform-examples --bin gltf_damaged_helmet
cargo run --locked -p neocogi-platform-examples --bin gltf_brainstem
```

| Input | Action |
| --- | --- |
| Left drag | Arcball rotation about the initial model center |
| Right or middle drag | Pan in the camera plane |
| Mouse wheel | Zoom, bounded relative to the model size |
| R | Reset orientation, pan and fitted distance |
| Space | Pause/resume animation |
| B | Show/hide the skybox while retaining environment lighting |
| Escape or window close | Exit |

For shared-library loading use the [native assembly guide](../README.md). Both
models use the same scene/render loop in static and dynamic mode.

Both cameras initially fit the model's world-space bounds. Resize updates the
projection without resetting orientation or pan. Focus loss and resize end active
drags, so returning to a window cannot continue an old gesture. Each viewer stays
open by default; `--frames N` exits after a positive number of rendered frames.
`--time SECONDS` fixes the animation pose for repeatable inspection. `--asset PATH`
loads a local compatible asset, and `--help` lists options without creating a window.
`--environment PATH.hdr` selects another Radiance panorama. `--exposure MULTIPLIER`
sets a positive linear exposure factor (default 1); `--no-skybox` hides the
background while retaining HDR reflections and material illumination.

GL is the default. To select Vulkan or WGPU, add
`--no-default-features --features static,vulkan` or `static,wgpu` **before** the `--` separator:

```sh
cargo run --locked -p neocogi-platform-examples --bin gltf_brainstem --no-default-features --features static,vulkan -- --frames 3 --time 1
SDL_VIDEODRIVER=offscreen cargo run --locked -p neocogi-platform-examples --bin gltf_damaged_helmet -- --frames 3
```

Vulkan and WGPU need a window-system display; Linux CI uses Xvfb and Mesa drivers. The
examples are native; their browser targets have no viewer entry point. A browser
viewer would need asynchronous asset loading and a browser event-loop owner.

Fetch the models explicitly from the workspace root:

```sh
cargo tools fetch-gltf-examples
```

The script pins Khronos glTF-Sample-Assets revision
`8f492390ddc7a2ae221a1369ef560589cf8c70fb`, checks SHA-256 before installing each
GLB, and reuses verified local files. Downloads go into the ignored `assets/`
directory beside this document. Builds and ordinary tests do not access the network.
The same command also fetches the separately pinned Cannon Exterior HDR. The
first start prepares its lighting atlas; subsequent starts reuse a source-keyed
local cache. See [HDR data and cache contracts](ENVIRONMENT.md) for the environment
revision, license, numerical filtering and cache validation.

DamagedHelmet is by theblueturtle_ (original) and ctxwing (rebuild/conversion).
See its [upstream model](https://github.com/KhronosGroup/glTF-Sample-Assets/tree/8f492390ddc7a2ae221a1369ef560589cf8c70fb/Models/DamagedHelmet)
and [license notice](DamagedHelmet-LICENSE.md), which includes CC-BY-4.0 and
CC-BY-NC-4.0 terms. BrainStem is by Keith Hunter, © Smith Micro Software, Inc.
See its [upstream model](https://github.com/KhronosGroup/glTF-Sample-Assets/tree/8f492390ddc7a2ae221a1369ef560589cf8c70fb/Models/BrainStem),
[license notice](BrainStem-LICENSE.md), and [Poser EULA](LicenseRef-Poser-EULA.txt).
The downloaded artwork retains its own license; the workspace software license
does not relicense these assets.

`scene.rs` owns local import, scene hierarchy, materials, animation and skinning.
These semantics belong to the application; the renderer receives only typed
resources, explicit byte layouts and sealed commands. The selected default (or
first) scene supplies indexed triangle primitives with normals, UV0 and at most
four skin influences. Each skin has 1–64 joints. Weights are normalized on import;
out-of-range joints/indices and inconsistent attribute counts reject. Matrix nodes
keep their exact transforms; TRS animation targets decomposed nodes. The first
clip loops in seconds, supports LINEAR and STEP, and uses shortest-arc quaternion
interpolation. Unsupported topology, morphing, required extensions, nonopaque
materials, alternate UV sets and CUBICSPLINE animation fail with a diagnostic.
This is a sample viewer for the pinned core assets, not a complete glTF importer.

Joint palettes contain `joint_world * inverse_bind`; rigid primitives use the
mesh node's world transform. This world-space convention excludes the skinned
mesh node transform, avoiding its double application. Vertices are serialized
as position (12 bytes), normal (12), UV (8), u32 joints (16), f32 weights (16).
Matrices are explicitly encoded column-major, 64 bytes each; Rust struct layout
never defines a GPU payload.

The viewer owns the domain controller and immutable GPU buffers/textures for the
entire loop. Per-frame draws copy only camera, material and palette uniforms.
The shader applies inverse-transpose normals, base color, metallic/roughness,
derivative-based normal maps, occlusion and emissive maps. Materials retain full
image resolution. A missing emissive image contributes white, preserving an
authored emissive factor; an absent factor remains black and produces no emission.
An authored emissive image is decoded from sRGB and multiplied by its linear factor.
Materials use a four-tile atlas; occlusion occupies metallic/roughness's
unused red channel. Color texels are decoded from sRGB before interpolation;
data maps remain linear. HDR diffuse convolution and GGX prefiltering supply
illumination and reflections, with split-sum BRDF integration and compensation
for multiple scattering. Radiance remains HDR until exposure and PBR Neutral tone
mapping are applied once. The same atlas and orientation supply the skybox, which
follows arcball rotation and ignores camera pan/distance.

This supersedes the original directional-light/hemisphere approximation. The
portable renderer still receives only single-mip RGBA8 2D images: RGBE preserves
HDR radiance, with decoding before manual bilinear interpolation. Materials repeat
within each tile, preventing neighboring maps from bleeding into each other;
environment longitude repeats and polar rows clamp. The native sampler uses
nearest texel reads because interpolating RGBE exponent bytes is invalid. These
semantics stay inside the examples. glTF sampler variations, native mipmapping,
and exact pixel matching to the reference viewer's GPU integration remain outside
the sample contract; its 128-sample finite CPU convolutions have lower filtering
precision. No renderer material/mesh/environment API is introduced.

Offline import/animation tests and optional downloaded-asset acceptance:

```sh
cargo test --locked -p neocogi-platform-examples --test gltf_scene
cargo test --locked -p neocogi-platform-examples --test gltf_scene -- --include-ignored
cargo test --locked -p neocogi-platform-examples --test gltf_camera
cargo test --locked -p neocogi-platform-examples --test gltf_viewer --features test-support
cargo test --locked -p neocogi-platform-examples --test gltf_environment --test gltf_material
SDL_VIDEODRIVER=offscreen cargo test --locked -p neocogi-platform-examples --test gltf_viewer --features static,test-support -- --include-ignored --test-threads=1
```

The optional GL acceptance test checks visible pixels for both models, visible
BrainStem deformation, changed pixels after rotate/pan/zoom, and exact reset.
It also verifies HDR illumination independently of skybox visibility, exposure
changes, skybox rotation and invariance under pan/zoom. Numerical regressions
cover HDR encoding, convolution energy, BRDF bounds, material packing/AO and
source-keyed cache corruption/bounds handling.
An actual fragment probe checks that the midpoint of RGBE-encoded radiance 1 and
8 is 4.5, catching incorrect interpolation of the encoded exponent bytes.
Set `NEOCOGI_GLTF_CAPTURE=/tmp/gltf-captures` to save documentation PNGs from
the real GL renderer. Damaged Helmet uses a slightly elevated diagonal view;
BrainStem keeps the initial view. This capture camera does not change the
interactive reset view or the acceptance assertions:

```sh
NEOCOGI_GLTF_CAPTURE=/tmp/gltf-captures SDL_VIDEODRIVER=offscreen \
  cargo test --locked -p neocogi-platform-examples --no-default-features \
  --features static,glow,test-support --test gltf_viewer \
  models_render_animate_and_respond_to_arcball_pan_zoom -- --ignored --test-threads=1
```

CI also runs both standalone executables on GL, Vulkan and WGPU.

Numerical kernels belong to rs-math3d 0.21: scene TRS/animation uses typed vectors
and quaternions, bounds use its box and checked vector norm, and GPU matrices use
its explicit column-major scalar export before little-endian byte encoding.
The arcball uses shared dot/cross/basis/normalization operations. The library's
OpenGL perspective is adapted to the renderer's canonical [0,1] depth by changing
only the two Z coefficients. Near/far clipping, aspect and field of view have
numeric regressions.

Example policy remains local: glTF LINEAR/STEP timing and shortest-arc spherical
interpolation, arcball gestures, RGBE encoding, GGX integration and cache identity.
There is no rs-math3d spherical interpolation API to replace that policy; its
dot, quaternion arithmetic and normalization implement the numerical operations.
Likewise the renderer retains its validated wide clipping and exact outward pixel
rounding; ordinary rectangle intersection/fixed quantization has different empty
and rounding semantics. No speedup for these consumers is claimed without a
matching workload measurement.
