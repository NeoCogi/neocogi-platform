# glTF example assets and scene contract

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

Offline import/animation tests and optional downloaded-asset acceptance:

```sh
cargo test --locked -p neocogi-render-plugin --test gltf_scene
cargo test --locked -p neocogi-render-plugin --test gltf_scene -- --include-ignored
```
