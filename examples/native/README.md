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
cargo build --locked -p neocogi-platform-dylib --no-default-features --features glow --examples
cargo run --locked -p neocogi-platform-examples --bin triangle --no-default-features --features dynamic,glow -- 3
cargo build --locked -p neocogi-platform-dylib --no-default-features --features vulkan --examples
cargo run --locked -p neocogi-platform-examples --bin gltf_damaged_helmet --no-default-features --features dynamic,vulkan -- --frames 3
cargo build --locked -p neocogi-platform-dylib --no-default-features --features wgpu --examples
cargo run --locked -p neocogi-platform-examples --bin gltf_brainstem --no-default-features --features dynamic,wgpu -- --frames 3 --time 1
~~~

For a min-size dynamic build, prepare the same profile before running:

~~~sh
cargo build --profile min-size -p neocogi-platform-dylib --no-default-features --features vulkan --examples
cargo run --profile min-size -p neocogi-platform-examples --bin gltf_brainstem --no-default-features --features vulkan,dynamic
~~~

Both commands must use the same profile and target. The first builds the selected
backend's named Cargo platform library example; the second automatically stages it
from that profile's `examples/` directory into `lib/`. Only Cargo and Rust are
needed for building and staging libraries.
Startup checks the platform library and returns an owned error with
the matching preparation command when a file is missing. This check runs before
model import and HDR lighting preparation. It never starts Cargo from a build script
or application. Existing library validation still follows the SDK loader policy.

Use static,vulkan or static,wgpu for linked alternatives. Cargo features select
the backend at build time; this does not add runtime renderer switching.
SDL offscreen works for Glow. Vulkan/WGPU require a native display/adapter.

The triangle's 320×240 size is an initial window request. Each frame queries the
window's actual physical pixel size and uses it for both recording bounds and
submission, including when scaling or resize changes that size. Zero dimensions
suspend without consuming the requested frame count. Its application-owned loop
is shared by the executable and the real linked/shared-library regression test.

## Ownership and deployment

Static assembly registers the platform descriptor and acquires its typed root
through the SDK. The renderer creates and owns its fixed-format compiler.
Dynamic assembly uses final cdylib artifacts explicitly prepared by
`cargo build --examples`; `neocogi-example-plugins` stages their exact named outputs,
loads the platform, then acquires its typed root.
Only final dylib roots export discovery; normal provider implementations are not
Rust dependencies of a dynamic executable. The compiler is internal to the selected renderer.

The application owns its one process-lived SDK host in both modes. Plugin code
and metadata remain mapped under the SDK's existing loader policy. Missing
platform libraries fail through that policy. No new renderer ABI or native-handle exchange
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
cargo test --locked -p neocogi-platform-examples --test triangle -- --include-ignored --test-threads=1
cargo test --locked -p neocogi-platform-examples --no-default-features --features dynamic,glow --test triangle -- --include-ignored --test-threads=1
cargo test --locked -p neocogi-platform-examples --no-default-features --features dynamic,glow --test graphs --test deployment
SDL_VIDEODRIVER=offscreen cargo test --locked -p neocogi-platform-examples --test gltf_viewer --features test-support -- --include-ignored --test-threads=1
SDL_VIDEODRIVER=offscreen cargo test --locked -p neocogi-platform-examples --no-default-features --features dynamic,glow --test deployment -- --include-ignored --test-threads=1
~~~

Use dynamic,vulkan or dynamic,wgpu with a display for the last command.
Ordinary deployment tests copy and rename actual executables/libraries, remove
Cargo loader paths, and acquire through COM without a GPU. Isolated children
exercise missing platform libraries and prove activation needs no compiler plugin.
Opt-in deployment acceptance renders all three real examples after relocation.
Static GL pixel acceptance retains animation, arcball, material/HDR and skybox
checks. Dependency regressions use the selected Cargo tree and verify that dynamic
applications enable no SDL, graphics-loader or compiler implementation dependency.
The triangle regression renders a real framebuffer whose extent differs from its
initial request, checks recorded bounds and successful submissions, counts size
queries, and tests a suspended iteration. It does not require a particular monitor's
HiDPI setting; use a working native display/adapter or SDL offscreen with Glow.
