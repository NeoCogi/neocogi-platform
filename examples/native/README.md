# Native example assembly

The native applications are triangle, gltf_damaged_helmet and gltf_brainstem.
They share an example-owned typed platform bootstrap and the same graphics COM
render loop in both assembly modes. Native platform-root/session behavior is
unchanged; each application acquires and retains one root.

Default features select static registration and Glow:

~~~sh
python3 scripts/fetch_gltf_examples.py
cargo run --locked -p neocogi-platform-examples --bin triangle -- 3
cargo run --locked -p neocogi-platform-examples --bin gltf_damaged_helmet
cargo run --locked -p neocogi-platform-examples --bin gltf_brainstem
~~~

Select exactly one assembly and one backend. To load shared objects:

~~~sh
cargo run --locked -p neocogi-platform-examples --bin triangle --no-default-features --features dynamic,glow -- 3
cargo run --locked -p neocogi-platform-examples --bin gltf_damaged_helmet --no-default-features --features dynamic,vulkan -- --frames 3
cargo run --locked -p neocogi-platform-examples --bin gltf_brainstem --no-default-features --features dynamic,wgpu -- --frames 3 --time 1
~~~

Use static,vulkan or static,wgpu for linked alternatives. Cargo features select
the backend at build time; this does not add runtime renderer switching.
SDL offscreen works for Glow. Vulkan/WGPU require a native display/adapter.

## Ownership and deployment

Static assembly registers linked compiler then platform descriptors and acquires
the typed root through the SDK. Dynamic assembly builds ordinary final cdylib
artifacts through neocogi-example-plugins. It stages their exact filenames,
loads the compiler first, loads the platform, then acquires its typed root.
Only final dylib roots export discovery; normal provider implementations are not
Rust dependencies of a dynamic executable. There is no linked compiler fallback.

The application owns its one process-lived SDK host in both modes. Plugin code
and metadata remain mapped under the SDK's existing loader policy. Missing
libraries/dependencies fail through that policy; wrong compiler roots reject
before native device construction. No new renderer ABI or native-handle exchange
is introduced.

Run build with the same mode/backend as run. For a debug deployment, copy the
selected executable from target/debug together with lib/ and assets/gltf/ into
a fresh application directory. Release/min-size use their corresponding output
profile. Executables load lib beside themselves and locate artwork in
assets/gltf, independent of CWD or checkout paths. Keep the matching selected
libraries together when building several backend variants into one Cargo profile.

The explicit asset download remains separate from compilation. When all pinned
models/HDR are present, build stages their files and checked-in license notices
as exclusive file slots. Ordinary builds without artwork still compile; viewers
then diagnose missing runtime assets. The sibling source-keyed IBL cache is runtime
data and is not a build-owned slot, so rebuilding does not prune it. No downloads
or lighting bake occur in build scripts.

See [glTF controls, rendering and asset terms](gltf/README.md) and
[native staging contracts](../../docs/contracts/NATIVE_LAYOUT.md).

## Verification

~~~sh
cargo test --locked -p neocogi-platform-examples
cargo test --locked -p neocogi-platform-examples --no-default-features --features dynamic,glow --test graphs --test deployment
SDL_VIDEODRIVER=offscreen cargo test --locked -p neocogi-platform-examples --test gltf_viewer --features test-support -- --include-ignored --test-threads=1
SDL_VIDEODRIVER=offscreen cargo test --locked -p neocogi-platform-examples --no-default-features --features dynamic,glow --test deployment -- --include-ignored --test-threads=1
~~~

Use dynamic,vulkan or dynamic,wgpu with a display for the last command.
Ordinary deployment tests copy and rename actual executables/libraries, remove
Cargo loader paths, and acquire through COM without a GPU. Isolated children
exercise missing libraries, missing compiler dependencies and incompatible roots.
Opt-in deployment acceptance renders all three real examples after relocation.
Static GL pixel acceptance retains animation, arcball, material/HDR and skybox
checks. Dependency regressions use the selected Cargo tree and verify artifact-only
edges; metadata alone may contain inactive weak-feature edges.
