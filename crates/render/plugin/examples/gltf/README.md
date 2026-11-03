# Native glTF examples

These examples run through the public platform, shader compiler and graphics COM
contracts. `gltf_damaged_helmet` displays a textured rigid mesh;
`gltf_brainstem` loops the model's first animation and skins its vertices on the GPU.

```sh
python3 scripts/fetch_gltf_examples.py
cargo run --locked -p neocogi-render-plugin --example gltf_damaged_helmet
cargo run --locked -p neocogi-render-plugin --example gltf_brainstem
```

| Input | Action |
| --- | --- |
| Left drag | Arcball rotation about the initial model center |
| Right or middle drag | Pan in the camera plane |
| Mouse wheel | Zoom, bounded relative to the model size |
| R | Reset orientation, pan and fitted distance |
| Space | Pause/resume animation |
| Escape or window close | Exit |

Both cameras initially fit the model's world-space bounds. Resize updates the
projection without resetting orientation or pan. Focus loss and resize end active
drags, so returning to a window cannot continue an old gesture. Each viewer stays
open by default; `--frames N` exits after a positive number of rendered frames.
`--time SECONDS` fixes the animation pose for repeatable inspection. `--asset PATH`
loads a local compatible asset, and `--help` lists options without creating a window.

GL is the default. To select Vulkan or WGPU, add
`--no-default-features --features vulkan` or `wgpu` **before** the `--` separator:

```sh
cargo run --locked -p neocogi-render-plugin --example gltf_brainstem --no-default-features --features vulkan -- --frames 3 --time 1
SDL_VIDEODRIVER=offscreen cargo run --locked -p neocogi-render-plugin --example gltf_damaged_helmet -- --frames 3
```

Vulkan and WGPU need a window-system display; Linux CI uses Xvfb and Mesa drivers. The
examples are native; their browser targets have no viewer entry point. A browser
viewer would need asynchronous asset loading and a browser event-loop owner.

Fetch the models explicitly from the workspace root:

```sh
python3 scripts/fetch_gltf_examples.py
```

The script pins Khronos glTF-Sample-Assets revision
`8f492390ddc7a2ae221a1369ef560589cf8c70fb`, checks SHA-256 before installing each
GLB, and reuses verified local files. Downloads go into the ignored `assets/`
directory beside this document. Builds and ordinary tests do not access the network.

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
derivative-based normal maps and emissive maps. Color maps use sRGB decoding;
data maps remain linear. GGX directional lights and hemisphere fill provide an
approximate environment; image-based lighting, occlusion maps and glTF sampler
variations are not implemented. All sampled images use linear/repeat filtering
and one mip, within the renderer's portable profile. These are documented sample
constraints; no renderer material/mesh API is introduced.

Offline import/animation tests and optional downloaded-asset acceptance:

```sh
cargo test --locked -p neocogi-render-plugin --test gltf_scene
cargo test --locked -p neocogi-render-plugin --test gltf_scene -- --include-ignored
cargo test --locked -p neocogi-render-plugin --test gltf_camera --test gltf_viewer
SDL_VIDEODRIVER=offscreen cargo test --locked -p neocogi-render-plugin --test gltf_viewer --features test-support -- --include-ignored --test-threads=1
```

The optional GL acceptance test checks visible pixels for both models, visible
BrainStem deformation, changed pixels after rotate/pan/zoom, and exact reset.
Set `NEOCOGI_GLTF_CAPTURE=/tmp/gltf-captures` to retain initial-view PPM images.
CI also runs both standalone executables on GL, Vulkan and WGPU.
