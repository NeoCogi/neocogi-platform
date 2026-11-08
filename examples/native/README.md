# Native example assembly

The native applications are triangle, gltf_damaged_helmet and gltf_brainstem.
They share an example-owned typed platform bootstrap and the same graphics COM
render loop in both assembly modes. Native platform-root/session behavior is
unchanged; each application acquires and retains one root.

Default features select static registration and Glow:

~~~sh
cargo tools fetch-gltf-examples
cargo run --locked -p neocogi-platform-examples --bin triangle -- 3
cargo run --locked -p neocogi-platform-examples --bin gltf_damaged_helmet
cargo run --locked -p neocogi-platform-examples --bin gltf_brainstem
~~~

Static builds select one assembly and backend feature. Dynamic examples need only
`--features dynamic`; select the library at runtime with `--backend NAME` when
several are deployed. To load shared objects:

~~~sh
cargo build --locked -p neocogi-platform-dylib --no-default-features --features glow --examples
cargo run --locked -p neocogi-platform-examples --bin triangle --no-default-features --features dynamic -- 3
cargo build --locked -p neocogi-platform-dylib --no-default-features --features vulkan --examples
cargo run --locked -p neocogi-platform-examples --bin gltf_damaged_helmet --no-default-features --features dynamic -- --backend vulkan --frames 3
cargo build --locked -p neocogi-platform-dylib --no-default-features --features wgpu --examples
cargo run --locked -p neocogi-platform-examples --bin gltf_brainstem --no-default-features --features dynamic -- --backend wgpu --frames 3 --time 1
~~~

For a min-size dynamic build, prepare the same profile before running:

~~~sh
cargo build --profile min-size -p neocogi-platform-dylib --no-default-features --features vulkan --examples
cargo run --profile min-size -p neocogi-platform-examples --bin gltf_brainstem --no-default-features --features dynamic
~~~

Both commands must use the same profile and target. The first builds the selected
backend's named Cargo platform library example; the second automatically stages
all available backend-named platform libraries from that profile's `examples/`
directory into `lib/`. With one deployed library, no runtime option is needed.
With several, append `-- --backend vulkan` (or glow/wgpu). The same dynamic
executable can load any of them without recompilation. Only Cargo and Rust are
needed for building and staging libraries.
Startup checks the platform library and returns an owned error with
the matching preparation command when a file is missing. This check runs before
model import and HDR lighting preparation. It never starts Cargo from a build script
or application. Existing library validation still follows the SDK loader policy.

Use static,vulkan or static,wgpu for linked alternatives. Backend Cargo features
are only valid for static assembly. Dynamic selection occurs once during startup;
each acquired renderer/device retains its immutable backend and shader format.
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

Dynamic applications depend only on `neocogi-platform-contracts` contracts and the
example loader. Asset/plugin build scripts use the contracts crate's `build-support`
API through normal Cargo dependencies. The contracts crate has no renderer or Naga
dependency, so dynamic builds compile neither in host or target units. Linked
static applications use the SDK and include their renderer/compiler normally.

Platform roots also expose typed PCM capabilities and playback/capture opening.
Audio lives in the same selected platform library and can be used without creating
a window. See [audio contracts](../../docs/contracts/AUDIO.md).

The application owns its one process-lived SDK host in both modes. Plugin code
and metadata remain mapped under the SDK's existing loader policy. Missing
platform libraries fail through that policy. No new renderer ABI or native-handle exchange
is introduced.

Build and run use the same assembly; only static builds select a backend feature. For a debug deployment, copy the
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
cargo test --locked -p neocogi-platform-examples --no-default-features --features dynamic --test triangle -- --include-ignored --test-threads=1
cargo test --locked -p neocogi-platform-examples --no-default-features --features dynamic --test graphs --test deployment
SDL_VIDEODRIVER=offscreen cargo test --locked -p neocogi-platform-examples --test gltf_viewer --features test-support -- --include-ignored --test-threads=1
SDL_VIDEODRIVER=offscreen cargo test --locked -p neocogi-platform-examples --no-default-features --features dynamic --test deployment -- --include-ignored --test-threads=1
~~~

For Vulkan/WGPU acceptance, build that library and set NEOCOGI_TEST_BACKEND=vulkan
or wgpu while running the same dynamic-only tests with a native display.
NEOCOGI_TEST_BACKEND is an acceptance-test selector when several libraries are
built in the same profile; application selection uses --backend. Ordinary deployment tests copy and rename actual executables/libraries, remove
Cargo loader paths, and acquire through COM without a GPU. Isolated children
exercise missing platform libraries and prove activation needs no compiler plugin.
Opt-in deployment acceptance renders all three real examples after relocation.
Static GL pixel acceptance retains animation, arcball, material/HDR and skybox
checks. Dependency regressions use the selected Cargo tree and verify that dynamic
applications enable no SDL, graphics-loader or compiler implementation dependency.
The ignored multi-library acceptance test needs two platform variants built in
the same profile. It loads each using one unchanged executable, rejects ambiguity
before model/HDR work, and rejects a valid library with a mismatched descriptor.
The triangle regression renders a real framebuffer whose extent differs from its
initial request, checks recorded bounds and successful submissions, counts size
queries, and tests a suspended iteration. It does not require a particular monitor's
HiDPI setting; use a working native display/adapter or SDL offscreen with Glow.
